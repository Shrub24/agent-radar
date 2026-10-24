use super::*;

pub(super) fn presence_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    use registry::presence::{self, Standing};
    let (registry, verifier, backend, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            core.backend.clone(),
            Arc::clone(&core.stopping),
        )
    };
    let _presence = registry.presence.lock().unwrap_or_else(|e| e.into_inner());
    let now = store::now_ms();
    let result = (|| -> Result<presence::Decision, String> {
        if request.method == "agent.release" {
            let release: presence::Release =
                serde_json::from_value(request.params.clone()).map_err(|e| e.to_string())?;
            let registration = registry
                .get(&release.registration_id)?
                .ok_or("registration is not recorded")?;
            if registration.request.process.as_ref() != Some(&release.process)
                || !matches!(verifier.inspect(release.process.pid), Ok(Some(p)) if p == release.process)
            {
                return Err("release does not identify the registered live process".into());
            }
            return presence::release(&registry, &release.registration_id, &release.claim_id);
        }
        let report: presence::Report =
            serde_json::from_value(request.params.clone()).map_err(|e| e.to_string())?;
        for id in [&report.claim_id, &report.session_id, &report.incarnation] {
            registry::validate_registration_id(id)?;
        }
        if let Some(subject_id) = &report.subject_id {
            registry::validate_registration_id(subject_id)?;
        }
        let process_observation = verifier.inspect(report.process.pid);
        match &process_observation {
            Ok(Some(process)) if process == &report.process => {}
            Err(_) => {}
            Ok(_) => return Err("process birth identity is absent or mismatched".into()),
        }
        let registration = presence::registration(&registry, &report, now)?;
        let mut claim = presence::begin(
            &registry,
            &registration.registration_id,
            &report.claim_id,
            &report.session_id,
            None,
        )?;
        if matches!(claim.standing, Standing::Released | Standing::Duplicate) {
            return Ok(claim);
        }
        if process_observation.is_err() {
            return Ok(claim);
        }
        let mut attachment = subjects::attachment(
            &registry,
            &registration.registration_id,
            &report.session_id,
            now,
        )?;
        if let subjects::Attachment::Other(holder) = &attachment
            && subjects::release_if_gone(&registry, holder, verifier.as_ref(), now)?
        {
            attachment = subjects::attachment(
                &registry,
                &registration.registration_id,
                &report.session_id,
                now,
            )?;
        }
        if matches!(attachment, subjects::Attachment::Other(_)) {
            claim.standing = Standing::Duplicate;
            claim.subject_id = None;
            presence::save(&registry, &claim)?;
            return Ok(claim);
        }
        let Some(backend) = backend else {
            return Ok(claim);
        };
        let Some(pane) = report
            .location
            .as_ref()
            .and_then(|location| location.pane.as_deref())
        else {
            return Ok(claim);
        };
        if verify_live_occupant(
            &backend,
            &verifier,
            pane,
            &report.process,
            &stopping,
            verification,
        )
        .is_err()
        {
            return Ok(claim);
        }
        claim = presence::begin(
            &registry,
            &registration.registration_id,
            &report.claim_id,
            &report.session_id,
            report.location.clone(),
        )?;
        registry::context::report_presence(
            &registry,
            &registration.registration_id,
            registry::PublisherIdentity {
                source: registration.request.source.clone(),
                incarnation: registration.request.incarnation.clone(),
                reporting_owner: None,
            },
            Some(report.session_id.clone()),
            now,
        )?;
        subjects::record_launch_session(
            &registry,
            &registration.registration_id,
            &report.session_id,
            now,
        )?;
        attachment = subjects::attachment(
            &registry,
            &registration.registration_id,
            &report.session_id,
            now,
        )?;
        let subject = match attachment {
            subjects::Attachment::Other(_) => {
                claim.standing = Standing::Duplicate;
                claim.subject_id = None;
                presence::save(&registry, &claim)?;
                return Ok(claim);
            }
            subjects::Attachment::Own(subject) if subject.claim_id == report.claim_id => *subject,
            subjects::Attachment::Own(_) | subjects::Attachment::Free => {
                let id = record_enrollment(
                    &registry,
                    &verifier,
                    &EnrollParams {
                        registration_id: registration.registration_id.clone(),
                        session: report.session_id,
                    },
                    &report.process,
                    &report.claim_id,
                    now,
                )?;
                subjects::read(&registry, &id)?.ok_or("enrolled subject is missing")?
            }
        };
        let subject_id = subject.subject_id.unwrap_or(subject.claim_id);
        if report
            .subject_id
            .as_deref()
            .is_some_and(|known| known != subject_id)
        {
            return Err("reported subject does not match Radar's stored claim".into());
        }
        claim.subject_id = Some(subject_id);
        claim.standing = Standing::Granted;
        presence::save(&registry, &claim)?;
        if subject.process.origin == subjects::Origin::Spawned {
            registry.record_spawn_granted(&subject.process.spawn_request_id, now)?;
        }
        Ok(claim)
    })();
    match result {
        Ok(claim) => Response::ok(&request.id, json!(presence::Answer::from(&claim))),
        Err(error) => registry_error(request, error),
    }
}

pub(super) fn registry_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    match request.method.as_str() {
        "agent.presence" | "agent.release" => presence_request(core, request, verification),
        "agent.register" => {
            let registration: RegistrationRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.register(&registration, store::now_ms()) {
                Ok(record) => Response::ok(&request.id, json!({"registration": record.public()})),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.acquire" => {
            let acquire: AcquireRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.acquire_writer(&acquire, store::now_ms()) {
                Ok(binding) => Response::ok(&request.id, json!({"writer": binding})),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.publish" => {
            let publish: registry::PublishRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.publish(&publish, store::now_ms()) {
                Ok(record) => Response::ok(
                    &request.id,
                    json!({"channel": record.view(store::now_ms(), core.registry.serving_epoch())}),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.context" => {
            let publish: registry::ContextRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            let now_ms = store::now_ms();
            match core.registry.publish_context(&publish, now_ms) {
                Ok(result) => {
                    // A launched process learns its session by reporting it; record it
                    // on its bound subject, so that subject can be attached. A failure
                    // leaves the subject unattached, which is reported, not retried here.
                    if let Some(session) = publish.context.session.as_deref()
                        && let Err(error) = subjects::record_launch_session(
                            &core.registry,
                            &publish.registration_id,
                            session,
                            now_ms,
                        )
                    {
                        tracing::warn!(%error, "could not record a launched session");
                    }
                    Response::ok(
                        &request.id,
                        json!({
                            // The write response returns the credential only to its
                            // publisher; agent.get/list use the credential-free projection.
                            "writer": result.record.writer,
                            "context": result.record.public(now_ms, core.registry.serving_epoch()),
                            "warning": result.warning,
                            "management": management_view(&core.registry, &publish.registration_id, now_ms),
                        }),
                    )
                }
                Err(error) => registry_error(request, error),
            }
        }
        "agent.retire" => {
            let params: AgentRetireParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.retire(
                &params.registration_id,
                params.channel,
                &params.writer_handle,
                store::now_ms(),
            ) {
                Ok(record) => Response::ok(
                    &request.id,
                    json!({"channel": record.view(store::now_ms(), core.registry.serving_epoch())}),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.get" => {
            let params: AgentGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let (registry, verifier, stopping) = {
                let core = locked(core);
                (
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                    Arc::clone(&core.stopping),
                )
            };
            let registration = match registry.get(&params.registration_id) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::NotFound, "agent is not registered"),
                    );
                }
                Err(error) => return registry_error(request, error),
            };
            // Verification is external evidence, so it runs outside every
            // registry lock and is bounded: a slow verifier must not hold the
            // worker past the deadline or a stop request.
            let deadline = Instant::now() + VERIFICATION_TIMEOUT;
            let process = registration.request.process.clone();
            let verified = verify_bounded(process, &verifier, &stopping, verification, deadline);
            if stopping.load(Ordering::SeqCst) {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::Busy, "daemon is stopping"),
                );
            }
            // One agent record, built once: a single read and a page of a list
            // answer with the same shape, so a client never has to guess which
            // field a second call would have added.
            let entry = match crate::control_plane::registry_read::agent_entry(
                &registry,
                &registration,
                verified,
                store::now_ms(),
            ) {
                Ok(entry) => entry,
                Err(error) => return registry_error(request, error),
            };
            Response::ok(&request.id, json!({"agent": entry}))
        }
        "agent.list" => {
            let params: AgentListParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_REGISTRY_PAGE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_REGISTRY_PAGE}"),
                        ),
                    );
                }
                _ => {}
            }
            let (registry, verifier, stopping) = {
                let core = locked(core);
                (
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                    Arc::clone(&core.stopping),
                )
            };
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            // One page reads its registry entries once, then verifies each with a
            // single shared deadline so a page of blocked verifiers cannot hold
            // the worker, or the shutdown that joins it, without bound.
            let deadline = Instant::now() + VERIFICATION_TIMEOUT;
            // One record beyond the page tells whether a continuation exists,
            // without reading the whole fleet to page a bounded answer.
            let read = match registry.list_after(params.after.as_deref(), limit + 1) {
                Ok(records) => records,
                Err(error) => return registry_error(request, error),
            };
            let has_more = read.len() > limit;
            let candidates: Vec<_> = read.into_iter().take(limit).collect();
            let now_ms = store::now_ms();
            let mut entries = Vec::new();
            let mut used = 0usize;
            for registration in &candidates {
                let process = registration.request.process.clone();
                let verified =
                    verify_bounded(process, &verifier, &stopping, verification, deadline);
                let entry = match crate::control_plane::registry_read::agent_entry(
                    &registry,
                    registration,
                    verified,
                    now_ms,
                ) {
                    Ok(entry) => entry,
                    Err(error) => return registry_error(request, error),
                };
                let size = serde_json::to_vec(&entry).map_or(usize::MAX, |bytes| bytes.len() + 1);
                if used + size > MAX_LIST_RESPONSE_BYTES {
                    if entries.is_empty() {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(
                                Code::Refused,
                                "one agent record exceeds the registry response byte bound",
                            ),
                        );
                    }
                    break;
                }
                used += size;
                entries.push(entry);
            }
            // The continuation follows the last key actually returned, so a page
            // cut short by the byte bound resumes without skipping a record.
            let next = (entries.len() < candidates.len() || has_more).then(|| {
                entries
                    .last()
                    .expect("a continuation has a returned record")["registration"]["registration_id"]
                    .as_str()
                    .expect("public registration id")
                    .to_owned()
            });
            Response::ok(&request.id, json!({"agents": entries, "next": next}))
        }
        "spawn.get" => {
            let params: SpawnGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let stored = {
                let core = locked(core);
                match core.registry.spawn_edge(&params.request_id) {
                    Ok(Some(edge)) => edge,
                    Ok(None) => {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(Code::NotFound, "spawn edge is not recorded"),
                        );
                    }
                    Err(error) => return registry_error(request, error),
                }
            };
            let stored = match observe_spawn_exits(core, &[stored]) {
                Ok(mut edges) => edges.pop().expect("one spawn edge was supplied"),
                Err(error) => return registry_error(request, error),
            };
            // Location presence is separate from exit evidence: a missing pane
            // never becomes an exit observation.
            let inventory = observed_inventory(core);
            let evidence =
                crate::control_plane::registry_read::location_evidence(inventory.as_ref(), &stored);
            Response::ok(&request.id, json!({"spawn": stored.topology(evidence)}))
        }
        "lifecycle.get" => {
            let params: LifecycleGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            if SessionUuid::parse(&params.subject_id).is_none() {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`subject_id` must be a canonical UUID"),
                );
            }
            let (registry, verifier) = {
                let core = locked(core);
                (Arc::clone(&core.registry), Arc::clone(&core.verifier))
            };
            let current = match subjects::current_attachment(&registry, &params.subject_id) {
                Ok(Some(subject)) => subject,
                Ok(None) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::NotFound, "launch subject is not recorded"),
                    );
                }
                Err(error) => return registry_error(request, error),
            };
            // Observe exit for the latest execution by verified identity before
            // answering; the public ID remains the durable subject.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as i64);
            if let Err(error) =
                subjects::observe_exit(&registry, &current.claim_id, verifier.as_ref(), now)
            {
                return registry_error(request, error);
            }
            match subjects::current_attachment(&registry, &params.subject_id) {
                Ok(Some(subject)) => Response::ok(
                    &request.id,
                    json!({"subject": subjects::public_view(&subject), "source": observation_source(core)}),
                ),
                Ok(None) => Response::refused(
                    &request.id,
                    &Refusal::new(Code::NotFound, "launch subject is not recorded"),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "lifecycle.list" => {
            let params: LifecycleListParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_REGISTRY_PAGE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_REGISTRY_PAGE}"),
                        ),
                    );
                }
                _ => {}
            }
            if params
                .after
                .as_deref()
                .is_some_and(|after| SessionUuid::parse(after).is_none())
            {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`after` must be a canonical UUID"),
                );
            }
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            let (registry, verifier) = {
                let core = locked(core);
                (Arc::clone(&core.registry), Arc::clone(&core.verifier))
            };
            // One record beyond the page tells whether a continuation exists.
            let read = match subjects::list_subjects_after(
                &registry,
                params.after.as_deref(),
                limit + 1,
            ) {
                Ok(read) => read,
                Err(error) => return registry_error(request, error),
            };
            let has_more = read.len() > limit;
            let page: Vec<(String, String)> = read
                .iter()
                .take(limit)
                .map(|subject| {
                    (
                        subject
                            .subject_id
                            .clone()
                            .unwrap_or_else(|| subject.claim_id.clone()),
                        subject.claim_id.clone(),
                    )
                })
                .collect();
            let now = unix_millis();
            let mut views = Vec::with_capacity(page.len());
            for (subject_id, claim_id) in &page {
                if let Err(error) =
                    subjects::observe_exit(&registry, claim_id, verifier.as_ref(), now)
                {
                    return registry_error(request, error);
                }
                match subjects::current_attachment(&registry, subject_id) {
                    Ok(Some(subject)) => views.push(subjects::public_view(&subject)),
                    Ok(None) => {}
                    Err(error) => return registry_error(request, error),
                }
            }
            let next_after = if has_more {
                page.last().map(|(subject, _)| subject.clone())
            } else {
                None
            };
            Response::ok(
                &request.id,
                json!({
                    "subjects": views,
                    "has_more": has_more,
                    "next_after": next_after,
                    "source": observation_source(core),
                }),
            )
        }
        "lifecycle.reconcile" => {
            // An explicit operator request is one of the two triggers; the other
            // is daemon start. Nothing here dispatches an effect: a pass settles
            // only what independent evidence establishes.
            let (store, registry, verifier) = {
                let core = locked(core);
                (
                    core.store.clone(),
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                )
            };
            match reconcile::unresolved(
                &store,
                &registry,
                verifier.as_ref(),
                reconcile::Trigger::OperatorRequest,
                unix_millis(),
            ) {
                Ok(outcome) => Response::ok(&request.id, json!({"reconciled": outcome})),
                Err(error) => registry_error(request, error),
            }
        }
        "topology.get" => {
            let params: ContainerGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            if SessionUuid::parse(&params.container_id).is_none() {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`container_id` must be a canonical UUID"),
                );
            }
            let registry = {
                let core = locked(core);
                Arc::clone(&core.registry)
            };
            match containers::read(&registry, &params.container_id) {
                Ok(Some(container)) => {
                    Response::ok(&request.id, json!({"container": container.public()}))
                }
                Ok(None) => Response::refused(
                    &request.id,
                    &Refusal::new(Code::NotFound, "container is not recorded"),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "topology.list" => {
            let params: ContainerListParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_REGISTRY_PAGE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_REGISTRY_PAGE}"),
                        ),
                    );
                }
                _ => {}
            }
            if params
                .after
                .as_deref()
                .is_some_and(|after| SessionUuid::parse(after).is_none())
            {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`after` must be a canonical UUID"),
                );
            }
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            let registry = {
                let core = locked(core);
                Arc::clone(&core.registry)
            };
            // One record beyond the page tells whether a continuation exists.
            let read = match containers::list_after(&registry, params.after.as_deref(), limit + 1) {
                Ok(read) => read,
                Err(error) => return registry_error(request, error),
            };
            let has_more = read.len() > limit;
            let page: Vec<_> = read
                .iter()
                .take(limit)
                .map(|container| container.public())
                .collect();
            let next_after = if has_more {
                page.last().map(|container| container.container_id.clone())
            } else {
                None
            };
            Response::ok(
                &request.id,
                json!({"containers": page, "has_more": has_more, "next_after": next_after}),
            )
        }
        "topology.reconcile" => {
            let params: ContainerReconcileParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_RECONCILE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_RECONCILE}"),
                        ),
                    );
                }
                _ => {}
            }
            let limit = params.limit.unwrap_or(MAX_RECONCILE).min(MAX_RECONCILE);
            let backend = {
                let core = locked(core);
                core.backend.clone()
            };
            // A reconciliation needs an inventory to reconcile against: without
            // one it refuses rather than recording that everything is missing.
            let Some(backend) = backend else {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BackendUnavailable, "no mux backend is wired"),
                );
            };
            let Some(inventory) = observed_inventory(core) else {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::BackendUnavailable,
                        "the backend did not answer an inventory read",
                    ),
                );
            };
            let (registry, verifier, stopping) = {
                let core = locked(core);
                (
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                    Arc::clone(&core.stopping),
                )
            };
            // Every handle in the snapshot belongs to the incarnation serving it,
            // so an unverified serving process confirms nothing.
            let incarnation = serving_identity(&backend, &verifier, &stopping, verification);
            let inventory = containers::Inventory::from_observation(
                &inventory,
                &backend.identity(),
                incarnation,
            );
            let at = store::format_millis(unix_millis());
            match containers::reconcile(&registry, &inventory, params.after.as_deref(), &at, limit)
            {
                Ok(outcome) => Response::ok(&request.id, json!({"reconciled": outcome})),
                Err(error) => registry_error(request, error),
            }
        }
        "spawn.close" => {
            let params: ChildCloseParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let close = registry::ChildCloseRequest {
                request_id: request.id.clone(),
                spawn_request_id: params.spawn_request_id,
                source: params.source,
                incarnation: params.incarnation,
                intent: params.intent,
            };
            if let Err(error) = close.validate() {
                return Response::refused(&request.id, &Refusal::new(Code::BadParams, error));
            }
            execute_child_close(&close, request, core, verification)
        }
        "spawn.list" => {
            let params: crate::control_plane::public_contracts::spawn_list::SpawnListParams =
                match decode_params(request) {
                    Ok(value) => value,
                    Err(response) => return response,
                };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_REGISTRY_PAGE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_REGISTRY_PAGE}"),
                        ),
                    );
                }
                _ => {}
            }
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            // One record beyond the page tells whether a continuation exists,
            // without reading the whole topology to page a bounded answer.
            let read = {
                let core = locked(core);
                let parent = match params.registration_id.as_deref() {
                    Some(registration_id) => {
                        match crate::control_plane::registry::presence::attachment(
                            &core.registry,
                            registration_id,
                        ) {
                            Ok(Some(attachment)) => Some(attachment.subject_id),
                            Ok(None) => {
                                return Response::refused(
                                    &request.id,
                                    &Refusal::new(
                                        Code::Refused,
                                        "this registration holds no subject",
                                    ),
                                );
                            }
                            Err(error) => return registry_error(request, error),
                        }
                    }
                    None => None,
                };
                let page = match parent.as_deref() {
                    Some(subject) => core.registry.spawn_edges_for_subject_after(
                        subject,
                        params.after.as_deref(),
                        limit + 1,
                    ),
                    None => core
                        .registry
                        .spawn_edges_after(params.after.as_deref(), limit + 1),
                };
                match page {
                    Ok(edges) => edges,
                    Err(error) => return registry_error(request, error),
                }
            };
            let has_more = read.len() > limit;
            let candidates: Vec<_> = read.into_iter().take(limit).collect();
            let candidates = match observe_spawn_exits(core, &candidates) {
                Ok(edges) => edges,
                Err(error) => return registry_error(request, error),
            };
            // One inventory serves the whole page, independently of the single
            // batch exit-status read above.
            let inventory = observed_inventory(core);
            let mut edges = Vec::new();
            let mut used = 0usize;
            for edge in &candidates {
                let entry = edge.topology(crate::control_plane::registry_read::location_evidence(
                    inventory.as_ref(),
                    edge,
                ));
                let size = serde_json::to_vec(&entry).map_or(usize::MAX, |bytes| bytes.len() + 1);
                if used + size > MAX_LIST_RESPONSE_BYTES {
                    if edges.is_empty() {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(
                                Code::Refused,
                                "one spawn edge exceeds the topology response byte bound",
                            ),
                        );
                    }
                    break;
                }
                used += size;
                edges.push(entry);
            }
            // The continuation follows the last key actually returned, so a page
            // cut short by the byte bound resumes without skipping an edge.
            let next = (edges.len() < candidates.len() || has_more).then(|| {
                edges
                    .last()
                    .expect("a continuation has a returned edge")
                    .edge
                    .request_id
                    .clone()
            });
            Response::ok(
                &request.id,
                json!(
                    crate::control_plane::public_contracts::spawn_list::SpawnListPage {
                        spawns: edges,
                        next,
                    }
                ),
            )
        }
        _ => Response::refused(
            &request.id,
            &Refusal::new(Code::UnknownMethod, "unknown registry method"),
        ),
    }
}
