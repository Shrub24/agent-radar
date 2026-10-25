use super::*;

/// Revalidate the exact managed child against fresh pane and process evidence,
/// persist the request only after the checks succeed, then dispatch one pane
/// close.
///
/// A live occupant is closed only after its birth identity is verified. A pane
/// whose command has already exited has no such occupant to protect, so a failed
/// live verification is retried against dead-pane evidence and closed as
/// `already_exited`; without that evidence the live refusal stands. Neither
/// outcome says the registered process exited.
pub(super) fn execute_child_close(
    close: &registry::ChildCloseRequest,
    request: &Request,
    core: &Arc<Mutex<Core>>,
    budget: &Arc<VerificationBudget>,
) -> Response {
    let (registry, backend, verifier, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            core.backend.clone(),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    // A recorded close is answered from its own record before anything is
    // re-verified or dispatched: a replay repeats the one outcome this request id
    // reached, even when the pane it closed is no longer there to verify.
    if let Some(existing) = match registry.child_close(&close.request_id) {
        Ok(existing) => existing,
        Err(error) => return registry_error(request, error),
    } {
        if !existing.answers(close) {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Refused,
                    "spawn.close request id already names different contents",
                ),
            );
        }
        return close_response(request, &existing);
    }
    let location = match registry.validate_child_close_target(close) {
        Ok(location) => location,
        Err(error) => return registry_error(request, error),
    };
    let pane_id = location.pane.as_deref().expect("validated pane target");
    let registration = match registry.child_registration(&close.source, &close.incarnation) {
        Ok(Some(registration)) => registration,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "bound child registration is not recorded"),
            );
        }
        Err(error) => return registry_error(request, error),
    };
    let Some(claim) = registration.request.process.clone() else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "bound child registration has no process identity claim",
            ),
        );
    };
    let Some(backend) = backend else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                "spawn.close requires a mux backend",
            ),
        );
    };
    if stopping.load(Ordering::SeqCst) {
        return Response::refused(&request.id, &Refusal::new(Code::Busy, "daemon is stopping"));
    }
    if let Some(refusal) = primitive_refuse(&backend, protocol::OBSERVE_METHOD) {
        return Response::refused(&request.id, &refusal);
    }
    let inventory = match bounded_inventory(
        &backend,
        &stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(inventory) => inventory,
        Err(_) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BackendUnavailable,
                    "fresh mux inventory is unavailable",
                ),
            );
        }
    };
    let Some(pane) = inventory.pane(pane_id) else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "recorded child pane is absent from fresh inventory",
            ),
        );
    };
    if location
        .workspace
        .as_deref()
        .is_some_and(|id| id != pane.location.workspace_id)
        || location
            .tab
            .as_deref()
            .is_some_and(|id| id != pane.location.tab_id)
    {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, "recorded child pane containment changed"),
        );
    }
    let already_dead =
        match verify_live_occupant(&backend, &verifier, pane_id, &claim, &stopping, budget) {
            Ok(()) => None,
            Err(refusal) => {
                match dead_pane_evidence(
                    &registry, &backend, &verifier, close, pane_id, &stopping, budget,
                ) {
                    DeadPane::Cleanup(exit) => Some(exit),
                    DeadPane::Unresolved => return Response::refused(&request.id, &refusal),
                    DeadPane::Unfenced(fence) => {
                        return Response::refused(&request.id, &fence);
                    }
                }
            }
        };
    // A close backend is required before recording admission; otherwise this
    // request cannot reach a truthful dispatch result.
    if !backend.primitives().contains(Primitives::CLOSE) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                "spawn.close needs the `mux.close` capability; this backend does not provide it"
                    .to_string(),
            ),
        );
    }
    if stopping.load(Ordering::SeqCst) {
        return Response::refused(&request.id, &Refusal::new(Code::Busy, "daemon is stopping"));
    }
    // Admission decides the one dispatch this request id may make: a caller that
    // raced this one for the same id dispatches nothing, and reads the outcome
    // recorded here instead.
    match registry.record_child_close(close, store::now_ms()) {
        Ok(CloseAdmission::Admitted(_)) => {}
        Ok(CloseAdmission::Recorded(recorded)) => return close_response(request, &recorded),
        Err(error) => return registry_error(request, error),
    }

    let result = match bounded_close_dispatch(
        &backend,
        pane_id,
        &stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(crate::runtime::CloseOutcome::Completed) => match already_dead {
            Some(exit) => CloseResult::AlreadyExited(exit),
            None => CloseResult::Completed,
        },
        Ok(crate::runtime::CloseOutcome::Refused(message)) => CloseResult::Refused(message),
        Ok(crate::runtime::CloseOutcome::Unknown(message)) => CloseResult::Unknown(Some(message)),
        // The dispatch may have reached the backend and its answer was lost.
        Err(()) => CloseResult::Unknown(None),
    };
    // The outcome is durable before this call answers: the effect is claimed only
    // from what the backend said, and a dispatch that said nothing leaves the
    // record unknown for every later replay to read.
    let recorded = match registry.record_child_close_outcome(&close.request_id, result) {
        Ok(recorded) => recorded,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Internal,
                    format!("the child close outcome was not recorded: {error}"),
                ),
            );
        }
    };
    close_response(request, &recorded)
}

/// Closes the pane of a caller's own child, named by subject.
///
/// Radar resolves what the caller does not hold: the launch edge that bound the
/// child's current process, and from it the exact child the close verifies. The
/// target is then closed by `execute_child_close`, so a subject close is never
/// looser than a spawn close. A retry resolves to the target its first
/// admission recorded, even when the subject has since been relaunched.
pub(super) fn subject_child_close(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    use crate::control_plane::public_contracts::subject::ChildCloseParams;
    let params: ChildCloseParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let registry = Arc::clone(&locked(core).registry);
    let record = match super::subject_reads::child_record(
        &registry,
        &params.registration_id,
        &params.subject_id,
    ) {
        Ok(record) => record,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let close = match registry.child_close(&request.id) {
        Ok(Some(recorded)) => registry::ChildCloseRequest {
            request_id: request.id.clone(),
            spawn_request_id: recorded.spawn_request_id,
            source: recorded.source,
            incarnation: recorded.incarnation,
            intent: params.intent,
        },
        Ok(None) => {
            let Some(parent) = record.managed.parent_subject_id.as_deref() else {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::Refused, "subject has no recorded parent"),
                );
            };
            match current_edge(&registry, parent, &params.subject_id) {
                Ok(Some(edge)) => {
                    let bound = edge.bound.expect("a current edge is bound");
                    registry::ChildCloseRequest {
                        request_id: request.id.clone(),
                        spawn_request_id: edge.request_id,
                        source: bound.source,
                        incarnation: bound.incarnation,
                        intent: params.intent,
                    }
                }
                Ok(None) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::Refused, "child has no launch to close"),
                    );
                }
                Err(error) => return registry_error(request, error),
            }
        }
        Err(error) => return registry_error(request, error),
    };
    if let Err(error) = close.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, error));
    }
    execute_child_close(&close, request, core, verification)
}

/// The newest bound, unwithdrawn launch of one child subject.
fn current_edge(
    registry: &Registry,
    parent_subject: &str,
    subject: &str,
) -> Result<Option<registry::SpawnEdge>, String> {
    let mut newest: Option<registry::SpawnEdge> = None;
    let mut after: Option<String> = None;
    loop {
        let page = registry.spawn_edges_for_subject_after(parent_subject, after.as_deref(), 100)?;
        let last = page.last().map(|edge| edge.request_id.clone());
        for edge in page.iter() {
            let Some(bound) = edge.bound.as_ref() else {
                continue;
            };
            if bound.subject_id.as_deref() != Some(subject) || edge.withdrawn.is_some() {
                continue;
            }
            let later = newest.as_ref().is_none_or(|current| {
                current.bound.as_ref().map(|b| b.bound_at.as_str()) < Some(bound.bound_at.as_str())
            });
            if later {
                newest = Some(edge.clone());
            }
        }
        match last {
            Some(last) if page.len() >= 100 => after = Some(last),
            _ => return Ok(newest),
        }
    }
}

/// The daemon's answer for one recorded close.
///
/// The durable outcome decides the shape, so a replay — including one after a
/// restart — answers as the original call did. A confirmed close reports the mux
/// pane closure and nothing else: no answer here claims a process or process
/// group exit, and an unknown outcome stays unknown rather than being retried.
pub(super) fn close_response(request: &Request, close: &registry::ChildClose) -> Response {
    match close.outcome {
        CloseOutcome::Completed => Response::ok(
            &request.id,
            json!({"close": {
                "outcome": "completed",
                "intent": close.intent,
                "pane": close.location.pane.as_deref(),
            }}),
        ),
        CloseOutcome::AlreadyExited => Response::ok(
            &request.id,
            json!({"close": {
                "outcome": "already_exited",
                "intent": close.intent,
                "pane": close.location.pane.as_deref(),
                "exit_observation": close.exit.clone().map(|exit| {
                    registry::PublicSpawnExitObservation::Exited {
                        code: exit.code,
                        signal: exit.signal,
                        observed_at: exit.observed_at,
                    }
                }),
            }}),
        ),
        CloseOutcome::Refused => Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                close
                    .message
                    .as_deref()
                    .unwrap_or("the backend refused the close"),
            ),
        ),
        CloseOutcome::Unknown => Response::refused(
            &request.id,
            // A close that may have been dispatched keeps that uncertainty on the
            // wire: `outcome_unknown` is the code a caller reads as "an effect
            // may exist", so it is never reported as a pre-dispatch refusal.
            &Refusal::new(
                Code::OutcomeUnknown,
                close
                    .message
                    .as_deref()
                    .unwrap_or("managed child close outcome is unknown"),
            ),
        ),
    }
}
