use super::*;

/// Validates, records and dispatches focus.
pub(super) fn execute_focus(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let allowed: &[&str] = &["target", "target_kind", "requester"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "focus accepts only target, target_kind, and requester",
            ),
        );
    }
    let target = match request.required_string("target") {
        Ok(target) => target,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    if !valid_identifier(&target) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`target` is not a valid identifier"),
        );
    }
    let runtime_target = match request.params.get("target_kind").and_then(|v| v.as_str()) {
        Some("pane") => Target::Pane(target.clone()),
        Some("workspace") => Target::Workspace(target.clone()),
        _ => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`target_kind` must be `pane` or `workspace`",
                ),
            );
        }
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Focus;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let outcome = backend.focus_outcome(&runtime_target, &cancel);
    let done = match outcome {
        FocusOutcome::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            None,
            vec!["focus".into()],
            store::now_ms(),
        ),
        FocusOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        FocusOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// Validates, records and dispatches one guarded close.
///
/// The caller's frozen identity travels as one [`CloseRequest`], so the daemon
/// applies exactly the policy Radar's direct close applies — fresh inventory,
/// frozen-identity match, positive-unmanaged containment — and never a second
/// implementation of it. A location its owner manages is refused here; there is
/// no generic close route may signal it; managed close uses subject cancellation followed by spawn.close.
pub(super) fn execute_close(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let allowed: &[&str] = &["request", "requester"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "close accepts only request and requester"),
        );
    }
    let Some(frozen) = request.params.get("request") else {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`request` is required"),
        );
    };
    let close_request: CloseRequest = match serde_json::from_value(frozen.clone()) {
        Ok(close_request) => close_request,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a frozen close request: {error}"),
                ),
            );
        }
    };
    let target = close_request.target.id().to_string();
    if !valid_identifier(&target) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the close target is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Close;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    // A backend confirmation says the close was performed, not that the location
    // is gone: nothing here re-reads the post-state, so the record says so.
    let registrations = || locked(core).registry.public_registrations();
    let done = match lifecycle::close_unmanaged(
        backend.as_ref(),
        &close_request,
        &cancel,
        registrations,
    ) {
        DirectClose::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            Some("the runtime confirmed the close; its removal was not re-observed"),
            vec!["close".into()],
            store::now_ms(),
        ),
        DirectClose::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        DirectClose::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// The frozen normalized request a create or input carries under `request`,
/// or the refusal to answer with. `extra` names further params this method
/// accepts beside it.
/// The launch subjects created inside a place, from the daemon's own spawn edges.
///
/// Membership is the edge's recorded location, never a pane name guessed from a
/// report: an agent is inside the place the create that produced it named.
/// Why a deletion waits, when agents it could not stop are still inside.
///
/// Radar does not close a place over an agent that is still alive unless the
/// operator asks twice: the first deletion whose stop window left survivors is
/// refused with this message, and the repeat is authorised to close anyway. The
/// decision is here rather than inline so the branch can be exercised without a
/// live process inside the place.
pub(super) fn surviving_agents(
    alive: &[String],
    force: bool,
    target: &crate::CloseTarget,
) -> Option<String> {
    if alive.is_empty() || force {
        return None;
    }
    let mut held = alive.to_vec();
    held.sort();
    held.dedup();
    Some(format!(
        "{} agent(s) inside {} did not exit: {} — nothing was closed; delete again to close anyway",
        held.len(),
        target.description(),
        held.join(", ")
    ))
}

pub(super) fn subjects_inside(registry: &Registry, target: &crate::CloseTarget) -> Vec<String> {
    let inside = |location: &crate::control_plane::registry::RegistryLocation| match target {
        crate::CloseTarget::Pane(pane) => location.pane.as_deref() == Some(pane.as_str()),
        crate::CloseTarget::Tab(tab) => location.tab.as_deref() == Some(tab.as_str()),
        crate::CloseTarget::Workspace(workspace) => {
            location.workspace.as_deref() == Some(workspace.as_str())
        }
    };
    let mut subjects: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    while let Ok(edges) = registry.spawn_edges_after(cursor.as_deref(), MAX_PANE_OBSERVATIONS) {
        let end = edges.len() < MAX_PANE_OBSERVATIONS;
        cursor = edges.last().map(|edge| edge.request_id.clone());
        for edge in &edges {
            // An unbound edge may never have released a program, so it is no
            // execution to cancel.
            if edge.bound.is_none() {
                continue;
            }
            let (Some(subject_id), Some(location)) =
                (edge.claim_id.as_deref(), edge.location.as_ref())
            else {
                continue;
            };
            if inside(location) && !subjects.iter().any(|known| known == subject_id) {
                subjects.push(subject_id.to_string());
            }
        }
        if end {
            break;
        }
    }
    subjects
}

pub(super) fn frozen_params(
    request: &Request,
    extra: &[&str],
) -> Result<serde_json::Value, Response> {
    if request.params.as_object().is_none_or(|params| {
        params.keys().any(|key| {
            !matches!(key.as_str(), "request" | "requester") && !extra.contains(&key.as_str())
        })
    }) {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("{} accepts only request and requester", request.method),
            ),
        ));
    }
    request.params.get("request").cloned().ok_or_else(|| {
        Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`request` is required"),
        )
    })
}

/// One creation's result: the record to settle, and the container it recorded.
pub(super) struct Creation {
    pub(super) record: RequestRecord,
    pub(super) container: Option<containers::Container>,
}

/// The label a creation carries, bounded and free of control characters.
///
/// A label is Radar's own name for a container: it is recorded verbatim and shown
/// in the tree, and the backend is never told it. It is bounded here, at the wire
/// boundary, rather than truncated wherever it is displayed.
pub(super) fn creation_label(request: &Request) -> Result<Option<String>, Response> {
    let label = match request.optional_string("label") {
        Ok(label) => label,
        Err(refusal) => return Err(Response::refused(&request.id, &refusal)),
    };
    let Some(label) = label.filter(|label| !label.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("a container label is at most {MAX_LABEL_CHARS} characters"),
            ),
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "a container label cannot carry control characters",
            ),
        ));
    }
    Ok(Some(label))
}

/// Dispatches a primitive create: a location is made, and nothing is started in
/// it.
///
/// The record's target is the location the request changes — the pane being split,
/// or the workspace a new tab joins — so a competing mutation of that location
/// cannot run beside it. A new workspace changes nothing that exists, so it has no
/// target and shares only the unscoped mutations' lane.
pub(super) fn execute_create(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    match dispatch_creation(ops::Operation::Create, None, request, core, verification) {
        Ok(creation) => write_settled(request, creation.record, core),
        Err(response) => response,
    }
}

/// Dispatches a creation Radar records as one of its own containers.
///
/// The creation is the same primitive one; what this verb adds is that the
/// container, its label and its provenance are Radar's records, and that the
/// caller is answered with the container it made.
pub(super) fn execute_container_create(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let label = match creation_label(request) {
        Ok(label) => label,
        Err(response) => return response,
    };
    match dispatch_creation(
        ops::Operation::CreateContainer,
        label,
        request,
        core,
        verification,
    ) {
        Ok(creation) => write_created(request, creation, core),
        Err(response) => response,
    }
}

/// Writes a settled creation together with the container it recorded.
///
/// The container travels with the record so a caller acts on the identity it made
/// instead of joining a handle to whatever the tree shows next.
pub(super) fn write_created(
    request: &Request,
    creation: Creation,
    core: &Arc<Mutex<Core>>,
) -> Response {
    let core = locked(core);
    match core.store.write(&creation.record) {
        Ok(()) => Response::ok(
            &request.id,
            json!({
                "request": public_record(creation.record),
                "container": creation.container.map(|container| container.public()),
            }),
        ),
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// Validates, records and dispatches one creation, and records the container it
/// made.
///
/// `mux.create` asks a backend for a primitive and `topology.create` for the same
/// creation as Radar's own hierarchy verb, so the two share every step: the
/// request is validated here, the runtime's answer is turned into a container
/// record before the creation is reported, and only the operation a caller is
/// recorded under differs. A container is recorded for the identity the request
/// asked for and no other, and a runtime that names nothing records nothing.
pub(super) fn dispatch_creation(
    operation: ops::Operation,
    label: Option<String>,
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Result<Creation, Response> {
    // The primitive create carries the request alone; the hierarchy verb carries
    // the label its container is recorded under.
    let extra: &[&str] = match operation {
        ops::Operation::CreateContainer => &["label", "recipe"],
        _ => &[],
    };
    let frozen = frozen_params(request, extra)?;
    let create: CreateRequest = match serde_json::from_value(frozen) {
        Ok(create) => create,
        Err(error) => {
            return Err(Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a create request: {error}"),
                ),
            ));
        }
    };
    // The scope is named by the caller, so a create never lands wherever the
    // multiplexer happens to have focus.
    let target = match &create {
        CreateRequest::Workspace { .. } => None,
        CreateRequest::Tab { workspace_id, .. } => Some(workspace_id.clone()),
        CreateRequest::PaneSplit { pane_id, .. } => Some(pane_id.clone()),
    };
    if target
        .as_deref()
        .is_some_and(|target| !valid_identifier(target))
    {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the create target is not a valid identifier",
            ),
        ));
    }
    // The command this creation's slot runs, if it carries one, is parsed and
    // checked here: a recipe this backend cannot carry refuses before a pane
    // exists, exactly as a spawn is preflighted.
    let recipe = creation_recipe(request, &create)?;
    if let Some(recipe) = &recipe
        && let Err(refusal) = preflight_recipe(core, &create, recipe)
    {
        return Err(Response::refused(&request.id, &refusal));
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Err(Response::refused(&request.id, &refusal)),
    };
    let now = store::now_ms();
    let method = operation.as_str();
    let (backend, cancel) =
        match admit(operation, request, target.as_deref(), &requester, now, core) {
            Admission::Answered(response) => return Err(response),
            Admission::Unsupported(message) => {
                return Err(execute_unsupported(
                    operation, request, &message, target, core,
                ));
            }
            Admission::Dispatch(backend, cancel) => (backend, cancel),
        };
    let record = RequestRecord::new(&request.id, method, requester, target, now)
        .with_params(request.params.clone());
    // The incarnation serving this create is verified before it is dispatched, so
    // the container is bound to the generation that made it rather than to
    // whatever a later server puts at the same handle.
    let verifier = Arc::clone(&locked(core).verifier);
    let incarnation = serving_identity(&backend, &verifier, &cancel, verification);
    // Creation is confirmed, not re-observed: the runtime says it made the
    // location, nothing here lists to prove it exists.
    let done = match backend.create(&create, &cancel) {
        // The identity the answer named travels as data, so a client acts on what
        // it created without reading the effect prose.
        CreateOutcome::Completed(created) => {
            // The container is recorded before the creation is reported, so a
            // caller never reads an owned location that no record names. A failed
            // append is reported rather than hidden: the effect happened, and the
            // caller must not be told the topology is recorded when it is not.
            let now = store::now_ms();
            let registry = {
                let core = locked(core);
                Arc::clone(&core.registry)
            };
            let backend_identity = backend.identity();
            let id = store::random_uuid();
            let mut notes: Vec<String> = Vec::new();
            // A recipe is held privately before the record that says so is
            // written, so no record claims a command the daemon does not have. It
            // is held only for a create that named a pane: a recipe runs in a pane
            // slot, and a create that named nothing has no slot to run it in.
            let held = match (recipe.as_ref(), created.pane_id.as_deref()) {
                (Some(recipe), Some(_)) => match containers::write_recipe(&registry, &id, recipe) {
                    Ok(()) => true,
                    Err(error) => {
                        notes.push(format!("the recipe was not held: {error}"));
                        false
                    }
                },
                (Some(_), None) => {
                    notes.push("the runtime named no pane, so the recipe was not held".to_string());
                    false
                }
                (None, _) => false,
            };
            let recording = containers::record_created(
                &registry,
                containers::Confirmed {
                    id,
                    recipe: held,
                    request: &create,
                    created: &created,
                    backend: &backend_identity,
                    incarnation: incarnation.clone(),
                    label,
                    created_by: Some(request.id.clone()),
                },
                &store::format_millis(now),
            );
            let (container, _) = match (recording, created.is_empty()) {
                (Err(error), _) => {
                    notes.push(format!("the created container was not recorded: {error}"));
                    (None, false)
                }
                (Ok(_), true) => {
                    notes.push(
                        "the runtime confirmed the creation without naming what it created"
                            .to_owned(),
                    );
                    (None, true)
                }
                (Ok(mut recorded), false) => (recorded.pop(), true),
            };
            // The recipe runs in the pane the create named, launched by the daemon
            // outside any spawn: the child carries no spawn token, and this launch
            // is what a later reload requires to be gone before running it again.
            let launched = match (held, created.pane_id.as_deref()) {
                (true, Some(pane_id)) => recipe.as_ref().map(|recipe| {
                    backend.launch(
                        &LaunchRequest {
                            pane_id: pane_id.to_string(),
                            executable: recipe.executable.clone(),
                            argv: recipe.argv.clone(),
                            spawn_token: None,
                            cwd: recipe.cwd.clone(),
                            env: recipe.env.clone(),
                        },
                        &cancel,
                    )
                }),
                _ => None,
            };
            let mut effects = created.effects();
            let (outcome, category) = match &launched {
                Some(LaunchOutcome::Completed) => {
                    effects.push("launch".into());
                    (RequestOutcome::Completed, None)
                }
                Some(LaunchOutcome::Refused(message)) => {
                    notes.push(format!("the recipe was not launched: {message}"));
                    (RequestOutcome::Refused, Some(Category::BACKEND_REFUSED))
                }
                Some(LaunchOutcome::Unknown(message)) => {
                    notes.push(format!(
                        "the recipe launch is unknown and is not retried: {message}"
                    ));
                    (RequestOutcome::Unknown, Some(Category::BACKEND_UNAVAILABLE))
                }
                None => (RequestOutcome::Completed, None),
            };
            let note = (!notes.is_empty()).then(|| notes.join("; "));
            return Ok(Creation {
                record: record
                    .created(created.identity(create.created_kind()))
                    .complete(outcome, category, note.as_deref(), effects, now),
                container,
            });
        }
        CreateOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        CreateOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    Ok(Creation {
        record: done,
        container: None,
    })
}

/// Deletes one container Radar records: the hierarchy's own verb for removing a
/// workspace, a tab or a pane slot.
///
/// The container is what is deleted and the frozen target is what the operator
/// confirmed, so a container replaced in the meantime is not the one acted on.
/// The close is the same guarded dispatch a generic close runs — positively
/// unmanaged, still the frozen identity, unregistered — and what a deletion would
/// take with it is read from the daemon's own records before anything is
/// dispatched. Pane removal is recorded as a removal; no process is claimed to
/// have exited, no child to have been unregistered, and no assignment to have
/// completed.
pub(super) fn execute_container_delete(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let delete: containers::DeleteRequest = match serde_json::from_value(frozen) {
        Ok(delete) => delete,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a container deletion: {error}"),
                ),
            );
        }
    };
    if !valid_identifier(&delete.container_id) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the container id is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::DeleteContainer;
    let method = operation.as_str();
    let target = delete.container_id.clone();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    // Everything below refuses into the record: a confirmed deletion is readable
    // by its request id whatever it decided.
    let registry = {
        let core = locked(core);
        Arc::clone(&core.registry)
    };
    let container = match containers::read(&registry, &delete.container_id) {
        Ok(Some(container)) => container,
        Ok(None) => {
            return write_refused(
                request,
                record,
                &format!(
                    "container {} is not recorded: nothing was deleted",
                    delete.container_id
                ),
                core,
            );
        }
        Err(error) => return write_refused(request, record, &error, core),
    };
    // A container this daemon already removed is history, and one whose binding
    // names no verified serving instance cannot be fenced: neither is deleted.
    if let Some(at) = container.removed_at.as_deref() {
        return write_refused(
            request,
            record,
            &format!(
                "container {} was removed at {at}: nothing was deleted",
                container.id
            ),
            core,
        );
    }
    let Some(binding) = container.binding.clone() else {
        return write_refused(
            request,
            record,
            &format!(
                "container {} records no binding: nothing was deleted",
                container.id
            ),
            core,
        );
    };
    let verifier = Arc::clone(&locked(core).verifier);
    let serving = serving_identity(&backend, &verifier, &cancel, verification);
    // A container whose place cannot be verified is only ever forgotten: there is
    // nothing to close, so the deletion is its records going. A place that is
    // still served is never forgotten around, because that would orphan it.
    let Some(frozen) = delete.frozen.clone() else {
        let forgotten = match (binding.incarnation.as_ref(), serving.as_ref()) {
            (Some(recorded), Some(serving)) => recorded != serving,
            (Some(_), None) => false,
            (None, _) => true,
        };
        if !forgotten {
            return write_refused(
                request,
                record,
                &format!(
                    "container {} is still served by its place: nothing was deleted",
                    container.id
                ),
                core,
            );
        }
        let members = match containers::subtree(&registry, &container.id) {
            Ok(members) => members,
            Err(message) => return write_refused(request, record, &message, core),
        };
        let now = store::now_ms();
        let at = store::format_millis(now);
        let mut removed: Vec<containers::Container> = Vec::new();
        for id in &members {
            match containers::record_removed(&registry, id, &at) {
                Ok(container) => removed.push(container),
                Err(error) => return write_refused(request, record, &error, core),
            }
        }
        let settled = record.complete(
            RequestOutcome::Completed,
            None,
            Some(&format!(
                "no place was recorded for this container; {} record(s) forgotten",
                removed.len()
            )),
            vec!["container.removal".into()],
            now,
        );
        return write_deleted(request, settled, removed, core);
    };
    let expected = container.kind.target(&binding.handle);
    if frozen.target != expected {
        return write_refused(
            request,
            record,
            &format!(
                "{} is not container {}'s location ({}): nothing was deleted",
                frozen.target.description(),
                container.id,
                expected.description()
            ),
            core,
        );
    }
    let Some(recorded) = binding.incarnation.clone() else {
        return write_refused(
            request,
            record,
            &format!(
                "container {} records no verified serving instance: nothing was deleted",
                container.id
            ),
            core,
        );
    };
    // What the deletion would take with it, from the daemon's own records: the
    // container, everything recorded inside it, and any worker registered into
    // one of those places.
    let members = match containers::subtree(&registry, &container.id) {
        Ok(members) => members,
        Err(message) => return write_refused(request, record, &message, core),
    };
    // The recorded serving instance is gone when the server now serving is a
    // different process, or none is. The places recorded under it went with it:
    // there is nothing left to close, so no runtime confirmation is awaited and
    // no process is claimed to have exited. The records are forgotten instead.
    if serving.as_ref() != Some(&recorded) {
        let now = store::now_ms();
        let at = store::format_millis(now);
        let mut removed: Vec<containers::Container> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        for id in &members {
            match containers::record_removed(&registry, id, &at) {
                Ok(container) => removed.push(container),
                Err(error) => failures.push(format!("{id}: {error}")),
            }
        }
        let note = if failures.is_empty() {
            format!(
                "{} was recorded under a serving instance that no longer serves; {} container record(s) forgotten, nothing was closed",
                frozen.target.description(),
                removed.len()
            )
        } else {
            format!(
                "{} was recorded under a serving instance that no longer serves; removals were not recorded for: {}",
                frozen.target.description(),
                failures.join("; ")
            )
        };
        let settled = record.complete(
            RequestOutcome::Completed,
            None,
            Some(&note),
            vec!["container.removal".into()],
            now,
        );
        return write_deleted(request, settled, removed, core);
    }
    // Every agent launched inside this place ends the way a single `D` on its own
    // row ends it: fenced, SIGTERM, one shared window for the exit, and the exit
    // recorded. Radar does not close the place over an agent that is still alive
    // unless the operator asks twice.
    let inside = subjects_inside(&registry, &frozen.target);
    let mut stopped = Vec::new();
    // Agents left alive by the window. On a repeat deletion the place closes
    // anyway, and the outcome says which exits were never verified.
    let mut alive: Vec<String> = Vec::new();
    if !inside.is_empty() {
        let verifier = Arc::clone(&locked(core).verifier);
        let cancelled = crate::control_plane::lifecycle::cancel_many(
            &registry,
            &inside,
            verifier.as_ref(),
            store::now_ms(),
        );
        stopped = cancelled.stopped;
        alive = cancelled.survivors;
        if let Some(message) = surviving_agents(&alive, delete.force, &frozen.target) {
            // The daemon records that it asked: a repeat that names `force` is
            // recognised from this record, not from what a client remembers.
            let at = store::format_millis(store::now_ms());
            if let Err(error) = containers::record_held(&registry, &container.id, &at) {
                return write_refused(request, record, &error, core);
            }
            return write_refused_with(
                request,
                record,
                store::Category::AGENTS_ALIVE,
                &message,
                core,
            );
        }
    }
    let settled = match lifecycle::close_for_deletion(backend.as_ref(), &frozen, &cancel) {
        DirectClose::Completed => {
            let now = store::now_ms();
            let at = store::format_millis(now);
            let mut removed: Vec<containers::Container> = Vec::new();
            let mut failures: Vec<String> = Vec::new();
            for id in &members {
                match containers::record_removed(&registry, id, &at) {
                    Ok(container) => removed.push(container),
                    Err(error) => failures.push(format!("{id}: {error}")),
                }
            }
            // The close is the runtime's confirmation that it removed the place,
            // not a re-observation of it, and the removals are the daemon's own
            // records: neither claims anything about a process.
            let ended = match (stopped.len(), alive.len()) {
                (0, 0) => String::new(),
                (count, 0) => format!(" after {count} agent(s) exited"),
                (count, alive) => format!(
                    " after {count} agent(s) exited; {alive} agent(s) did not, and their exits are unverified"
                ),
            };
            let note = if failures.is_empty() {
                format!(
                    "{} was closed by the runtime{ended}; {} container record(s) marked removed",
                    frozen.target.description(),
                    removed.len()
                )
            } else {
                format!(
                    "{} was closed by the runtime; removals were not recorded for: {}",
                    frozen.target.description(),
                    failures.join("; ")
                )
            };
            let settled = record.complete(
                RequestOutcome::Completed,
                None,
                Some(&note),
                vec!["close".into(), "container.removal".into()],
                now,
            );
            return write_deleted(request, settled, removed, core);
        }
        DirectClose::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        DirectClose::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_deleted(request, settled, Vec::new(), core)
}

/// Reloads one pane container's slot: the recipe it was created with runs again
/// in the pane it was created in.
///
/// Reload requires the previous command to be gone — the pane's own retained exit
/// evidence, read now — so nothing is replaced while it may still be running. It
/// records its own outcome and claims no session restart, no new container and no
/// assignment fact: it is one slot's command run again.
pub(super) fn execute_container_reload(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let reload: containers::ReloadRequest = match serde_json::from_value(frozen) {
        Ok(reload) => reload,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a container reload: {error}"),
                ),
            );
        }
    };
    if !valid_identifier(&reload.container_id) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the container id is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::ReloadContainer;
    let method = operation.as_str();
    let target = reload.container_id.clone();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let registry = {
        let core = locked(core);
        Arc::clone(&core.registry)
    };
    let container = match containers::read(&registry, &reload.container_id) {
        Ok(Some(container)) => container,
        Ok(None) => {
            return reload_refused(
                request,
                record,
                &format!(
                    "container {} is not recorded: nothing was reloaded",
                    reload.container_id
                ),
                None,
                core,
            );
        }
        Err(error) => return reload_refused(request, record, &error, None, core),
    };
    // Everything a reload requires is decided here, and every refusal names what
    // is missing: only a pane slot runs a recipe, only a container Radar created
    // has one, and the recipe itself must still be held.
    let eligible = (|| -> Result<(containers::Binding, containers::Recipe), String> {
        if container.kind != containers::Kind::Pane {
            return Err("only a pane slot runs a recipe".to_string());
        }
        if container.provenance != containers::Provenance::Created {
            return Err(
                "this container is not one Radar created: it has no recipe to run".to_string(),
            );
        }
        if let Some(at) = container.removed_at.as_deref() {
            return Err(format!("container {} was removed at {at}", container.id));
        }
        let binding = container
            .binding
            .clone()
            .ok_or_else(|| format!("container {} records no binding", container.id))?;
        let verifier = Arc::clone(&locked(core).verifier);
        let serving = serving_identity(&backend, &verifier, &cancel, verification);
        let Some(recorded) = binding.incarnation.clone() else {
            return Err(format!(
                "container {} records no verified serving instance",
                container.id
            ));
        };
        if serving.as_ref() != Some(&recorded) {
            return Err(format!(
                "container {} was recorded under another serving instance",
                container.id
            ));
        }
        if !container.recipe {
            return Err(format!("container {} records no recipe", container.id));
        }
        // The record says a recipe is held and the private store has none: the two
        // disagree, and a command is never invented to run.
        let recipe = containers::read_recipe(&registry, &container.id)?
            .ok_or_else(|| format!("the recipe held for container {} is missing", container.id))?;
        Ok((binding, recipe))
    })();
    let (binding, recipe) = match eligible {
        Ok(ready) => ready,
        Err(message) => {
            return reload_refused(
                request,
                record,
                &format!("{message}: nothing was reloaded"),
                Some(container),
                core,
            );
        }
    };
    // The pane's own retained exit is the evidence that the previous command is
    // gone. A command still running, or one whose exit cannot be established, is
    // not replaced: the operator stops it first.
    let pane = binding.handle.clone();
    let observed = backend.exit_observations(std::slice::from_ref(&pane), &cancel);
    match observed.get(&pane) {
        Some(PaneExitObservation::Exited { .. }) => {}
        Some(PaneExitObservation::Running) => {
            return reload_refused(
                request,
                record,
                &format!("pane {pane} is still running its command: stop it first, then reload"),
                Some(container),
                core,
            );
        }
        _ => {
            return reload_refused(
                request,
                record,
                &format!(
                    "pane {pane} does not report an exited command: not replacing what may still be running"
                ),
                Some(container),
                core,
            );
        }
    }
    let launched = backend.launch(
        &LaunchRequest {
            pane_id: pane.clone(),
            executable: recipe.executable.clone(),
            argv: recipe.argv.clone(),
            // A container's own launch is not a spawn child: no token, and the
            // variable is not put in its environment.
            spawn_token: None,
            cwd: recipe.cwd.clone(),
            env: recipe.env.clone(),
        },
        &cancel,
    );
    let settled = match launched {
        LaunchOutcome::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            Some(&format!(
                "the recorded recipe was run again in {pane}; the container is unchanged"
            )),
            vec!["launch".into()],
            store::now_ms(),
        ),
        LaunchOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        LaunchOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_reloaded(request, settled, Some(container), core)
}

/// Answers an admitted reload with a refusal that is recorded before it is
/// written.
pub(super) fn reload_refused(
    request: &Request,
    record: RequestRecord,
    message: &str,
    container: Option<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_reloaded(
        request,
        record.complete(
            RequestOutcome::Refused,
            Some(Category::REFUSED),
            Some(message),
            Vec::new(),
            store::now_ms(),
        ),
        container,
        core,
    )
}

/// Answers a settled reload with the container it acted on, unchanged.
pub(super) fn write_reloaded(
    request: &Request,
    record: RequestRecord,
    container: Option<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_settled_with(
        request,
        record,
        json!({
            "container": container.map(|container| container.public()),
        }),
        core,
    )
}

/// Writes a settled operation with the public facts its own answer names beside
/// the record, so a caller acts on what its request did rather than on what the
/// next read happens to show.
pub(super) fn write_settled_with(
    request: &Request,
    record: RequestRecord,
    extra: serde_json::Value,
    core: &Arc<Mutex<Core>>,
) -> Response {
    let core = locked(core);
    match core.store.write(&record) {
        Ok(()) => {
            let mut body = json!({ "request": public_record(record) });
            if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    body.insert(key.clone(), value.clone());
                }
            }
            Response::ok(&request.id, body)
        }
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// Answers an admitted deletion with a refusal that is recorded before it is
/// written, so a caller reads back what the daemon decided.
pub(super) fn write_refused(
    request: &Request,
    record: RequestRecord,
    message: &str,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_refused_with(request, record, Category::REFUSED, message, core)
}

/// A refusal filed under its own category, for the callers whose refusal the UI
/// acts on rather than only reads.
pub(super) fn write_refused_with(
    request: &Request,
    record: RequestRecord,
    category: &'static str,
    message: &str,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_deleted(
        request,
        record.complete(
            RequestOutcome::Refused,
            Some(category),
            Some(message),
            Vec::new(),
            store::now_ms(),
        ),
        Vec::new(),
        core,
    )
}

/// Writes a settled deletion together with the containers it marked removed.
///
/// The containers travel with the record so a caller reads what its own request
/// removed rather than joining handles to whatever the tree shows next.
pub(super) fn write_deleted(
    request: &Request,
    record: RequestRecord,
    removed: Vec<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_settled_with(
        request,
        record,
        json!({
            "removed": removed
                .into_iter()
                .map(|container| container.public())
                .collect::<Vec<_>>(),
        }),
        core,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deletion_over_a_surviving_agent_waits_only_until_it_is_forced() {
        let target = crate::CloseTarget::Workspace("wA".into());
        let alive = vec!["b".to_string(), "a".to_string(), "b".to_string()];
        let held = surviving_agents(&alive, false, &target).expect("the first deletion waits");
        assert_eq!(
            held,
            "2 agent(s) inside workspace wA did not exit: a, b — nothing was closed; delete again to close anyway",
            "the message counts distinct survivors in a stable order"
        );
        assert_eq!(
            surviving_agents(&alive, true, &target),
            None,
            "the repeat the operator authorised closes anyway"
        );
        assert_eq!(
            surviving_agents(&[], false, &target),
            None,
            "nothing survived, so nothing waits"
        );
    }
}
