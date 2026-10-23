use super::*;

/// Records an operation this daemon cannot serve, without executing it.
///
/// The record is the answer either way: a client asking for a primitive this
/// backend does not implement gets a refusal it can read, not silence and not a
/// plausible-looking effect.
pub(super) fn execute_unsupported(
    operation: ops::Operation,
    request: &Request,
    message: &str,
    target: Option<String>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    let now = store::now_ms();
    let requester = request.optional_string("requester").ok().flatten();
    let core = locked(core);
    match core.store.get(&request.id) {
        Ok(Some(record))
            if record.method == operation.as_str()
                && record.params.as_ref() == Some(&request.params) =>
        {
            return Response::ok(&request.id, json!({"request": public_record(record)}));
        }
        Ok(Some(_)) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, "request id already names different contents"),
            );
        }
        Ok(None) => {}
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
    match core.store.unresolved(target.as_deref(), now) {
        Ok(Some(held)) => {
            let message = format!(
                "request for this target is already in flight as {}",
                held.id
            );
            let record = RequestRecord::new(
                &request.id,
                operation.as_str(),
                requester.clone(),
                target.clone(),
                now,
            )
            .with_params(request.params.clone())
            .complete(
                RequestOutcome::Refused,
                Some(Category::IN_FLIGHT),
                Some(&message),
                Vec::new(),
                now,
            );
            if let Err(error) = core.store.create(&record) {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
            return Response::refused(&request.id, &Refusal::new(Code::Refused, message));
        }
        Ok(None) => {}
        Err(error) => {
            return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
        }
    }
    let record = RequestRecord::new(&request.id, operation.as_str(), requester, target, now)
        .with_params(request.params.clone())
        .complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(message),
            Vec::new(),
            now,
        );
    if let Err(error) = core.store.create(&record) {
        return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
    }
    Response::ok(&request.id, json!({"request": public_record(record)}))
}

/// Validates, records and dispatches one effectful operation without holding
/// the store lock over backend I/O.
pub(super) fn execute(
    operation: ops::Operation,
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    match operation {
        ops::Operation::Focus => execute_focus(request, core),
        ops::Operation::Close => execute_close(request, core),
        ops::Operation::Create => execute_create(request, core, verification),
        ops::Operation::CreateContainer => execute_container_create(request, core, verification),
        ops::Operation::DeleteContainer => execute_container_delete(request, core, verification),
        ops::Operation::ReloadContainer => execute_container_reload(request, core, verification),
        ops::Operation::Input => execute_input(request, core),
        ops::Operation::Report => execute_report(request, core),
        ops::Operation::Spawn => execute_spawn(request, core, verification),
        ops::Operation::Enroll => execute_enroll(request, core, verification),
        ops::Operation::Restart => execute_restart(request, core, verification),
        ops::Operation::Cancel | ops::Operation::Stop => {
            execute_subject_control(request, core, operation, verification)
        }
        ops::Operation::Abandon => execute_abandon(request, core),
    }
}

/// Ends one unresolved record's recovery obligation, releasing only the
/// administrative block it held.
///
/// The record is named in params rather than being the request's own target: the
/// record being abandoned is exactly the one holding that target, so admission
/// must not consult it. No backend is touched, no outcome is established, and no
/// effect is dispatched — every independent process and pane safety check is
/// unchanged and still refuses on its own evidence.
pub(super) fn execute_abandon(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        request_id: String,
    }
    let params: Params = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    if SessionUuid::parse(&params.request_id).is_none() {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`request_id` must be a canonical UUID"),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    // The record being released is the target, not the daemon: naming it keeps the
    // in-flight check per record, so releasing one record does not refuse releasing
    // another, and the abandonment's own record says what it acted on.
    let released = params.request_id.clone();
    // `lifecycle.abandon` needs no backend primitive — it releases one of the
    // daemon's own records. `admit` refuses every operation when no backend is
    // wired, and a registry-only daemon still owns the record an operator needs
    // released, so that one verdict is not a refusal here. Every other verdict is
    // answered as usual: a replay, a content conflict, an in-flight refusal, a
    // store error, or a wired backend that cannot serve the operation.
    let wired = {
        let core = locked(core);
        core.backend.is_some()
    };
    match admit(
        ops::Operation::Abandon,
        request,
        Some(&released),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message)
            if !wired && ops::Operation::Abandon.needs() == ops::Primitives::NONE =>
        {
            tracing::debug!(%message, %released, "abandoning a record with no backend wired");
        }
        Admission::Unsupported(message) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, message),
            );
        }
        Admission::Dispatch(_, _) => {}
    }
    let record = RequestRecord::new(
        &request.id,
        ops::Operation::Abandon.as_str(),
        requester,
        Some(released.clone()),
        now,
    )
    .with_params(request.params.clone());
    let abandoned = {
        let core = locked(core);
        core.store.abandon(&released, now)
    };
    // Nothing is dispatched, so a refusal here is a definite answer about the
    // record — there is no record, it was never claimed, or it is already settled.
    // A failed abandonment therefore settles with a definite outcome and never
    // leaves a record that blocks a later attempt.
    let done = match abandoned {
        Ok(abandoned) => record.complete(
            RequestOutcome::Completed,
            None,
            Some(&format!(
                "released the administrative block of `{}`; its effect remains unestablished",
                abandoned.id
            )),
            vec!["abandon".into()],
            store::now_ms(),
        ),
        Err(error) => record.complete(
            RequestOutcome::Refused,
            Some(Category::REFUSED),
            Some(&error),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// What the admission every effectful request shares decided.
pub(super) enum Admission {
    /// The request is already answered: a replay, a content conflict, an
    /// in-flight refusal, or a store error.
    Answered(Response),
    /// The operation cannot be served: no backend is wired, or the wired backend
    /// does not provide the primitive it needs. The message says which.
    Unsupported(String),
    /// Admitted and recorded as started: dispatch to this backend.
    Dispatch(Arc<dyn RuntimeProvider>, Arc<AtomicBool>),
}

/// The admission every effectful request shares: exact same-ID replay, the
/// same-ID-different-contents conflict, conflicting unresolved targets across
/// methods, the primitive the operation needs, and the Started record written
/// before any dispatch. Never holds the store lock over backend I/O.
///
/// A request this backend cannot serve returns [`Admission::Unsupported`] before
/// any record claims to be started, so an unimplemented primitive is never
/// recorded as having been dispatched.
pub(super) fn replay_existing_operation(
    operation: ops::Operation,
    request: &Request,
    target: Option<&str>,
    requester: &Option<String>,
    core: &Arc<Mutex<Core>>,
) -> Option<Response> {
    replay_existing_operation_inner(operation, request, target, true, requester, core)
}

pub(super) fn replay_existing_operation_contents(
    operation: ops::Operation,
    request: &Request,
    requester: &Option<String>,
    core: &Arc<Mutex<Core>>,
) -> Option<Response> {
    replay_existing_operation_inner(operation, request, None, false, requester, core)
}

pub(super) fn replay_existing_operation_inner(
    operation: ops::Operation,
    request: &Request,
    target: Option<&str>,
    check_target: bool,
    requester: &Option<String>,
    core: &Arc<Mutex<Core>>,
) -> Option<Response> {
    let core = locked(core);
    let record = match core.store.get(&request.id) {
        Ok(record) => record,
        Err(error) => {
            return Some(Response::refused(
                &request.id,
                &Refusal::new(Code::Internal, error),
            ));
        }
    }?;
    let same = record.method == operation.as_str()
        && (!check_target || record.target.as_deref() == target)
        && record.requester == *requester
        && record.params.as_ref() == Some(&request.params);
    if same {
        Some(Response::ok(
            &request.id,
            json!({ "request": public_record(record) }),
        ))
    } else {
        let message = if record.params.is_none() {
            "legacy record cannot establish content-equivalent replay"
        } else {
            "request id already names different contents"
        };
        Some(Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, message),
        ))
    }
}

pub(super) fn admit(
    operation: ops::Operation,
    request: &Request,
    target: Option<&str>,
    requester: &Option<String>,
    now: i64,
    core: &Arc<Mutex<Core>>,
) -> Admission {
    admit_with_aliases(operation, request, target, &[], requester, now, core)
}

pub(super) fn unresolved_for_targets(
    store: &store::Store,
    target: Option<&str>,
    aliases: &[String],
    now: i64,
) -> Result<Option<RequestRecord>, String> {
    if let Some(held) = store.unresolved(target, now)? {
        return Ok(Some(held));
    }
    for alias in aliases {
        if target == Some(alias.as_str()) {
            continue;
        }
        if let Some(held) = store.unresolved(Some(alias), now)? {
            return Ok(Some(held));
        }
    }
    Ok(None)
}

pub(super) fn admit_with_aliases(
    operation: ops::Operation,
    request: &Request,
    target: Option<&str>,
    target_aliases: &[String],
    requester: &Option<String>,
    now: i64,
    core: &Arc<Mutex<Core>>,
) -> Admission {
    let method = operation.as_str();
    let (held, backend, cancel) = {
        let core = locked(core);
        let existing = match core.store.get(&request.id) {
            Ok(record) => record,
            Err(error) => {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
        };
        if let Some(record) = &existing {
            let same = record.method == method
                && record.target.as_deref() == target
                && record.requester == *requester
                && record.params.as_ref() == Some(&request.params);
            if same {
                let mut result = json!({ "request": public_record(record.clone()) });
                // A replayed creation names the container its first answer named,
                // so a caller recovering from a lost answer reads the identity it
                // already made instead of searching the tree for it. No other
                // operation's answer carries a container, so a replay of one stays
                // the answer the first call gave.
                if let ops::Operation::CreateContainer = operation {
                    let container = containers::created_by(&core.registry, &request.id);
                    result["container"] = json!(container.map(|container| container.public()));
                }
                return Admission::Answered(Response::ok(&request.id, result));
            }
            let message = if record.params.is_none() {
                "legacy record cannot establish content-equivalent replay"
            } else {
                "request id already names different contents"
            };
            return Admission::Answered(Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, message),
            ));
        }
        let held = match unresolved_for_targets(&core.store, target, target_aliases, now) {
            Ok(held) => held,
            Err(error) => {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
        };
        let backend = core.backend.clone();
        let capable = match backend.as_ref() {
            Some(backend) => operation.ready_for(backend.primitives()).is_ok(),
            // An operation that needs no primitive is served from the daemon's own
            // records rather than by a backend, so a daemon with none still serves
            // it and records it. `lifecycle.abandon` is the only such operation.
            None => operation.needs() == ops::Primitives::NONE,
        };
        // Only a request this daemon can serve is recorded as started: an
        // operation no backend can serve leaves no claim that it ran.
        if held.is_none() && capable {
            let pending = RequestRecord::new(
                &request.id,
                method,
                requester.clone(),
                target.map(str::to_string),
                now,
            )
            .with_params(request.params.clone());
            if let Err(error) = core.store.create(&pending) {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
            let started = pending.claim().start();
            if let Err(error) = core.store.write(&started) {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
        }
        (held, backend, Arc::clone(&core.stopping))
    };
    if let Some(held) = held {
        let message = format!(
            "{method} for this target is already in flight as {}",
            held.id
        );
        let record = RequestRecord::new(
            &request.id,
            method,
            requester.clone(),
            target.map(str::to_string),
            now,
        )
        .with_params(request.params.clone())
        .complete(
            RequestOutcome::Refused,
            Some(Category::IN_FLIGHT),
            Some(&message),
            Vec::new(),
            now,
        );
        let core = locked(core);
        if let Err(error) = core.store.create(&record) {
            return Admission::Answered(Response::refused(
                &request.id,
                &Refusal::new(Code::Internal, error),
            ));
        }
        return Admission::Answered(Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, message),
        ));
    }
    match backend {
        None => Admission::Unsupported(operation.unavailable_message()),
        Some(backend) => match operation.ready_for(backend.primitives()) {
            Ok(()) => Admission::Dispatch(backend, cancel),
            Err(message) => Admission::Unsupported(message),
        },
    }
}
