use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WakeParams {
    pub(super) subject_id: String,
    pub(super) server: crate::model::ProcessIdentity,
}

/// Reconciles one launch after its pane's process may have died.
///
/// How long the recovery sweep waits between passes.
pub(super) const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
/// How often empty managed scopes are cleaned. Slice accumulation is not
/// latency-sensitive, and an idle daemon should not pay a manager transaction per pass.
pub(super) const SCOPE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
/// Starts the bounded recovery sweep. It re-observes recorded launches on a fixed
/// interval, so a missed exit notification is recovered without any client, and
/// it stops with the daemon.
pub(super) fn spawn_sweeper(
    core: &Arc<Mutex<Core>>,
    stopping: &Arc<AtomicBool>,
    verification: &Arc<VerificationBudget>,
) -> Option<JoinHandle<()>> {
    let (registry, verifier, sweep_at) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            Arc::clone(&core.sweep_at),
        )
    };
    let stopping = Arc::clone(stopping);
    let core = Arc::clone(core);
    let verification = Arc::clone(verification);
    thread::Builder::new()
        .name("radar-control-sweep".to_string())
        .spawn(move || {
            // What the last topology pass saw, so an unchanged server costs one
            // read and no record.
            let mut seen: Option<String> = None;
            // Where the last pane-evidence pass stopped, so every recorded launch
            // is observed in turn rather than only the first page.
            let mut pane_cursor: Option<String> = None;
            // The same for the process-exit pass, which is bounded per pass too.
            let mut sweep_cursor: Option<String> = None;
            // And for container observation, so a record set larger than one page
            // is reached over successive passes.
            let mut reconcile_after: Option<String> = None;
            // Empty scopes are cleaned on their own cadence, not every pass, and
            // only once a slice has been seen empty twice. The first pass waits a
            // full interval so a starting daemon does not add manager calls to the
            // launches already contending for it.
            let mut next_cleanup = std::time::Instant::now() + SCOPE_CLEANUP_INTERVAL;
            let mut empty_slices = std::collections::BTreeSet::new();
            while !stopping.load(Ordering::SeqCst) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_millis() as i64);
                match subjects::sweep(&registry, verifier.as_ref(), sweep_cursor.as_deref(), now) {
                    Ok((_, next)) => {
                        sweep_cursor = next;
                        sweep_at.store(now, Ordering::SeqCst);
                    }
                    Err(error) => tracing::error!(%error, "launch recovery sweep failed"),
                }
                // One inventory read serves both the pane evidence a bound launch
                // needs and the adoption pass, so a pass costs one listing.
                let observation = observed_inventory(&core);
                let serving = {
                    let core = locked(&core);
                    (
                        core.backend.clone(),
                        Arc::clone(&core.verifier),
                        Arc::clone(&core.stopping),
                    )
                };
                let serving = serving.0.as_ref().and_then(|backend| {
                    serving_identity(backend, &serving.1, &serving.2, &verification)
                });
                if let Err(error) = observe_launch_panes(
                    &registry,
                    observation.as_ref(),
                    serving.as_ref(),
                    &mut pane_cursor,
                    now,
                    &mut |edge| withdraw_unpresented(&core, &verification, edge, now),
                ) {
                    tracing::error!(%error, "launch pane observation failed");
                }

                if let Err(error) = adopt_topology(
                    &core,
                    &registry,
                    &verification,
                    observation,
                    &mut seen,
                    &mut reconcile_after,
                ) {
                    tracing::error!(%error, "topology adoption failed");
                }
                // A slice stop ends everything inside it, so this only ever stops a
                // group that holds nothing, and it is what clears slices whose exit
                // nothing observed.
                if std::time::Instant::now() >= next_cleanup {
                    match crate::managed_scope::sweep(Some(&mut empty_slices)) {
                        Ok(swept) if swept != crate::managed_scope::Swept::default() => {
                            tracing::info!(?swept, "stopped empty managed scopes");
                        }
                        Ok(_) => {}
                        Err(error) => tracing::error!(%error, "managed scope cleanup failed"),
                    }
                    next_cleanup = std::time::Instant::now() + SCOPE_CLEANUP_INTERVAL;
                }
                let deadline = std::time::Instant::now() + SWEEP_INTERVAL;
                while std::time::Instant::now() < deadline && !stopping.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                }
            }
        })
        .ok()
}

/// Withdraws a launch that never presented, then closes the pane it left behind.
pub(super) fn withdraw_unpresented(
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
    edge: &registry::SpawnEdge,
    now_ms: i64,
) {
    let (registry, verifier, backend, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            core.backend.clone(),
            Arc::clone(&core.stopping),
        )
    };
    let cleanup = |claim_id: &str| {
        if edge.location.is_none() {
            return Ok(edge.created == RequestOutcome::Refused);
        }
        let Some(backend) = &backend else {
            return Ok(false);
        };
        Ok(matches!(
            cleanup_launch_pane(
                &registry,
                backend,
                &verifier,
                claim_id,
                &stopping,
                verification,
                now_ms,
            ),
            Some(subjects::PaneCleanup::Cleaned)
        ))
    };
    if let Err(error) =
        subjects::withdraw_unpresented(&registry, edge, verifier.as_ref(), now_ms, cleanup)
    {
        tracing::warn!(%error, request = %edge.request_id, "launch withdrawal failed");
    }
}

/// How many recorded spawn edges one pane-evidence pass may examine.
pub(super) const MAX_PANE_OBSERVATIONS: usize = 64;

/// The management standing of a publisher's current session, so a publisher can
/// tell managed, unmanaged and duplicate sessions apart after each publication.
/// Read errors answer `unmanaged` rather than failing the publication, which
/// already succeeded.
pub(super) fn management_view(
    registry: &registry::Registry,
    registration_id: &str,
    now_ms: i64,
) -> serde_json::Value {
    let session = match registry.context(registration_id, now_ms) {
        Ok(Some(context)) => context.context.session,
        _ => None,
    };
    let Some(session) = session else {
        return json!({"state": "unmanaged"});
    };
    match subjects::attachment(registry, registration_id, &session, now_ms) {
        Ok(subjects::Attachment::Own(subject_id)) => {
            json!({"state": "managed", "subject_id": subject_id})
        }
        Ok(subjects::Attachment::Other(_)) => json!({"state": "duplicate"}),
        Ok(subjects::Attachment::Free) | Err(_) => json!({"state": "unmanaged"}),
    }
}

/// Whether the server generation a pane was created in is provably over.
///
/// A pane is owned by exactly one server process. When the server now serving is
/// a different incarnation, every pane recorded under the earlier one died with
/// it — that is absence by identity, not an unreadable read. No server answering
/// at all is not the same claim, so it stays unconfirmed.
pub(super) fn generation_ended(
    recorded: Option<&crate::model::ProcessIdentity>,
    serving: Option<&crate::model::ProcessIdentity>,
) -> bool {
    matches!((recorded, serving), (Some(recorded), Some(serving)) if recorded != serving)
}

/// Records what one inventory read said about each bound launch's pane.
///
/// Paging is fair rather than a sorted prefix: the pass continues after the last
/// edge it examined and wraps when the page ends, so a fleet larger than one page
/// is still covered over successive passes. A failed or unavailable read is
/// passed through as such and never counts as absence. Nothing is dispatched.
pub(super) fn observe_launch_panes(
    registry: &Arc<Registry>,
    observation: Option<&FleetObservation>,
    serving: Option<&crate::model::ProcessIdentity>,
    cursor: &mut Option<String>,
    now_ms: i64,
    unbound: &mut dyn FnMut(&registry::SpawnEdge),
) -> Result<usize, String> {
    let edges = registry.spawn_edges_after(cursor.as_deref(), MAX_PANE_OBSERVATIONS)?;
    *cursor = match edges.len() {
        // A short page is the end of the listing, so the next pass starts over.
        len if len < MAX_PANE_OBSERVATIONS => None,
        _ => edges.last().map(|edge| edge.request_id.clone()),
    };
    let mut ended = 0;
    for edge in edges {
        // Only a bound launch's pane is evidence about a process: an unbound edge
        // may never have released a program, and its absence says nothing.
        if edge.granted_at.is_none() {
            unbound(&edge);
        }
        if edge.bound.is_none() || edge.withdrawn.is_some() {
            continue;
        }
        let Some(subject_id) = edge.claim_id.as_deref() else {
            continue;
        };
        let Some(location) = edge.location.as_ref() else {
            continue;
        };
        let evidence =
            match crate::control_plane::registry_read::location_evidence(observation, &edge) {
                // A pane cannot outlive the server that owned it: a different
                // server now serving is proof its generation ended, so the pane
                // is gone whether or not a fresh listing reached it.
                _ if generation_ended(edge.instance.as_ref(), serving) => {
                    crate::control_plane::lifecycle::PaneEvidence::Absent
                }
                LocationEvidence::Present => crate::control_plane::lifecycle::PaneEvidence::Present,
                LocationEvidence::Absent => crate::control_plane::lifecycle::PaneEvidence::Absent,
                LocationEvidence::Unavailable => {
                    crate::control_plane::lifecycle::PaneEvidence::Unavailable
                }
            };
        match crate::control_plane::lifecycle::observe_pane(
            registry, subject_id, location, evidence, now_ms,
        ) {
            Ok(true) => ended += 1,
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, subject = %subject_id, "pane observation failed"),
        }
    }
    Ok(ended)
}

/// Records the places of this daemon's own server as observed containers, and
/// reports the ones that have gone, when the server's places have changed since
/// the last pass.
///
/// Adoption is scoped to the one server the daemon serves, runs under a verified
/// serving incarnation, and dispatches nothing. A failed read leaves `seen`
/// as it was, so the next pass looks again.
pub(super) fn adopt_topology(
    core: &Arc<Mutex<Core>>,
    registry: &Arc<Registry>,
    verification: &Arc<VerificationBudget>,
    observation: Option<FleetObservation>,
    seen: &mut Option<String>,
    reconcile_after: &mut Option<String>,
) -> Result<(), String> {
    let (backend, verifier, stopping) = {
        let core = locked(core);
        (
            core.backend.clone(),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let Some(backend) = backend else {
        return Ok(());
    };
    // A creation records the container it made after the place exists. Adopting
    // in between would record the same place twice, so the pass waits for it.
    if locked(core)
        .store
        .unfinished()?
        .iter()
        .any(|record| record.method == "topology.create")
    {
        return Ok(());
    }
    let Some(observation) = observation else {
        return Ok(());
    };
    let incarnation = serving_identity(&backend, &verifier, &stopping, verification);
    let inventory = containers::Inventory::from_observation(
        &observation,
        &backend.identity(),
        incarnation.clone(),
    );
    let signature = format!(
        "{:?}|{:?}|{:?}|{:?}",
        inventory.incarnation, inventory.workspaces, inventory.tabs, inventory.panes
    );
    let at = store::format_millis(unix_millis());
    // Adoption is one-time per place, so an unchanged server costs no record.
    if seen.as_deref() != Some(signature.as_str()) {
        containers::adopt(
            registry,
            &observation,
            &backend.identity(),
            incarnation.as_ref(),
            &at,
        )?;
        *seen = Some(signature);
    }
    // Observation is not one-time: a recorded container is re-observed whether or
    // not the server changed, one bounded page per pass, resuming where the last
    // pass stopped so every container is reached in turn.
    let outcome = containers::reconcile(
        registry,
        &inventory,
        reconcile_after.as_deref(),
        &at,
        MAX_RECONCILE,
    )?;
    *reconcile_after = outcome.next_after;
    Ok(())
}

/// A wake is honoured only from the server generation that created the pane and
/// is serving now, so a pane id reused by a replaced server is not this launch's
/// pane. It records an exit only when pidfd verification shows the process gone.
pub(super) fn wake_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let params: WakeParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let (registry, backend, verifier, stopping) = {
        let core = locked(core);
        let Some(backend) = core.backend.clone() else {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "no backend is wired"),
            );
        };
        (
            Arc::clone(&core.registry),
            backend,
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let subject = match subjects::read(&registry, &params.subject_id) {
        Ok(Some(subject)) => subject,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "launch subject is not recorded"),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let edge = match registry.spawn_edge(&subject.process.spawn_request_id) {
        Ok(Some(edge)) => edge,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "spawn is not recorded"),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let serving = serving_identity(&backend, &verifier, &stopping, verification);
    if edge.instance.as_ref() != Some(&params.server) || serving.as_ref() != Some(&params.server) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "wake names a server generation that does not serve this launch",
            ),
        );
    }
    match subjects::observe_exit_from(
        &registry,
        &params.subject_id,
        verifier.as_ref(),
        store::now_ms(),
        subjects::Observer::TmuxHook,
    ) {
        Ok(exited) => {
            if exited {
                retire_wake(&backend, &params.subject_id, &stopping);
            }
            Response::ok(&request.id, json!({ "exited": exited }))
        }
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// The location a create answered with, in the coordinates this daemon records.
///
/// The backend names itself, so a location recorded through one daemon is
/// distinguishable from the same coordinates read through another.
pub(super) fn created_location(
    named: &CreatedLocation,
    backend: &BackendIdentity,
) -> RegistryLocation {
    RegistryLocation {
        backend: backend.name.clone(),
        instance: backend.instance.clone(),
        workspace: named.workspace_id.clone(),
        tab: named.tab_id.clone(),
        pane: named.pane_id.clone(),
    }
}
