use super::*;

/// Daemon-wide process-verification admission. A detached call keeps its slot
/// until the injected verifier returns, so timeouts cannot grow outstanding work.
pub(super) struct VerificationBudget {
    pub(super) active: Mutex<usize>,
    pub(super) workers: Mutex<Vec<JoinHandle<()>>>,
}

impl VerificationBudget {
    pub(super) fn acquire(self: &Arc<Self>) -> Option<VerificationPermit> {
        let mut active = locked(&self.active);
        if *active >= MAX_VERIFICATION_JOBS {
            return None;
        }
        *active += 1;
        Some(VerificationPermit(Arc::clone(self)))
    }

    pub(super) fn track(&self, worker: JoinHandle<()>) {
        let mut workers = locked(&self.workers);
        let mut unfinished = Vec::with_capacity(workers.len() + 1);
        for worker in workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                unfinished.push(worker);
            }
        }
        unfinished.push(worker);
        *workers = unfinished;
    }

    pub(super) fn reap(&self) {
        let mut workers = locked(&self.workers);
        let mut unfinished = Vec::with_capacity(workers.len());
        for worker in workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                unfinished.push(worker);
            }
        }
        *workers = unfinished;
    }

    pub(super) fn detach_remaining(&self) {
        let mut workers = locked(&self.workers);
        let all = std::mem::take(&mut *workers);
        for worker in all {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                drop(worker);
            }
        }
    }
}

pub(super) struct VerificationPermit(Arc<VerificationBudget>);

impl Drop for VerificationPermit {
    fn drop(&mut self) {
        let mut active = locked(&self.0.active);
        *active -= 1;
    }
}

/// Runs one process verification on its own thread, bounded by `deadline`.
///
/// The verifier may read procfs and can, in a test, block; the connection worker
/// polls so a stop request is noticed in [`VERIFY_POLL`] rather than after the
/// whole deadline. A verification that does not finish in time is `unavailable`,
/// which is explicitly not evidence of absence.
pub(super) fn verify_bounded(
    process: Option<crate::model::ProcessIdentity>,
    verifier: &Arc<dyn ProcessVerifier>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> registry::ProcessVerification {
    if deadline.saturating_duration_since(Instant::now()).is_zero()
        || stopping.load(Ordering::SeqCst)
    {
        return registry::ProcessVerification::Unavailable;
    }
    budget.reap();
    let Some(permit) = budget.acquire() else {
        return registry::ProcessVerification::Unavailable;
    };
    let Some(process) = process else {
        drop(permit);
        return registry::ProcessVerification::Unavailable;
    };
    let (send, receive) = mpsc::sync_channel(1);
    let verifier = Arc::clone(verifier);
    let spawned = thread::Builder::new()
        .name("radar-agent-verify".into())
        .spawn(move || {
            let _permit = permit;
            let result = registry::verify_identity(Some(&process), verifier.as_ref());
            let _ = send.send(result);
        });
    match spawned {
        Ok(worker) => budget.track(worker),
        Err(_) => return registry::ProcessVerification::Unavailable,
    }
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return registry::ProcessVerification::Unavailable;
        }
        match receive.recv_timeout(remaining.min(VERIFY_POLL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return registry::ProcessVerification::Unavailable;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stopping.load(Ordering::SeqCst) {
                    return registry::ProcessVerification::Unavailable;
                }
            }
        }
    }
}

/// Run one evidence read on a bounded worker. The same global budget bounds
/// backend inventory, foreground evidence and procfs verification for close.
pub(super) fn run_bounded<T: Send + 'static>(
    budget: &Arc<VerificationBudget>,
    stopping: &Arc<AtomicBool>,
    deadline: Instant,
    name: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ()> {
    if deadline.saturating_duration_since(Instant::now()).is_zero()
        || stopping.load(Ordering::SeqCst)
    {
        return Err(());
    }
    budget.reap();
    let Some(permit) = budget.acquire() else {
        return Err(());
    };
    let (send, receive) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _permit = permit;
            let result = work();
            let _ = send.send(result);
        })
        .map_err(|_| ())?;
    budget.track(worker);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stopping.load(Ordering::SeqCst) {
            return Err(());
        }
        match receive.recv_timeout(remaining.min(VERIFY_POLL)) {
            Ok(result) => return Ok(result),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Verifies that a pane's live foreground process is the exact process the bound
/// child registered.
///
/// A pane is closed on a live occupant only when the birth identity read now is
/// the identity that was claimed: a matching PID alone is not an identity. Any
/// other answer is a refusal, and the caller repeats it unless dead-pane evidence
/// replaces this gate.
pub(super) fn verify_live_occupant(
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    pane_id: &str,
    claim: &crate::model::ProcessIdentity,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Result<(), Refusal> {
    let foreground = match bounded_foreground(
        backend,
        pane_id,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(evidence) => evidence,
        Err(()) => {
            return Err(Refusal::new(
                Code::BackendUnavailable,
                "foreground process evidence is unavailable",
            ));
        }
    };
    let pid = match foreground {
        crate::model::ForegroundEvidence::NonShell { pid, .. } if pid > 0 => pid,
        crate::model::ForegroundEvidence::NonShell { .. } => {
            return Err(Refusal::new(
                Code::Refused,
                "foreground process PID is invalid",
            ));
        }
        crate::model::ForegroundEvidence::Shell
        | crate::model::ForegroundEvidence::Inconclusive => {
            return Err(Refusal::new(
                Code::Refused,
                "foreground process evidence is inconclusive",
            ));
        }
    };
    // The pane's own process is the one supervising the claimed program, so the
    // claim is the foreground or a descendant of it, and its birth identity is
    // what establishes that it is the process the record names.
    let observed = verify_birth_identity(
        claim.pid,
        verifier,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    );
    match observed {
        Ok(observed) if observed == *claim && super::launch::descends_from(claim.pid, pid) => {
            Ok(())
        }
        _ => Err(Refusal::new(
            Code::Refused,
            "foreground process birth identity is unavailable or changed",
        )),
    }
}

/// What the dead-pane evidence says about a pane the live gate could not verify.
pub(super) enum DeadPane {
    /// A retained pane of this edge, in the very instance that created it: its
    /// cleanup is defined. Carries the status the program's supervisor reported,
    /// when it did: a missing status never makes the pane unclosable.
    Cleanup(Option<registry::SpawnExitObservation>),
    /// Nothing here says the pane is dead and this edge's: the live refusal
    /// stands.
    Unresolved,
    /// The instance that created the recorded pane cannot be established, which
    /// nothing here may infer: refuse for that reason rather than adopt a pane
    /// that merely carries the same id.
    Unfenced(Refusal),
}

/// Evidence that this edge's recorded pane is a retained, already-dead pane whose
/// cleanup is defined, or why it is not.
///
/// The live gate protects a process that may be this child's and alive; a pane
/// reported dead now has no such occupant. What replaces the birth-identity match
/// is that the daemon created this location (the edge names this backend and
/// server instance and was created in it), that the pane's command exit is
/// already a recorded fact of this edge, and that the backend reports the pane
/// dead in a fresh read. History alone never closes anything, and a
/// re-pointed edge is not eligible.
pub(super) fn dead_pane_evidence(
    registry: &Arc<Registry>,
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    close: &registry::ChildCloseRequest,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> DeadPane {
    if !backend.primitives().contains(Primitives::EXIT_OBSERVATION) {
        return DeadPane::Unresolved;
    }
    let Ok(Some(edge)) = registry.spawn_edge(&close.spawn_request_id) else {
        return DeadPane::Unresolved;
    };
    let Some(location) = edge.location.as_ref() else {
        return DeadPane::Unresolved;
    };
    let identity = backend.identity();
    if location.backend != identity.name
        || location.instance != identity.instance
        || location.pane.as_deref() != Some(pane_id)
    {
        return DeadPane::Unresolved;
    }
    // A pane id outlives the server that answered the create, so the pane is only
    // this edge's while the very process that served the create is still the one
    // serving: a restart under the same name hands the same id to a pane that was
    // never Radar's to close.
    let Some(created_in) = edge.instance.clone() else {
        return DeadPane::Unfenced(Refusal::new(
            Code::Refused,
            "the spawn edge records no verified backend instance for its pane",
        ));
    };
    match serving_identity(backend, verifier, stopping, budget) {
        Some(serving) if serving == created_in => {}
        _ => {
            return DeadPane::Unfenced(Refusal::new(
                Code::Refused,
                "the backend instance serving now is not the one that created the recorded pane",
            ));
        }
    }
    match bounded_exit_observation(backend, pane_id, stopping, budget) {
        Ok(PaneExitObservation::Exited { .. }) => DeadPane::Cleanup(recorded_exit(
            registry,
            edge.claim_id.as_deref().and_then(ClaimId::parse).as_ref(),
        )),
        // A pane reported running, undetermined, or unreadable is not a pane whose
        // command has exited: the live refusal stands.
        Ok(PaneExitObservation::Running | PaneExitObservation::Unknown) | Err(()) => {
            DeadPane::Unresolved
        }
    }
}

fn recorded_exit(
    registry: &Registry,
    claim_id: Option<&ClaimId>,
) -> Option<registry::SpawnExitObservation> {
    match subjects::spawn_exit(registry, claim_id) {
        registry::PublicSpawnExitObservation::Exited {
            code: None,
            signal: None,
            ..
        }
        | registry::PublicSpawnExitObservation::Unknown => None,
        registry::PublicSpawnExitObservation::Exited {
            code,
            signal,
            observed_at,
        } => Some(registry::SpawnExitObservation {
            code,
            signal,
            observed_at,
        }),
    }
}

/// The verified birth identity of the process serving `backend` now, read within
/// the daemon's verification budget.
///
/// Both reads — the backend's own answer for its serving process and the process
/// behind that pid — run bounded on the shared budget, so a stalled backend or
/// verifier is abandoned at the deadline instead of holding the worker that is
/// answering a caller. A claim that cannot be read, a process that is gone and a
/// pid the verifier does not confirm are all none, so neither a recorded
/// instance nor a compared one is ever a guess.
pub(super) fn serving_identity(
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Option<crate::model::ProcessIdentity> {
    let deadline = Instant::now() + VERIFICATION_TIMEOUT;
    let reader = Arc::clone(backend);
    let cancel = Arc::clone(stopping);
    let pid = run_bounded(
        budget,
        stopping,
        deadline,
        "radar-serving-process",
        move || reader.server_process(&cancel),
    )
    .ok()??;
    verify_birth_identity(pid, verifier, stopping, budget, deadline)
        .ok()
        .filter(|observed| observed.pid == pid)
}

pub(super) fn bounded_close_dispatch(
    backend: &Arc<dyn RuntimeProvider>,
    target: &crate::CloseTarget,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::runtime::CloseOutcome, ()> {
    let backend = Arc::clone(backend);
    let target = target.clone();
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-child-close-dispatch",
        move || backend.close(&target, &cancel),
    )
}

pub(super) fn bounded_inventory(
    backend: &Arc<dyn RuntimeProvider>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<FleetObservation, ()> {
    let backend = Arc::clone(backend);
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-close-inventory",
        move || backend.inventory(&cancel),
    )?
    .map_err(|_| ())
}

pub(super) fn bounded_foreground(
    backend: &Arc<dyn RuntimeProvider>,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::model::ForegroundEvidence, ()> {
    let backend = Arc::clone(backend);
    let pane_id = pane_id.to_string();
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-close-foreground",
        move || backend.foreground_evidence(&pane_id, &cancel),
    )
}

/// The caller's reason for closing the pane, which decides what "gone" has to
/// mean before it is.
pub(super) enum CleanupIntent {
    /// A cancel or a close ends the launch, so its pane goes once the process is
    /// verified gone.
    Close,
    /// A withdrawal of a launch that never presented, which leaves only the pane
    /// the launch built for itself.
    Retire,
}

/// Closes the pane an unbound launch left behind, fenced to the very instance
/// that created it, and records what happened as that launch's own durable fact.
///
/// Closing a pane ends whatever still runs in it, so a pane is closed only once
/// nothing this daemon launched can still be alive there: the launch's process is
/// verified gone, or the pane itself is a retained, already-exited pane of this
/// edge. Anything less leaves the pane to the operator and says why. This reports
/// a cleanup, never a process exit.
///
/// A refusal is recorded like a dispatch: it says nothing was closed and why, and
/// the first outcome stands. A later admitted cancel re-decides eligibility
/// against the evidence it finds only when nothing was recorded at all.
pub(super) fn cleanup_launch_pane(
    core: &Arc<Mutex<Core>>,
    backend: &Arc<dyn RuntimeProvider>,
    key: &ClaimId,
    intent: CleanupIntent,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    now_ms: i64,
) -> Option<subjects::PaneCleanup> {
    use subjects::{PaneCleanup, ResourceEnd};
    let (registry, verifier) = {
        let core = locked(core);
        (Arc::clone(&core.registry), Arc::clone(&core.verifier))
    };
    let claim = match subjects::read_claim(&registry, key) {
        Ok(Some(claim)) => claim,
        Ok(None) => return None,
        // A record that cannot be read is not an absence: nothing is dispatched
        // on evidence this daemon could not check, and the reason says so.
        Err(error) => {
            let refusal = format!("the launch record could not be read: {error}");
            return Some(PaneCleanup::Refused(refusal));
        }
    };
    if let Some(cleanup) = claim.process.pane_cleanup {
        return Some(cleanup);
    }
    if matches!(intent, CleanupIntent::Retire) && claim.process.origin != subjects::Origin::Spawned
    {
        return None;
    }
    let placement = claim.process.placement?;
    let pane = placement.location.pane?;
    let instance = placement.instance?;
    if serving_identity(backend, &verifier, stopping, budget).as_ref() != Some(&instance) {
        return Some(PaneCleanup::Refused(
            "the execution's serving generation is no longer current".into(),
        ));
    }
    subjects::await_completion(&registry, &claim.process.registration_id);
    let inventory = match bounded_inventory(
        backend,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(inventory) => inventory,
        Err(()) => {
            return Some(PaneCleanup::Refused(
                "fresh inventory is unavailable".into(),
            ));
        }
    };
    let target = crate::CloseTarget::Pane(pane.clone());
    let frozen = CloseRequest {
        execution: None,
        identity: lifecycle::identity(&inventory, &target),
        target,
    };
    let resource = if matches!(intent, CleanupIntent::Retire)
        && inventory.pane(&pane).is_some()
        && !matches!(
            bounded_exit_observation(backend, &pane, stopping, budget),
            Ok(PaneExitObservation::Exited { .. })
        ) {
        ResourceEnd::Kept("the dedicated pane no longer reports an exited command".into())
    } else {
        places::remove_place(core, backend, &frozen, stopping, budget, Some(&instance))
    };
    let outcome = match resource {
        ResourceEnd::Removed => PaneCleanup::Cleaned,
        ResourceEnd::Absent => PaneCleanup::Absent,
        ResourceEnd::Kept(reason) => PaneCleanup::Refused(reason),
        ResourceEnd::Unverified(reason) => PaneCleanup::Unknown(reason),
    };
    Some(
        subjects::record_pane_cleanup(&registry, key, outcome, now_ms).unwrap_or_else(|error| {
            PaneCleanup::Unknown(format!(
                "the cleanup outcome could not be recorded: {error}"
            ))
        }),
    )
}

/// The pane and serving generation a restart may reuse, from the record alone: a
/// spawned launch's own placement that is the anchor. Admitted and enrolled
/// executions live in a shell the operator owns.
fn own_pane(
    origin: subjects::Origin,
    placement: Option<&subjects::Placement>,
    anchor: &str,
) -> Option<(String, crate::model::ProcessIdentity)> {
    if origin != subjects::Origin::Spawned {
        return None;
    }
    let placement = placement?;
    let pane = placement.pane().filter(|pane| *pane == anchor)?;
    Some((pane.to_owned(), placement.instance.clone()?))
}

/// The predecessor's own pane, when a restart may launch into it again.
///
/// Only a pane this daemon made for a spawned launch, of the serving generation
/// that made it, anchoring this restart, and holding an exited command. A shell
/// the operator resumed is never eligible: a replacement is never typed into it.
pub(super) fn reusable_pane(
    core: &Arc<Mutex<Core>>,
    backend: &Arc<dyn RuntimeProvider>,
    key: &ClaimId,
    anchor: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Option<crate::runtime::CreatedLocation> {
    let (registry, verifier) = {
        let core = locked(core);
        (Arc::clone(&core.registry), Arc::clone(&core.verifier))
    };
    let claim = subjects::read_claim(&registry, key).ok()??;
    let (pane, instance) = own_pane(
        claim.process.origin,
        claim.process.placement.as_ref(),
        anchor,
    )?;
    if serving_identity(backend, &verifier, stopping, budget).as_ref() != Some(&instance) {
        return None;
    }
    subjects::await_completion(&registry, &claim.process.registration_id);
    let inventory = bounded_inventory(
        backend,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    )
    .ok()?;
    let location = inventory.pane(&pane)?.location.clone();
    matches!(
        bounded_exit_observation(backend, &pane, stopping, budget),
        Ok(PaneExitObservation::Exited { .. })
    )
    .then_some(crate::runtime::CreatedLocation {
        workspace_id: Some(location.workspace_id),
        tab_id: Some(location.tab_id),
        pane_id: Some(location.pane_id),
    })
}

/// Reads one pane's exit state now, bounded and cancellable like every other
/// read this path makes. A pane the backend does not answer for is unknown.
pub(super) fn bounded_exit_observation(
    backend: &Arc<dyn RuntimeProvider>,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Result<PaneExitObservation, ()> {
    let backend = Arc::clone(backend);
    let panes = vec![pane_id.to_string()];
    let cancel = Arc::clone(stopping);
    let observations = run_bounded(
        budget,
        stopping,
        Instant::now() + VERIFICATION_TIMEOUT,
        "radar-close-exit-observation",
        move || backend.exit_observations(&panes, &cancel),
    )?;
    observations.get(pane_id).cloned().ok_or(())
}

pub(super) fn verify_birth_identity(
    pid: i32,
    verifier: &Arc<dyn ProcessVerifier>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::model::ProcessIdentity, registry::ProcessVerification> {
    let verifier = Arc::clone(verifier);
    match run_bounded(
        budget,
        stopping,
        deadline,
        "radar-verify-birth-identity",
        move || verifier.inspect(pid),
    ) {
        Ok(Ok(Some(identity))) => Ok(identity),
        Ok(Ok(None)) => Err(registry::ProcessVerification::Absent),
        Ok(Err(_)) | Err(()) => Err(registry::ProcessVerification::Unavailable),
    }
}

/// What the backend reports right now, or `None` when it could not be asked.
///
/// A topology read is answered from the daemon's own records whether or not a
/// backend can be consulted, so a backend that is missing, unobservable, failing
/// or stopping leaves a recorded location unconfirmed instead of refusing the
/// read. One observation serves one answer, and the call runs outside every
/// registry lock because it reaches the runtime.
pub(super) fn observed_inventory(core: &Arc<Mutex<Core>>) -> Option<FleetObservation> {
    let (backend, cancel) = {
        let core = locked(core);
        (core.backend.clone(), Arc::clone(&core.stopping))
    };
    let backend = backend?;
    // The same primitive the wire `mux.observe` read needs: a backend that
    // cannot answer that read cannot confirm a location here either.
    if primitive_refuse(&backend, protocol::OBSERVE_METHOD).is_some()
        || cancel.load(Ordering::SeqCst)
    {
        return None;
    }
    match backend.inventory(&cancel) {
        Ok(inventory) if !cancel.load(Ordering::SeqCst) => Some(inventory),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProcessIdentity;

    fn placed(pane: &str, instance: bool) -> subjects::Placement {
        subjects::Placement {
            location: registry::RegistryLocation {
                backend: "tmux".into(),
                instance: None,
                workspace: None,
                tab: None,
                pane: Some(pane.into()),
            },
            instance: instance.then(|| ProcessIdentity {
                boot_id: "boot".into(),
                pid: 7,
                start_ticks: 9,
            }),
        }
    }

    #[test]
    fn only_a_spawned_launchs_own_anchor_pane_is_reusable() {
        let own = placed("%1", true);
        assert_eq!(
            own_pane(subjects::Origin::Spawned, Some(&own), "%1").map(|(pane, _)| pane),
            Some("%1".into())
        );
        // A shell the operator owns is never typed into, however it was placed.
        for origin in [subjects::Origin::Admitted, subjects::Origin::Enrolled] {
            assert!(own_pane(origin, Some(&own), "%1").is_none(), "{origin:?}");
        }
        // Another pane, an unrecorded serving generation, or no placement at all
        // leave nothing the daemon can show it made.
        assert!(own_pane(subjects::Origin::Spawned, Some(&own), "%2").is_none());
        assert!(own_pane(subjects::Origin::Spawned, Some(&placed("%1", false)), "%1").is_none());
        assert!(own_pane(subjects::Origin::Spawned, None, "%1").is_none());
    }
}
