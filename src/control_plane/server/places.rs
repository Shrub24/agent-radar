use super::*;

/// Validates, records and dispatches focus.
pub(super) fn execute_focus(request: &Request, core: &Arc<Mutex<Core>>) -> Reply {
    let allowed: &[&str] = &["target", "target_kind", "requester"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Err(Refusal::new(
            Code::BadParams,
            "focus accepts only target, target_kind, and requester",
        ));
    }
    let target = request.required_string("target")?;
    if !valid_identifier(&target) {
        return Err(Refusal::new(
            Code::BadParams,
            "`target` is not a valid identifier",
        ));
    }
    let runtime_target = match request.params.get("target_kind").and_then(|v| v.as_str()) {
        Some("pane") => Target::Pane(target.clone()),
        Some("workspace") => Target::Workspace(target.clone()),
        _ => {
            return Err(Refusal::new(
                Code::BadParams,
                "`target_kind` must be `pane` or `workspace`",
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    let now = store::now_ms();
    let operation = ops::Operation::Focus;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return *response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let outcome = backend.focus_outcome(&runtime_target, &cancel);
    let done = match outcome {
        FocusOutcome::Completed => {
            record.complete(Settlement::completed(vec!["focus".into()]), store::now_ms())
        }
        FocusOutcome::NoClient => record.complete(
            Settlement::backend_refused(crate::runtime::NO_CLIENT),
            store::now_ms(),
        ),
        FocusOutcome::Refused(message) => {
            record.complete(Settlement::backend_refused(&message), store::now_ms())
        }
        FocusOutcome::Unknown(message) => {
            record.complete(Settlement::backend_unknown(&message), store::now_ms())
        }
    };
    write_settled(done, core)
}

/// Validates, records and dispatches one guarded close.
///
/// The caller's frozen identity travels as one [`CloseRequest`]: the daemon
/// re-reads the inventory, refuses a place that changed since it was frozen, and
/// ends every execution placed inside it before removing the place.
pub(super) fn execute_close(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    let allowed: &[&str] = &["request", "requester"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Err(Refusal::new(
            Code::BadParams,
            "close accepts only request and requester",
        ));
    }
    let Some(frozen) = request.params.get("request") else {
        return Err(Refusal::new(Code::BadParams, "`request` is required"));
    };
    let close_request: CloseRequest = match serde_json::from_value(frozen.clone()) {
        Ok(close_request) => close_request,
        Err(error) => {
            return Err(Refusal::new(
                Code::BadParams,
                format!("`request` is not a frozen close request: {error}"),
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    run_close(request, close_request, requester, core, verification)
}

/// The identity this daemon can compare against its own backend inventory. That
/// inventory carries no registry joins, so a named execution's occupant cannot be
/// compared here; the claim and placement checks fence it instead.
fn backend_comparable(request: &CloseRequest) -> lifecycle::TargetIdentity {
    let mut identity = request.identity.clone();
    if let (Some(_), lifecycle::TargetIdentity::Pane(pane)) = (&request.execution, &mut identity) {
        pane.occupant = None;
    }
    identity
}

pub(super) fn run_close(
    request: &Request,
    close_request: CloseRequest,
    requester: Option<String>,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    let target = close_request.target.id().to_string();
    if !valid_identifier(&target) {
        return Err(Refusal::new(
            Code::BadParams,
            "the close target is not a valid identifier",
        ));
    }
    let now = store::now_ms();
    let operation = ops::Operation::Close;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return *response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let inventory = match bounded_inventory(
        &backend,
        &cancel,
        verification,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(inventory) => inventory,
        Err(()) => {
            return write_settled(
                record.complete(
                    Settlement::backend_refused("fresh inventory is unavailable"),
                    store::now_ms(),
                ),
                core,
            );
        }
    };
    if !lifecycle::matches(&inventory, &backend_comparable(&close_request)) {
        return write_settled(
            record.complete(
                Settlement::backend_refused("the selected place changed: nothing was closed"),
                store::now_ms(),
            ),
            core,
        );
    }
    if let Some(expected) = &close_request.execution {
        let registry = Arc::clone(&locked(core).registry);
        let valid = subjects::read_subject_record(&registry, &expected.subject_id)
            .ok().flatten().is_some_and(|subject| {
                subject.current_claim.as_deref().or(subject.last_claim.as_deref()) == Some(expected.claim_id.as_str())
            }) && subjects::read(&registry, &expected.claim_id).ok().flatten().is_some_and(|claim| {
                claim.subject_id == expected.subject_id && claim.process.placement.as_ref().is_some_and(|placement| {
                    matches!(&close_request.target, crate::CloseTarget::Pane(pane) if placement.location.pane.as_deref() == Some(pane.as_str()))
                })
            });
        if !valid {
            return write_refused(
                record,
                "the selected execution changed: nothing was closed",
                core,
            );
        }
    }
    let done = match close_place(core, &backend, &close_request, &cancel, verification) {
        Ok(closed) => record.complete(close_settlement(closed), store::now_ms()),
        Err(reason) => record.complete(Settlement::backend_refused(reason), store::now_ms()),
    };
    write_settled(done, core)
}

/// The executions running inside a place, as their own records place them there.
///
/// Membership is a verified placement — the pane an execution recorded, with the
/// instance that made it — never a pane guessed from a report and never a spawn
/// edge. An execution admitted in place (`radar run --agent`) has no spawn edge at
/// all, and it still runs inside the place: a branch closed over spawn edges alone
/// would leave it running.
fn executions_inside(
    registry: &Registry,
    target: &crate::CloseTarget,
    instance: &crate::model::ProcessIdentity,
    inventory: &FleetObservation,
) -> Vec<crate::control_plane::lifecycle::Placed> {
    let inside = |location: &crate::control_plane::registry::RegistryLocation| {
        let Some(pane) = location.pane.as_deref().and_then(|id| inventory.pane(id)) else {
            return false;
        };
        match target {
            crate::CloseTarget::Pane(id) => pane.location.pane_id == *id,
            crate::CloseTarget::Tab(id) => pane.location.tab_id == *id,
            crate::CloseTarget::Workspace(id) => pane.location.workspace_id == *id,
        }
    };
    let mut placed: Vec<crate::control_plane::lifecycle::Placed> = Vec::new();
    for execution in crate::control_plane::lifecycle::placements(registry) {
        if execution.placement.instance.as_ref() != Some(instance)
            || !inside(&execution.placement.location)
        {
            continue;
        }

        placed.push(execution);
    }
    placed
}

pub(super) fn close_place(
    core: &Arc<Mutex<Core>>,
    backend: &Arc<dyn RuntimeProvider>,
    frozen: &CloseRequest,
    cancel: &Arc<AtomicBool>,
    verification: &Arc<VerificationBudget>,
) -> Result<subjects::ClosedExecution, String> {
    use subjects::ClosedExecution;
    let (registry, verifier, closing) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            Arc::clone(&core.closing),
        )
    };
    let target = &frozen.target;
    // The place is held before it is read: an admission between this read and the
    // close would otherwise be accepted into a branch already being torn down.
    let mut held = vec![closing.begin(std::iter::once(target_handle(target)))];
    let inventory = bounded_inventory(
        backend,
        cancel,
        verification,
        Instant::now() + VERIFICATION_TIMEOUT,
    )
    .map_err(|()| "fresh inventory is unavailable".to_string())?;
    let instance = serving_identity(backend, &verifier, cancel, verification);
    let panes: Vec<_> = inventory
        .panes
        .iter()
        .filter(|pane| match target {
            crate::CloseTarget::Pane(id) => pane.location.pane_id == *id,
            crate::CloseTarget::Tab(id) => pane.location.tab_id == *id,
            crate::CloseTarget::Workspace(id) => pane.location.workspace_id == *id,
        })
        .map(|pane| pane.location.pane_id.clone())
        .collect();
    held.push(closing.begin(panes));
    let inside = instance
        .as_ref()
        .map(|instance| executions_inside(&registry, target, instance, &inventory))
        .unwrap_or_default();
    let targets: Vec<subjects::ExecutionRef> = inside
        .iter()
        .map(|placed| subjects::ExecutionRef {
            registration_id: placed.registration_id.clone(),
            subject_id: placed.subject_id.clone(),
        })
        .collect();
    let ended = subjects::cancel_many(&registry, &targets, verifier.as_ref(), store::now_ms());
    let keys: Vec<_> = targets
        .iter()
        .map(|target| target.registration_id.clone())
        .collect();
    subjects::await_completions(&registry, &keys);
    let resource = remove_place(
        core,
        backend,
        frozen,
        cancel,
        verification,
        instance.as_ref(),
    );
    Ok(ClosedExecution {
        executions: ended.ended,
        resource: Some(resource),
        escalated: ended.escalated,
    })
}

pub(super) fn remove_place(
    core: &Arc<Mutex<Core>>,
    backend: &Arc<dyn RuntimeProvider>,
    frozen: &CloseRequest,
    cancel: &Arc<AtomicBool>,
    verification: &Arc<VerificationBudget>,
    instance: Option<&crate::model::ProcessIdentity>,
) -> subjects::ResourceEnd {
    use subjects::ResourceEnd;
    let verifier = Arc::clone(&locked(core).verifier);
    if instance.is_some()
        && serving_identity(backend, &verifier, cancel, verification).as_ref() != instance
    {
        return ResourceEnd::Kept("the serving generation changed during shutdown".into());
    }
    let inventory = match bounded_inventory(
        backend,
        cancel,
        verification,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(inventory) => inventory,
        Err(()) => return ResourceEnd::Kept("fresh inventory is unavailable".into()),
    };
    let present = match &frozen.target {
        crate::CloseTarget::Pane(id) => inventory.pane(id).is_some(),
        crate::CloseTarget::Tab(id) => inventory.tabs.iter().any(|tab| tab.tab_id == *id),
        crate::CloseTarget::Workspace(id) => inventory
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == *id),
    };
    if !present {
        return ResourceEnd::Absent;
    }
    match bounded_close_dispatch(
        backend,
        &frozen.target,
        cancel,
        verification,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(crate::runtime::CloseOutcome::Completed) => ResourceEnd::Removed,
        Ok(crate::runtime::CloseOutcome::Refused(reason)) => ResourceEnd::Kept(reason),
        Ok(crate::runtime::CloseOutcome::Unknown(reason)) => ResourceEnd::Unverified(reason),
        Err(()) => ResourceEnd::Unverified("the removal reply was lost".into()),
    }
}

pub(super) fn close_settlement(closed: subjects::ClosedExecution) -> Settlement {
    use subjects::ResourceEnd;
    match &closed.resource {
        Some(ResourceEnd::Kept(reason)) => Settlement::backend_refused(reason),
        Some(ResourceEnd::Unverified(reason)) => Settlement::backend_unknown(reason),
        _ => Settlement::completed(Vec::new()),
    }
    .closed(closed)
}

/// The handle a launch would name for this target.
fn target_handle(target: &crate::runtime::CloseTarget) -> String {
    match target {
        crate::runtime::CloseTarget::Workspace(handle)
        | crate::runtime::CloseTarget::Tab(handle)
        | crate::runtime::CloseTarget::Pane(handle) => handle.clone(),
    }
}

pub(super) fn frozen_params(
    request: &Request,
    extra: &[&str],
) -> Result<serde_json::Value, Refusal> {
    if request.params.as_object().is_none_or(|params| {
        params.keys().any(|key| {
            !matches!(key.as_str(), "request" | "requester") && !extra.contains(&key.as_str())
        })
    }) {
        return Err(Refusal::new(
            Code::BadParams,
            format!("{} accepts only request and requester", request.method),
        ));
    }
    request
        .params
        .get("request")
        .cloned()
        .ok_or_else(|| Refusal::new(Code::BadParams, "`request` is required"))
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
fn validate_label(label: Option<String>) -> Result<Option<String>, Refusal> {
    let Some(label) = label.filter(|label| !label.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(Refusal::new(
            Code::BadParams,
            format!("a container label is at most {MAX_LABEL_CHARS} characters"),
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(Refusal::new(
            Code::BadParams,
            "a container label cannot carry control characters",
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
) -> Reply {
    match dispatch_creation(ops::Operation::Create, None, request, core, verification) {
        Ok(creation) => write_settled(creation.record, core),
        Err(reply) => *reply,
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
) -> Reply {
    let label = request.optional_string("label")?;
    match dispatch_creation(
        ops::Operation::CreateContainer,
        label,
        request,
        core,
        verification,
    ) {
        Ok(creation) => write_created(creation, core),
        Err(reply) => *reply,
    }
}

/// Writes a settled creation together with the container it recorded.
///
/// The container travels with the record so a caller acts on the identity it made
/// instead of joining a handle to whatever the tree shows next.
pub(super) fn write_created(creation: Creation, core: &Arc<Mutex<Core>>) -> Reply {
    write_settled_with(creation.record, core, |answer| {
        answer.with_container(creation.container.map(|container| container.public()))
    })
}

/// A refusal as the early exit of a creation dispatch, which otherwise yields
/// the creation it made.
fn boxed(refusal: Refusal) -> Box<Reply> {
    Box::new(Err(refusal))
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
) -> Result<Creation, Box<Reply>> {
    // The primitive create carries the request alone; the hierarchy verb carries
    // the label its container is recorded under.
    let extra: &[&str] = match operation {
        ops::Operation::CreateContainer => &["label", "recipe"],
        _ => &[],
    };
    let frozen = frozen_params(request, extra).map_err(boxed)?;
    let create: CreateRequest = match serde_json::from_value(frozen) {
        Ok(create) => create,
        Err(error) => {
            return Err(boxed(Refusal::new(
                Code::BadParams,
                format!("`request` is not a create request: {error}"),
            )));
        }
    };

    let recipe = creation_recipe(request, &create).map_err(boxed)?;
    let requester = request.optional_string("requester").map_err(boxed)?;
    run_creation(
        operation,
        request,
        CreateInput {
            create,
            label,
            recipe,
            requester,
        },
        core,
        verification,
    )
}

pub(super) struct CreateInput {
    pub(super) create: CreateRequest,
    pub(super) label: Option<String>,
    pub(super) recipe: Option<containers::Recipe>,
    pub(super) requester: Option<String>,
}

pub(super) fn run_creation(
    operation: ops::Operation,
    request: &Request,
    input: CreateInput,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Result<Creation, Box<Reply>> {
    let CreateInput {
        create,
        label,
        recipe,
        requester,
    } = input;
    let label = validate_label(label).map_err(boxed)?;
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
        return Err(boxed(Refusal::new(
            Code::BadParams,
            "the create target is not a valid identifier",
        )));
    }
    if let Some(target) = target.as_deref() {
        super::closing::refuse_if_closing(core, target).map_err(boxed)?;
    }
    // The command this creation's slot runs, if it carries one, is parsed and
    // checked here: a recipe this backend cannot carry refuses before a pane
    // exists, exactly as a spawn is preflighted.
    if recipe.is_some() && matches!(create, CreateRequest::Workspace { .. }) {
        return Err(boxed(Refusal::new(
            Code::BadParams,
            "a recipe runs in a pane slot: a pane split or a tab creation carries one",
        )));
    }
    // What the caller asked for, or the daemon's own choice when it said nothing.
    // A create dispatched unfocused is followed by a focus of what it made, so a
    // backend never moves the operator as a side effect of creating. A workspace
    // is exempt: tmux focuses one by switching a client, which a create cannot
    // do, so the follow-up focus is the only place that can.
    let focus = match &create {
        CreateRequest::Workspace { .. } => false,
        CreateRequest::Tab { focus, .. } | CreateRequest::PaneSplit { focus, .. } => {
            focus.unwrap_or_else(|| locked(core).config.focus_created)
        }
    };
    if let Some(recipe) = &recipe
        && let Err(refusal) = preflight_recipe(core, &create, recipe)
    {
        return Err(boxed(refusal));
    }
    let now = store::now_ms();
    let method = operation.as_str();
    let (backend, cancel) =
        match admit(operation, request, target.as_deref(), &requester, now, core) {
            Admission::Answered(reply) => return Err(reply),
            Admission::Unsupported(message) => {
                return Err(Box::new(execute_unsupported(
                    operation, request, &message, target, core,
                )));
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
    let tracking = match recipe
        .as_ref()
        .map(|recipe| launch::prepare_recipe(core, &request.id, recipe))
        .transpose()
    {
        Ok(tracking) => tracking.flatten(),
        Err(error) => {
            return Ok(Creation {
                record: record.complete(Settlement::backend_refused(error), store::now_ms()),
                container: None,
            });
        }
    };
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
                (true, Some(_)) => recipe.as_ref().map(|recipe| {
                    launch::release_recipe(
                        core,
                        &backend,
                        recipe,
                        subjects::Placement {
                            location: created_location(&created, &backend_identity),
                            instance: incarnation.clone(),
                        },
                        tracking.as_ref(),
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
                    // A refused launch runs no program, so its reservation is
                    // abandoned here rather than left for a program that will
                    // never arrive.
                    abandon_reservation(core, tracking.as_ref());
                    notes.push(format!("the recipe was not launched: {message}"));
                    (RequestOutcome::Refused, Some(Category::BackendRefused))
                }
                Some(LaunchOutcome::Unknown(message)) => {
                    notes.push(format!(
                        "the recipe launch is unknown and is not retried: {message}"
                    ));
                    (RequestOutcome::Unknown, Some(Category::BackendUnavailable))
                }
                None => (RequestOutcome::Completed, None),
            };
            // A successful creation is followed by a focus of the location it
            // named. The creation has already happened, so a focus that cannot
            // move a client is reported and never changes the creation's outcome
            // or repeats it.
            if focus && matches!(outcome, RequestOutcome::Completed) {
                match created.pane_id.as_deref() {
                    Some(pane) => {
                        match backend.focus_outcome(&Target::Pane(pane.to_string()), &cancel) {
                            FocusOutcome::Completed => {}
                            FocusOutcome::NoClient => {
                                notes.push(format!(
                                    "the created pane was not focused: {}",
                                    crate::runtime::NO_CLIENT
                                ));
                            }
                            FocusOutcome::Refused(message) => {
                                notes.push(format!("the created pane was not focused: {message}"));
                            }
                            FocusOutcome::Unknown(message) => {
                                notes.push(format!(
                                    "the focus of the created pane is unknown: {message}"
                                ));
                            }
                        }
                    }
                    None => notes.push("the creation named no pane to focus".to_string()),
                }
            }
            let note = (!notes.is_empty()).then(|| notes.join("; "));
            return Ok(Creation {
                record: record
                    .created(created.identity(create.created_kind()))
                    .complete(
                        Settlement::new(outcome, category, note.as_deref(), effects),
                        now,
                    ),
                container,
            });
        }
        CreateOutcome::Refused(message) => {
            record.complete(Settlement::backend_refused(&message), store::now_ms())
        }
        CreateOutcome::Unknown(message) => {
            record.complete(Settlement::backend_unknown(&message), store::now_ms())
        }
    };
    // Nothing will present for a create the runtime refused or did not confirm.
    abandon_reservation(core, tracking.as_ref());
    Ok(Creation {
        record: done,
        container: None,
    })
}

/// Abandons a reservation that will never present, so no subject row waits on it.
/// A launch the runtime accepted is left alone: its program claims it asynchronously.
fn abandon_reservation(
    core: &Arc<Mutex<Core>>,
    tracking: Option<&(subjects::Bootstrap, subjects::Ticket)>,
) {
    let Some((_, ticket)) = tracking else {
        return;
    };
    let registry = Arc::clone(&locked(core).registry);
    if let Err(error) = subjects::revoke(&registry, &ticket.claim_id, store::now_ms()) {
        tracing::warn!(%error, claim = %ticket.claim_id, "the abandoned reservation was not revoked");
    }
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
) -> Reply {
    let frozen = frozen_params(request, &[])?;
    let delete: containers::DeleteRequest = match serde_json::from_value(frozen) {
        Ok(delete) => delete,
        Err(error) => {
            return Err(Refusal::new(
                Code::BadParams,
                format!("`request` is not a container deletion: {error}"),
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    run_container_delete(request, delete, requester, core, verification)
}

pub(super) fn run_container_delete(
    request: &Request,
    delete: containers::DeleteRequest,
    requester: Option<String>,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    if !valid_identifier(&delete.container_id) {
        return Err(Refusal::new(
            Code::BadParams,
            "the container id is not a valid identifier",
        ));
    }
    let now = store::now_ms();
    let operation = ops::Operation::DeleteContainer;
    let method = operation.as_str();
    let target = delete.container_id.clone();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return *response,
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
                record,
                &format!(
                    "container {} is not recorded: nothing was deleted",
                    delete.container_id
                ),
                core,
            );
        }
        Err(error) => return write_refused(record, &error, core),
    };
    // A container this daemon already removed is history, and one whose binding
    // names no verified serving instance cannot be fenced: neither is deleted.
    if let Some(at) = container.removed_at.as_deref() {
        return write_refused(
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
            Err(message) => return write_refused(record, &message, core),
        };
        let now = store::now_ms();
        let at = store::format_millis(now);
        let mut removed: Vec<containers::Container> = Vec::new();
        for id in &members {
            match containers::record_removed(&registry, id, &at) {
                Ok(container) => removed.push(container),
                Err(error) => return write_refused(record, &error, core),
            }
        }
        let settled = record.complete(
            Settlement::new(
                RequestOutcome::Completed,
                None,
                Some(&format!(
                    "no place was recorded for this container; {} record(s) forgotten",
                    removed.len()
                )),
                vec!["container.removal".into()],
            ),
            now,
        );
        return write_deleted(settled, removed, core);
    };
    let expected = container.kind.target(&binding.handle);
    if frozen.target != expected {
        return write_refused(
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
        Err(message) => return write_refused(record, &message, core),
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
            Settlement::new(
                RequestOutcome::Completed,
                None,
                Some(&note),
                vec!["container.removal".into()],
            ),
            now,
        );
        return write_deleted(settled, removed, core);
    }
    let closed = match close_place(core, &backend, &frozen, &cancel, verification) {
        Ok(closed) => closed,
        Err(reason) => return write_refused(record, &reason, core),
    };
    let settled = match &closed.resource {
        Some(subjects::ResourceEnd::Kept(reason)) => record.complete(
            Settlement::backend_refused(reason).closed(closed),
            store::now_ms(),
        ),
        Some(subjects::ResourceEnd::Unverified(reason)) => record.complete(
            Settlement::backend_unknown(reason).closed(closed),
            store::now_ms(),
        ),
        _ => {
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
            // records: neither claims anything about a process. What became of
            // each execution is the close report's own answer.
            let note = if failures.is_empty() {
                format!(
                    "{} was closed by the runtime; {} container record(s) marked removed",
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
                Settlement::new(
                    RequestOutcome::Completed,
                    None,
                    Some(&note),
                    vec!["close".into(), "container.removal".into()],
                )
                .closed(closed.clone()),
                now,
            );
            return write_deleted(settled, removed, core);
        }
    };
    write_deleted(settled, Vec::new(), core)
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
) -> Reply {
    let frozen = frozen_params(request, &[])?;
    let reload: containers::ReloadRequest = match serde_json::from_value(frozen) {
        Ok(reload) => reload,
        Err(error) => {
            return Err(Refusal::new(
                Code::BadParams,
                format!("`request` is not a container reload: {error}"),
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    run_container_reload(request, reload, requester, core, verification)
}

pub(super) fn run_container_reload(
    request: &Request,
    reload: containers::ReloadRequest,
    requester: Option<String>,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    if !valid_identifier(&reload.container_id) {
        return Err(Refusal::new(
            Code::BadParams,
            "the container id is not a valid identifier",
        ));
    }
    let now = store::now_ms();
    let operation = ops::Operation::ReloadContainer;
    let method = operation.as_str();
    let target = reload.container_id.clone();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return *response,
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
                record,
                &format!(
                    "container {} is not recorded: nothing was reloaded",
                    reload.container_id
                ),
                None,
                core,
            );
        }
        Err(error) => return reload_refused(record, &error, None, core),
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
                record,
                &format!("pane {pane} is still running its command: stop it first, then reload"),
                Some(container),
                core,
            );
        }
        _ => {
            return reload_refused(
                record,
                &format!(
                    "pane {pane} does not report an exited command: not replacing what may still be running"
                ),
                Some(container),
                core,
            );
        }
    }
    let tracking = match launch::prepare_recipe(core, &request.id, &recipe) {
        Ok(tracking) => tracking,
        Err(reason) => return reload_refused(record, &reason, Some(container), core),
    };
    let location = registry::RegistryLocation {
        backend: binding.backend.clone(),
        instance: backend.identity().instance,
        workspace: None,
        tab: None,
        pane: Some(pane.clone()),
    };
    let launched = launch::release_recipe(
        core,
        &backend,
        &recipe,
        subjects::Placement {
            location,
            instance: binding.incarnation.clone(),
        },
        tracking.as_ref(),
        &cancel,
    );
    let settled = match launched {
        LaunchOutcome::Completed => record.complete(
            Settlement::new(
                RequestOutcome::Completed,
                None,
                Some(&format!(
                    "the recorded recipe was run again in {pane}; the container is unchanged"
                )),
                vec!["launch".into()],
            ),
            store::now_ms(),
        ),
        LaunchOutcome::Refused(message) => {
            abandon_reservation(core, tracking.as_ref());
            record.complete(Settlement::backend_refused(&message), store::now_ms())
        }
        LaunchOutcome::Unknown(message) => {
            record.complete(Settlement::backend_unknown(&message), store::now_ms())
        }
    };
    write_reloaded(settled, Some(container), core)
}

/// Answers an admitted reload with a refusal that is recorded before it is
/// written.
pub(super) fn reload_refused(
    record: RequestRecord,
    message: &str,
    container: Option<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Reply {
    write_reloaded(
        record.complete(
            Settlement::refused(Category::Refused, message),
            store::now_ms(),
        ),
        container,
        core,
    )
}

/// Answers a settled reload with the container it acted on, unchanged.
pub(super) fn write_reloaded(
    record: RequestRecord,
    container: Option<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Reply {
    write_settled_with(record, core, |answer| {
        answer.with_container(container.map(|container| container.public()))
    })
}

/// Answers an admitted deletion with a refusal that is recorded before it is
/// written, so a caller reads back what the daemon decided.
pub(super) fn write_refused(
    record: RequestRecord,
    message: &str,
    core: &Arc<Mutex<Core>>,
) -> Reply {
    write_deleted(
        record.complete(
            Settlement::refused(Category::Refused, message),
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
    record: RequestRecord,
    removed: Vec<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Reply {
    write_settled_with(record, core, |answer| {
        answer.with_removed(
            removed
                .into_iter()
                .map(|container| container.public())
                .collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{AgentIdentity, PaneIdentity, TargetIdentity};

    fn request(execution: bool) -> CloseRequest {
        CloseRequest {
            target: crate::CloseTarget::Pane("p".into()),
            execution: execution.then(|| crate::control_plane::command::Subject {
                subject_id: "s".into(),
                claim_id: "c".into(),
            }),
            identity: TargetIdentity::Pane(PaneIdentity {
                pane_id: "p".into(),
                occupant: Some(AgentIdentity {
                    name: None,
                    session: None,
                    lineage: None,
                    managed: true,
                    label: Some("agent".into()),
                    run: None,
                }),
            }),
        }
    }

    #[test]
    fn a_named_execution_is_not_compared_by_occupant_but_a_bare_pane_is() {
        let TargetIdentity::Pane(named) = backend_comparable(&request(true)) else {
            panic!("a pane stays a pane");
        };
        assert!(
            named.occupant.is_none(),
            "the claim fences a named execution"
        );
        let TargetIdentity::Pane(bare) = backend_comparable(&request(false)) else {
            panic!("a pane stays a pane");
        };
        assert!(
            bare.occupant.is_some(),
            "drift in a bare pane stays refused"
        );
    }
}
