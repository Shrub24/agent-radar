use super::*;

/// The wire shape of one spawn request: the parent runtime subject, and the
/// command the caller resolved for the child.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpawnParams {
    pub(super) parent: String,
    pub(super) parent_session_id: String,
    pub(super) executable: String,
    #[serde(default)]
    pub(super) argv: Vec<String>,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    #[serde(default)]
    pub(super) env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub(super) managed: subjects::ManagedMetadata,
    /// Start this child agent again instead of a new one.
    #[serde(default)]
    pub(super) continue_subject_id: Option<String>,
}

/// Asks the selected backend whether it can carry this launch, before any record
/// is written.
pub(super) fn preflight_launch(
    core: &Arc<Mutex<Core>>,
    params: &SpawnParams,
) -> Result<(), Refusal> {
    preflight_command(
        core,
        "preflight",
        &params.executable,
        &params.argv,
        params.cwd.as_deref(),
        &params.env,
    )
}

/// Asks the selected backend whether it can carry one command in one pane, before
/// any record is written. The probe names no token: what is checked is the command
/// and the context it needs.
pub(super) fn preflight_command(
    core: &Arc<Mutex<Core>>,
    pane_id: &str,
    executable: &str,
    argv: &[String],
    cwd: Option<&str>,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<(), Refusal> {
    for name in [
        subjects::MANAGED_SUBJECT_ENV,
        subjects::MANAGED_LAUNCH_ENV,
        subjects::MANAGED_GENERATION_ENV,
        subjects::MANAGED_PARENT_SUBJECT_ENV,
    ] {
        if env.contains_key(name) {
            return Err(Refusal::new(
                Code::BadParams,
                format!("launch env cannot set Radar-issued identity variable {name}"),
            ));
        }
    }
    let launch_context = LaunchRequest {
        pane_id: pane_id.to_string(),
        executable: executable.to_string(),
        argv: argv.to_vec(),
        spawn_token: None,
        cwd: cwd.map(str::to_string),
        env: env.clone(),
    };
    let preflight = {
        let core = locked(core);
        core.backend.as_ref().map(|backend| {
            if (!env.is_empty() || cwd.is_some())
                && !backend.primitives().contains(Primitives::LAUNCH_CONTEXT)
            {
                Err("selected backend does not support launch context".to_string())
            } else {
                backend.validate_launch(&launch_context)
            }
        })
    };
    match preflight {
        Some(Err(message)) => Err(Refusal::new(Code::BadParams, message)),
        None if cwd.is_some() || !env.is_empty() => Err(Refusal::new(
            Code::BackendUnavailable,
            "selected backend does not support launch context",
        )),
        _ => Ok(()),
    }
}

/// The recipe a creation carries: the command its pane slot runs.
///
/// A recipe is held privately and reported only as held. It is refused on a
/// creation that makes no pane of its own, because a recipe runs in a pane slot:
/// a pane split runs it in the pane it split from, and a tab runs it in the pane
/// the tab opened. A workspace creation carries none: its pane is the session's
/// own first slot, which this operation does not name.
pub(super) fn creation_recipe(
    request: &Request,
    create: &CreateRequest,
) -> Result<Option<containers::Recipe>, Response> {
    let Some(recipe) = request.params.get("recipe") else {
        return Ok(None);
    };
    if !matches!(
        create,
        CreateRequest::PaneSplit { .. } | CreateRequest::Tab { .. }
    ) {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "a recipe runs in a pane slot: a pane split or a tab creation carries one",
            ),
        ));
    }
    serde_json::from_value(recipe.clone())
        .map(Some)
        .map_err(|error| {
            Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`recipe` is not a container recipe: {error}"),
                ),
            )
        })
}

/// Asks the selected backend whether it can carry a container's recipe, before
/// anything is created.
pub(super) fn preflight_recipe(
    core: &Arc<Mutex<Core>>,
    create: &CreateRequest,
    recipe: &containers::Recipe,
) -> Result<(), Refusal> {
    // A pane split names the pane it splits from, so the command is checked
    // against it. A tab's pane does not exist yet, so the probe names a synthetic
    // one, exactly as a spawn's preflight does: what is checked here is the command
    // and the context it needs, and both backends validate the address inside
    // `launch`, against the pane the creation actually reported.
    let pane_id = match create {
        CreateRequest::PaneSplit { pane_id, .. } => pane_id.as_str(),
        CreateRequest::Tab { .. } => "preflight",
        _ => {
            return Err(Refusal::new(
                Code::BadParams,
                "a recipe runs in a pane slot: a pane split or a tab creation carries one",
            ));
        }
    };
    preflight_command(
        core,
        pane_id,
        &recipe.executable,
        &recipe.argv,
        recipe.cwd.as_deref(),
        &recipe.env,
    )
}

/// Validates, records and dispatches one managed child spawn.
///
/// One operation, and four durable facts in order: the intent with its private
/// token, the pane under the parent's own registered pane, the command typed
/// into it, and the request's record. Each is written before the next is
/// attempted, so a spawn that stops half-way leaves what it established readable
/// rather than an unattributable pane; nothing is cleaned up on a later failure,
/// because an operator can see and close a pane and cannot see one deleted
/// silently. No assignment fact and no process lifecycle is touched: creating a
/// child says nothing about keeping it alive.
pub(super) fn execute_spawn(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let params: SpawnParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`request` is not a valid spawn request".to_string(),
                ),
            );
        }
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    if let Some(response) =
        replay_existing_operation_contents(ops::Operation::Spawn, request, &requester, core)
    {
        return spawn_answer(request, response, core);
    }
    // The command and the parent are validated at the wire boundary, before
    // anything is recorded: a request this seam cannot carry creates no pane.
    if let Err(message) = validate_command(&params.executable, &params.argv) {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    if let Err(refusal) = preflight_launch(core, &params) {
        return Response::refused(&request.id, &refusal);
    }
    let intent = registry::SpawnRequest {
        request_id: request.id.clone(),
        parent: params.parent.clone(),
        parent_session_id: params.parent_session_id.clone(),
        parent_subject_id: params.managed.parent_subject_id.clone(),
    };
    if let Err(message) = intent.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let tracks_launches = locked(core).backend.as_ref().is_some_and(|backend| {
        backend
            .primitives()
            .contains(Primitives::LIFECYCLE_TRACKING)
    });
    if !tracks_launches && params.managed != subjects::ManagedMetadata::default() {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "managed launch metadata requires daemon lifecycle tracking",
            ),
        );
    }
    if tracks_launches
        && let Err(message) =
            subjects::check_bounds_with_metadata(&requested_launch(&params), &params.managed)
    {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    // The child is created beside the pane its parent registered: a spawn never
    // lands wherever the multiplexer happens to have focus, and a parent with no
    // recorded pane is refused rather than guessed from a title or a label.
    let parent_pane = {
        let core = locked(core);
        let parent = match core.registry.get(&params.parent) {
            Ok(Some(parent)) => parent,
            Ok(None) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::NotFound,
                        format!("parent {} is not a registered agent", params.parent),
                    ),
                );
            }
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        };
        let context = match core.registry.context(&params.parent, store::now_ms()) {
            Ok(Some(context)) => context,
            Ok(None) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::Refused, "parent has no published session context"),
                );
            }
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        };
        if context.context.freshness != registry::Freshness::Fresh
            || context.context.session.as_deref() != Some(&params.parent_session_id)
        {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Refused,
                    "parent session context is absent, stale or mismatched",
                ),
            );
        }
        match parent
            .request
            .location
            .as_ref()
            .and_then(|location| location.pane.clone())
        {
            Some(pane) => pane,
            None => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::Refused,
                        format!(
                            "parent {} names no pane to create the child under",
                            params.parent
                        ),
                    ),
                );
            }
        }
    };
    let (params, lineage) = match continuation(core, params) {
        Ok(continued) => continued,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    spawn_child(
        request,
        core,
        verification,
        &params,
        &intent,
        parent_pane,
        lineage,
    )
}

/// What a spawn runs as: a new agent, or its own released child started again in
/// the session the daemon recorded for it.
///
/// Refused before any effect unless the subject is the caller's child, has a
/// session, and has no live claim. Only the daemon's record names the session, and
/// the caller's argv has any session selector replaced; the same rule restart uses.
pub(super) fn continuation(
    core: &Arc<Mutex<Core>>,
    params: SpawnParams,
) -> Result<(SpawnParams, SpawnLineage), Refusal> {
    let Some(subject_id) = params.continue_subject_id.clone() else {
        return Ok((params, SpawnLineage::default()));
    };
    let refuse = |message: &str| Refusal::new(Code::Refused, message.to_string());
    let registry = Arc::clone(&locked(core).registry);
    let internal = |error: String| Refusal::new(Code::Internal, error);
    let record = subjects::read_subject_record(&registry, &subject_id)
        .map_err(|error| Refusal::new(Code::BadParams, error))?
        .ok_or_else(|| Refusal::new(Code::NotFound, "the subject to continue is not recorded"))?;
    let (Some(session), Some(last)) = (record.session.as_deref(), record.last_claim.as_deref())
    else {
        return Err(refuse("the subject has no recorded session to continue"));
    };
    if record.current_claim.is_some() {
        return Err(refuse("the subject is attached; nothing was started"));
    }
    let previous = subjects::read(&registry, last)
        .map_err(internal)?
        .ok_or_else(|| refuse("the subject's last attachment is unavailable"))?;
    if previous.managed.parent_subject_id.is_none()
        || previous.managed.parent_subject_id != params.managed.parent_subject_id
    {
        return Err(refuse("the subject is not this parent's child"));
    }
    let launch = subjects::resume_launch(&requested_launch(&params), session)
        .map_err(|message| refuse(&message))?;
    let lineage = SpawnLineage {
        session: Some(session.to_string()),
        identity: Some((subject_id, last.to_string())),
        prepared: None,
        in_place: false,
    };
    Ok((
        SpawnParams {
            executable: launch.executable,
            argv: launch.argv,
            cwd: launch.cwd,
            env: launch.env,
            ..params
        },
        lineage,
    ))
}

#[derive(Default)]
pub(super) struct SpawnLineage {
    pub(super) session: Option<String>,
    pub(super) identity: Option<(String, String)>,
    pub(super) prepared: Option<(Bootstrap, subjects::Ticket)>,
    pub(super) in_place: bool,
}

/// Creates the pane a child runs in and launches it there, once its parent pane is
/// resolved. A restart reuses this with the lead's own pane as the parent pane and
/// its resume recipe as the launch, so both paths share one durable sequence.
pub(super) fn spawn_child(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
    params: &SpawnParams,
    intent: &registry::SpawnRequest,
    parent_pane: String,
    lineage: SpawnLineage,
) -> Response {
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    if let Some(response) =
        replay_existing_operation_contents(ops::Operation::Spawn, request, &requester, core)
    {
        return spawn_answer(request, response, core);
    }
    if lineage.prepared.is_none()
        && let Some(parent_subject_id) = params.managed.parent_subject_id.as_deref()
    {
        let registry = Arc::clone(&locked(core).registry);
        match subjects::current_attachment(&registry, parent_subject_id) {
            Ok(Some(parent_subject))
                if parent_subject.process.bound_registration_id.as_deref()
                    == Some(params.parent.as_str()) => {}
            Ok(Some(_)) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::BadParams,
                        "managed parent_subject_id does not name this registered parent",
                    ),
                );
            }
            Ok(None) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::NotFound, "managed parent subject is not recorded"),
                );
            }
            Err(error) => return registry_error(request, error),
        }
    }
    let now = store::now_ms();
    let operation = ops::Operation::Spawn;
    let (backend, cancel) = match admit(
        operation,
        request,
        Some(&parent_pane),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return spawn_answer(request, response, core),
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(parent_pane), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(
        &request.id,
        operation.as_str(),
        requester,
        Some(parent_pane.clone()),
        now,
    )
    .with_params(request.params.clone());
    // The intent, its parent and the token the child will present are durable
    // before the first effect: a spawn that dies here leaves a readable intent,
    // not nothing to reconcile.
    if let Err(message) = locked(core).registry.record_spawn(intent, now) {
        let settled = record.complete(
            RequestOutcome::Refused,
            None,
            Some(&message),
            Vec::new(),
            store::now_ms(),
        );
        let _ = write_settled(request, settled, core);
        return Response::refused(&request.id, &Refusal::new(Code::Refused, message));
    }
    // A backend that tracks its launches gets the subject before any effect, so
    // the launch's correlation is recorded before the launch can run it.
    let tracking = if let Some(prepared) = lineage.prepared {
        Some(prepared)
    } else if backend
        .primitives()
        .contains(Primitives::LIFECYCLE_TRACKING)
    {
        match track_launch(
            core,
            &request.id,
            requested_launch(params),
            lineage.session.clone(),
            lineage.identity.clone(),
            params.managed.clone(),
            now,
        ) {
            Ok(tracking) => Some(tracking),
            Err(message) => {
                let settled = record.complete(
                    RequestOutcome::Refused,
                    None,
                    Some(&message),
                    Vec::new(),
                    store::now_ms(),
                );
                let _ = write_settled(request, settled, core);
                return Response::refused(&request.id, &Refusal::new(Code::Internal, message));
            }
        }
    } else {
        None
    };
    // The pane is a split of the parent's own pane, unfocused: a child is
    // created where its parent is, and creating one must not move the operator's
    // cursor.
    // The process serving this backend is read and verified on the daemon-wide
    // verification budget before the create is dispatched: a pane belongs to the
    // generation serving then, or to one that replaced it, so a later close can
    // tell the two apart instead of trusting a reused pane id. A verifier that
    // stalls is abandoned at the deadline, and a capture that cannot be verified
    // records nothing.
    let verifier = Arc::clone(&locked(core).verifier);
    let instance = serving_identity(&backend, &verifier, &cancel, verification);
    let create_request = CreateRequest::PaneSplit {
        pane_id: parent_pane.clone(),
        direction: SplitDirection::Right,
        focus: false,
    };
    let created = if lineage.in_place {
        CreateOutcome::Completed(crate::runtime::CreatedLocation {
            workspace_id: None,
            tab_id: None,
            pane_id: Some(parent_pane),
        })
    } else {
        backend.create(&create_request, &cancel)
    };
    let (created_outcome, named, mut created_message) = match &created {
        CreateOutcome::Completed(named) => (
            RequestOutcome::Completed,
            Some(named),
            named.is_empty().then(|| {
                "the runtime confirmed the creation without naming what it created".to_string()
            }),
        ),
        CreateOutcome::Refused(message) => (RequestOutcome::Refused, None, Some(message.clone())),
        CreateOutcome::Unknown(message) => (RequestOutcome::Unknown, None, Some(message.clone())),
    };
    let location = named
        .filter(|named| !named.is_empty())
        .map(|named| created_location(named, &backend.identity()));
    // The pane this launch was dispatched into is recorded as one of this
    // daemon's containers before the edge and the launch are recorded: a spawned
    // agent's slot is topology, not only a location on an edge.
    if let Some(named) = named.filter(|_| !lineage.in_place) {
        let registry = {
            let core = locked(core);
            Arc::clone(&core.registry)
        };
        let at = store::format_millis(store::now_ms());
        if let Err(error) = containers::record_created(
            &registry,
            containers::Confirmed {
                id: store::random_uuid(),
                recipe: false,
                request: &create_request,
                created: named,
                backend: &backend.identity(),
                incarnation: instance.clone(),
                label: None,
                created_by: Some(request.id.clone()),
            },
            &at,
        ) {
            created_message = Some(format!("the created container was not recorded: {error}"));
        }
    }
    let mut effects = named.map(CreatedLocation::effects).unwrap_or_default();
    let pane = named.and_then(|named| named.identity(CreatedKind::Pane));
    // An instance identity is recorded only for a pane: an edge with no location
    // has a generation no close could use.
    let instance = if pane.is_some() { instance } else { None };
    // The create is recorded before the launch is attempted: a confirmed pane is
    // readable even when the launch never answers.
    let written = locked(core).registry.record_spawn_created(
        &request.id,
        created_outcome,
        location.clone(),
        instance,
    );
    let edge = match written {
        Ok(edge) => edge,
        Err(error) => {
            // Recording an effect is part of doing it: the launch is not
            // attempted past a step the edge could not write, and the store
            // record still names the pane the create confirmed, so a pane that
            // exists is not an unattributable one.
            let settled = record.created(pane).complete(
                RequestOutcome::Unknown,
                None,
                Some(&error),
                effects,
                store::now_ms(),
            );
            let _ = write_settled(request, settled, core);
            return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
        }
    };
    // A command runs only in a pane the create named: with no pane there is
    // nothing to type into, and a launch is never claimed for a step that did not
    // happen.
    let launched = location
        .as_ref()
        .and_then(|location| location.pane.clone())
        .and_then(|pane| {
            let target = LaunchRequest {
                pane_id: pane,
                executable: params.executable.clone(),
                argv: params.argv.clone(),
                spawn_token: Some(edge.token.clone()),
                cwd: params.cwd.clone(),
                env: params.env.clone(),
            };
            let launch = match &tracking {
                Some((bootstrap, ticket)) => subjects::wrap(bootstrap, ticket, target).ok()?,
                None => target,
            };
            Some(backend.launch(&launch, &cancel))
        });
    // A launch whose scope cannot be formed at all is refused before anything runs.
    if tracking.is_some() && launched.is_none() {
        let settled = record.created(pane).complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_UNAVAILABLE),
            Some("no execution scope launcher is configured"),
            effects,
            store::now_ms(),
        );
        let _ = write_settled(request, settled, core);
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                "no execution scope launcher is configured",
            ),
        );
    }
    let (launched_outcome, launch_message) = match &launched {
        Some(LaunchOutcome::Completed) => (Some(RequestOutcome::Completed), None),
        Some(LaunchOutcome::Refused(_)) => (
            Some(RequestOutcome::Refused),
            Some("backend refused child launch".to_string()),
        ),
        Some(LaunchOutcome::Unknown(_)) => (
            Some(RequestOutcome::Unknown),
            Some("child launch outcome is unknown; it will not be retried".to_string()),
        ),
        None => (None, None),
    };
    let edge = match launched_outcome {
        Some(outcome) => {
            let written = locked(core)
                .registry
                .record_spawn_launched(&request.id, outcome);
            match written {
                Ok(edge) => edge,
                Err(error) => {
                    let settled = record.created(pane).complete(
                        RequestOutcome::Unknown,
                        None,
                        Some(&error),
                        effects,
                        store::now_ms(),
                    );
                    let _ = write_settled(request, settled, core);
                    return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
                }
            }
        }
        None => edge,
    };
    // The request's own outcome is the step that stopped it: completed only when
    // everything the daemon attempted completed, and never claiming a step that
    // did not run.
    let (outcome, category, message) = match (created_outcome, launched_outcome) {
        (RequestOutcome::Completed, Some(RequestOutcome::Completed)) => {
            (RequestOutcome::Completed, None, None)
        }
        (RequestOutcome::Completed, Some(RequestOutcome::Refused)) => (
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            launch_message.as_deref(),
        ),
        // A create that named no pane left nothing to launch into.
        (RequestOutcome::Completed, None) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            created_message.as_deref(),
        ),
        (RequestOutcome::Completed, Some(RequestOutcome::Unknown)) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            launch_message.as_deref(),
        ),
        (RequestOutcome::Refused, _) => (
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            created_message.as_deref(),
        ),
        (RequestOutcome::Unknown, _) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            created_message.as_deref(),
        ),
    };
    if launched_outcome == Some(RequestOutcome::Completed) {
        effects.push("launched child".to_string());
    }
    let done = record
        .created(pane)
        .complete(outcome, category, message, effects, store::now_ms());
    settle_spawn(request, done, &edge, core)
}

/// The launch chain a tracked launch starts: this binary, serving `socket`. `None`
/// when either path is not text the launch argv can carry.
pub(super) fn bootstrap_for(socket: &Path) -> Option<Bootstrap> {
    Some(Bootstrap {
        executable: std::env::current_exe().ok()?.to_str()?.to_string(),
        socket: socket.to_str()?.to_string(),
    })
}

/// Records the launch subject for a tracked spawn and attaches it to the edge,
/// returning what the launch needs to start its claim.
pub(super) fn track_launch(
    core: &Arc<Mutex<Core>>,
    request_id: &str,
    launch: subjects::LaunchContext,
    session: Option<String>,
    identity: Option<(String, String)>,
    managed: subjects::ManagedMetadata,
    now: i64,
) -> Result<(Bootstrap, subjects::Ticket), String> {
    let (registry, bootstrap) = {
        let core = locked(core);
        (Arc::clone(&core.registry), core.bootstrap.clone())
    };
    let bootstrap =
        bootstrap.ok_or("this daemon has no executable path a tracked launch can run through")?;
    let ticket = subjects::create_with_intent(
        &registry, request_id, launch, session, identity, managed, now,
    )?;
    registry.attach_subject(request_id, &ticket.claim_id, &ticket.registration_id)?;
    Ok((bootstrap, ticket))
}

pub(super) fn requested_launch(params: &SpawnParams) -> subjects::LaunchContext {
    subjects::LaunchContext {
        executable: params.executable.clone(),
        argv: params.argv.clone(),
        cwd: params.cwd.clone(),
        env: params.env.clone(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClaimParams {
    pub(super) claim_id: String,
    pub(super) correlation: String,
    pub(super) process: ProcessIdentity,
}

/// Captures the process a tracked launch's `radar run --launch` claims, and answers
/// with the registration to enter a scope under and the launch's program.
///
/// Released only when the subject is pending with this correlation, the pane the
/// edge recorded has this process as its foreground, the identity matches the
/// live process, and a pidfd can be opened on it. Every refusal reads the same
/// way, so a caller cannot probe which of those checks failed.
pub(super) fn claim_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let params: ClaimParams = match decode_params(request) {
        Ok(value) => value,
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
    let not_awaiting = || {
        Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, subjects::NOT_AWAITING),
        )
    };
    if !backend
        .primitives()
        .contains(Primitives::LIFECYCLE_TRACKING)
    {
        return not_awaiting();
    }
    let subject = match subjects::read(&registry, &params.claim_id) {
        Ok(Some(subject))
            if subject.process.state == subjects::State::Pending
                && subject.process.correlation.as_deref() == Some(params.correlation.as_str()) =>
        {
            subject
        }
        Ok(_) => return not_awaiting(),
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let edge = match registry.spawn_edge(&subject.process.spawn_request_id) {
        Ok(Some(edge)) => edge,
        Ok(None) => return not_awaiting(),
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let Some(pane) = edge
        .location
        .as_ref()
        .and_then(|location| location.pane.clone())
    else {
        return not_awaiting();
    };
    if let Err(refusal) = verify_live_occupant(
        &backend,
        &verifier,
        &pane,
        &params.process,
        &stopping,
        verification,
    ) {
        return Response::refused(&request.id, &refusal);
    }
    // The pidfd is held across the context read, so the read describes the
    // claimed process. A later stop reopens one from the record.
    let _pidfd = match process_control::acquire(&params.process, verifier.as_ref()) {
        Ok(Acquired::Process(pidfd)) => pidfd,
        Ok(Acquired::Absent) => return not_awaiting(),
        Ok(Acquired::Replaced) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, "process identity has changed"),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Refused, error)),
    };
    let Ok(effective) = process_control::effective(params.process.pid) else {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, "launch context is unavailable"),
        );
    };
    // The launch chain runs no program before it is answered, so its context reads describe
    // the claimed process only while that identity still holds.
    if !matches!(
        verifier.inspect(params.process.pid),
        Ok(Some(observed)) if observed == params.process
    ) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "foreground process birth identity is unavailable or changed",
            ),
        );
    }
    // The wake is installed before the launch is captured, so the program's exit
    // is noticed from the moment it can run. A launch whose wake cannot be
    // installed is never released.
    if let Err(message) = install_exit_wake(core, &backend, &params.claim_id, &edge, &stopping) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, message),
        );
    }
    match subjects::capture(
        &registry,
        &params.claim_id,
        &params.correlation,
        &params.process,
        &effective,
        store::now_ms(),
    ) {
        Ok(()) => Response::ok(
            &request.id,
            json!({
                "registration_id": subject.process.registration_id,
                "executable": subject.process.launch.executable,
                "argv": subject.process.launch.argv,
            }),
        ),
        Err(error) => {
            retire_wake(&backend, &params.claim_id, &stopping);
            Response::refused(&request.id, &Refusal::new(Code::Refused, error))
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmitParams {
    pub(super) request_id: String,
    pub(super) pane: String,
    pub(super) process: crate::model::ProcessIdentity,
    pub(super) executable: String,
    #[serde(default)]
    pub(super) argv: Vec<String>,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    pub(super) kind: subjects::ProcessKind,
}

/// Admits an in-place managed launch, then answers with the identity the caller
/// must enter its scope under. The pane must already hold this process, so
/// nothing is created and no unverified placement is accepted.
pub(super) fn admit_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let params: AdmitParams = match decode_params(request) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if params.executable.is_empty() {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`executable` must not be empty"),
        );
    }
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
    // The caller reports its own placement; what makes the claim credible is that
    // this process is the pane's own descendant, verified here and not taken from
    // the caller's environment.
    let foreground = match bounded_foreground(
        &backend,
        &params.pane,
        &stopping,
        verification,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(crate::model::ForegroundEvidence::NonShell { pid, .. }) if pid > 0 => pid,
        Ok(_) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, "this pane's foreground is not the launch"),
            );
        }
        Err(()) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BackendUnavailable,
                    "foreground process evidence is unavailable",
                ),
            );
        }
    };
    if !descends_from(params.process.pid, foreground) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "this process does not belong to the named pane",
            ),
        );
    }
    match verify_birth_identity(
        params.process.pid,
        &verifier,
        &stopping,
        verification,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(observed) if observed == params.process => {}
        _ => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Refused,
                    "process birth identity is unavailable or changed",
                ),
            );
        }
    }
    let launch = subjects::LaunchContext {
        executable: params.executable,
        argv: params.argv,
        cwd: params.cwd,
        env: std::collections::BTreeMap::new(),
    };
    let process = match subjects::create_process(
        &registry,
        &params.request_id,
        launch,
        params.kind,
        &params.process,
        store::now_ms(),
    ) {
        Ok(process) => process,
        Err(error) => {
            return Response::refused(&request.id, &Refusal::new(Code::Refused, error));
        }
    };
    // The scope is a pure function of the registration, so the client derives it
    // rather than receiving a second copy of the naming rule.
    Response::ok(
        &request.id,
        json!({"registration_id": process.registration_id}),
    )
}

/// Whether `pid` is the pane process or runs under it. Bounded, so a malformed
/// or recycled process tree cannot hold a request open.
pub(super) fn descends_from(pid: i32, ancestor: i32) -> bool {
    let mut current = pid;
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match parent_of(current) {
            Some(parent) if parent > 1 => current = parent,
            _ => return false,
        }
    }
    false
}

pub(super) fn parent_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is parenthesised and may contain spaces, so the fields
    // after it are read from the right of it: state, then the parent pid.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Installs the pane-death wake for a launch, naming the server generation its
/// pane was created in. A launch whose generation was not recorded cannot be
/// fenced, so it has no wake.
pub(super) fn install_exit_wake(
    core: &Arc<Mutex<Core>>,
    backend: &Arc<dyn RuntimeProvider>,
    subject_id: &str,
    edge: &registry::SpawnEdge,
    stopping: &AtomicBool,
) -> Result<(), String> {
    let bootstrap = locked(core)
        .bootstrap
        .clone()
        .ok_or("this daemon has no executable path a tracked launch can run through")?;
    let instance = edge
        .instance
        .as_ref()
        .ok_or("the launch's server generation is unrecorded, so its exit cannot be notified")?;
    let command = subjects::wake_command(&bootstrap, subject_id, instance)?;
    backend.install_exit_wake(subject_id, &command, stopping)
}
