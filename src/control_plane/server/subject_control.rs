use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubjectParams {
    #[serde(default)]
    pub(super) subject_id: Option<String>,
    pub(super) claim_id: String,
    #[serde(default)]
    pub(super) spawn_request_id: Option<String>,
}

/// Validates, records and dispatches a cancel or stop of a tracked launch.
///
/// The target is the exact subject, named directly or through the spawn that
/// created it. The record's target is the subject's pane, so a competing
/// mutation of that pane waits behind it. The record names each effect: launch
/// revocation (cancel only), signal dispatch, exit verification, and the pane an
/// unbound launch left behind.
pub(super) fn execute_subject_control(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    operation: ops::Operation,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    let frozen = frozen_params(request, &[])?;
    let params: SubjectParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Err(Refusal::new(
                Code::BadParams,
                "`request` is not a valid subject request".to_string(),
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    run_subject_control(request, params, requester, core, operation, verification)
}

pub(super) fn run_subject_control(
    request: &Request,
    params: SubjectParams,
    requester: Option<String>,
    core: &Arc<Mutex<Core>>,
    operation: ops::Operation,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    let registry = Arc::clone(&locked(core).registry);
    let (subject_id, target) = cancel_target(&registry, &params)?;
    let now = store::now_ms();
    match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return *response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(..) => {}
    }
    let current_subject = match subjects::read_claim(&registry, &subject_id) {
        Ok(subject) => subject,
        Err(error) => return Err(registry_refusal(error)),
    };
    if current_subject
        .as_ref()
        .is_some_and(|subject| !current_presence_grant(&registry, subject).unwrap_or(false))
    {
        let record = RequestRecord::new(
            &request.id,
            operation.as_str(),
            requester,
            Some(target.clone()),
            now,
        )
        .with_params(request.params.clone())
        .complete(
            Settlement::backend_refused("the Pi session has no current granted presence claim"),
            store::now_ms(),
        );
        return write_settled(record, core);
    }
    let (verifier, backend, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.verifier),
            core.backend.clone(),
            Arc::clone(&core.stopping),
        )
    };
    let termination = match operation {
        ops::Operation::Stop => subjects::stop(&registry, &subject_id, verifier.as_ref(), now),
        _ => subjects::cancel(&registry, &subject_id, verifier.as_ref(), now),
    };
    // Every cancel asks about the launch's own pane, so a launch already cleaned
    // up reports that instead of closing anything again. A bound launch's pane
    // belongs to its child, and a stop leaves the launch exactly as it is.
    let cleanup = match (&termination, operation, &backend) {
        (Ok(_), ops::Operation::Cancel, Some(backend)) => cleanup_launch_pane(
            core,
            backend,
            &subject_id,
            CleanupIntent::Close,
            &stopping,
            verification,
            now,
        ),
        _ => None,
    };
    // The supervisor's report is read back here rather than at the pane, so a
    // close that removed the pane still reports how the program ended. It is
    // absent when nothing reported: never an error, and never a block.
    let claim = subjects::read_claim(&registry, &subject_id).ok().flatten();
    let completion = claim
        .as_ref()
        .and_then(|claim| claim.process.completion.clone());
    let settlement = match termination {
        Ok(termination) => {
            if let (subjects::Exit::Exited, Some(backend)) = (&termination.exit, &backend) {
                retire_wake(backend, &subject_id, &stopping);
            }
            // The subject's own attempt is what the close addressed: a read that
            // went astray after the stop leaves the report unexecutioned rather
            // than inventing an identity.
            termination_outcome(
                &termination,
                claim.as_ref(),
                operation,
                cleanup.as_ref(),
                completion,
            )
        }
        Err(error) => Settlement::new(RequestOutcome::Unknown, None, Some(&error), Vec::new()),
    };
    let record = RequestRecord::new(
        &request.id,
        operation.as_str(),
        requester,
        Some(target),
        now,
    )
    .with_params(request.params.clone())
    .complete(settlement, store::now_ms());
    write_settled(record, core)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EnrollParams {
    pub(super) registration_id: String,
    pub(super) session: String,
}

/// Enrolls a running lead the daemon did not launch as a restartable subject.
///
/// The named registration must claim a process that is verifiably the foreground of
/// its own pane, and its fresh session context must name the operator's session.
/// Nothing is signalled or relaunched: the enrollment records what was verified.
pub(super) fn execute_enroll(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    let frozen = frozen_params(request, &[])?;
    let params: EnrollParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Err(Refusal::new(
                Code::BadParams,
                "`request` is not a valid enrollment".to_string(),
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    if crate::model::SessionUuid::parse(&params.session).is_none() {
        return Err(Refusal::new(
            Code::BadParams,
            "`session` must be a canonical UUID".to_string(),
        ));
    }
    let (registry, verifier, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let registration_id = id_param(
        &params.registration_id,
        "registration_id",
        RegistrationId::required,
    )?;
    let now = store::now_ms();
    let registration = match registry.get(&registration_id) {
        Ok(Some(registration)) => registration,
        Ok(None) => {
            return Err(Refusal::new(
                Code::NotFound,
                "registration is not recorded".to_string(),
            ));
        }
        Err(error) => return Err(Refusal::new(Code::Internal, error)),
    };
    let Some(pane) = registration
        .request
        .location
        .as_ref()
        .and_then(|location| location.pane.clone())
    else {
        return Err(Refusal::new(
            Code::Refused,
            "registration names no pane to enroll".to_string(),
        ));
    };
    let Some(claim) = registration.request.process.clone() else {
        return Err(Refusal::new(
            Code::Refused,
            "registration claims no process to enroll".to_string(),
        ));
    };
    match registry.context(&registration_id, now) {
        Ok(Some(context))
            if context.context.freshness == registry::Freshness::Fresh
                && context.context.session.as_deref() == Some(params.session.as_str()) => {}
        Ok(_) => {
            return Err(Refusal::new(
                Code::Refused,
                "registration's fresh session context does not match the session".to_string(),
            ));
        }
        Err(error) => return Err(Refusal::new(Code::Internal, error)),
    }
    let (backend, _cancel) = match admit(
        ops::Operation::Enroll,
        request,
        Some(&pane),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return *response,
        Admission::Unsupported(message) => {
            return execute_unsupported(
                ops::Operation::Enroll,
                request,
                &message,
                Some(pane),
                core,
            );
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(
        &request.id,
        ops::Operation::Enroll.as_str(),
        requester,
        Some(pane.clone()),
        now,
    )
    .with_params(request.params.clone());
    let enrolled =
        match verify_live_occupant(&backend, &verifier, &pane, &claim, &stopping, verification) {
            Err(refusal) => Err(refusal.message),
            Ok(()) => record_enrollment(&registry, &verifier, &params, &claim, &request.id, now),
        };
    let settlement = match enrolled {
        Ok(subject_id) => Settlement::completed(vec![format!("enrolled subject {subject_id}")]),
        Err(message) => Settlement::backend_refused(message),
    };
    write_settled(record.complete(settlement, store::now_ms()), core)
}

/// Records the enrolled subject for a verified process. The launch is read only
/// while the claimed birth identity still holds, so it describes the claimed process.
pub(super) fn record_enrollment(
    registry: &registry::Registry,
    verifier: &Arc<dyn ProcessVerifier>,
    params: &EnrollParams,
    claim: &crate::model::ProcessIdentity,
    enrollment_id: &str,
    now: i64,
) -> Result<ClaimId, String> {
    let enrollment_id = ClaimId::required(enrollment_id, "enrollment_id")?;
    let registration_id = RegistrationId::required(&params.registration_id, "registration_id")?;
    let presence = registry::presence::current(registry, &registration_id)?
        .ok_or("lifecycle enrollment requires a current presence claim")?;
    if presence.session_id.as_deref() != Some(params.session.as_str())
        || !matches!(
            presence.standing,
            registry::presence::Standing::Pending | registry::presence::Standing::Granted
        )
    {
        return Err("lifecycle enrollment does not match an accepted presence claim".into());
    }
    let effective = process_control::effective(claim.pid)
        .map_err(|_| "launch context is unavailable".to_string())?;
    if !matches!(verifier.inspect(claim.pid), Ok(Some(observed)) if observed == *claim) {
        return Err("foreground process birth identity is unavailable or changed".into());
    }
    let launch = subjects::captured_launch(&effective)?;
    subjects::enroll_subject(
        registry,
        &enrollment_id,
        &registration_id,
        claim,
        &params.session,
        launch,
        now,
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestartParams {
    pub(super) subject_id: String,
    /// The operator names the claim it saw, which fences the restart.
    #[serde(default)]
    pub(super) claim_id: Option<String>,
    /// A parent names itself instead and sends the program the child restarts as.
    #[serde(default)]
    pub(super) parent: Option<String>,
    #[serde(default)]
    pub(super) executable: Option<String>,
    #[serde(default)]
    pub(super) argv: Vec<String>,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    #[serde(default)]
    pub(super) env: std::collections::BTreeMap<String, String>,
}

impl RestartParams {
    /// The program a parent sends, or `None` for an operator restart.
    pub(super) fn parent_launch(&self) -> Result<Option<subjects::LaunchContext>, &'static str> {
        match (&self.claim_id, &self.parent, &self.executable) {
            (Some(_), None, None) => Ok(None),
            (None, Some(_), Some(executable)) => Ok(Some(subjects::LaunchContext {
                executable: executable.clone(),
                argv: self.argv.clone(),
                cwd: self.cwd.clone(),
                env: self.env.clone(),
            })),
            _ => Err("a restart names a claim, or a parent with the program to restart as"),
        }
    }
}

/// Restarts an enrolled or tracked lead: stops its verified process, then launches a
/// replacement that resumes its Pi session in a fresh pane split from the lead's own.
///
/// Refusals before the stop change nothing. The replacement is recorded as the old
/// process's successor before it is launched, so a replay under a new request never
/// launches a second one. Children and their immutable edges are left as they are.
pub(super) fn execute_restart(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    let frozen = frozen_params(request, &[])?;
    let params: RestartParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Err(Refusal::new(
                Code::BadParams,
                "`request` is not a valid restart request".to_string(),
            ));
        }
    };
    let requester = request.optional_string("requester")?;
    run_restart(request, params, requester, core, verification)
}

pub(super) fn run_restart(
    request: &Request,
    params: RestartParams,
    requester: Option<String>,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Reply {
    if crate::model::SessionUuid::parse(&params.subject_id).is_none() {
        return Err(Refusal::new(
            Code::BadParams,
            "`subject_id` must be a canonical UUID",
        ));
    }

    if let Some(response) = replay_existing_operation(
        ops::Operation::Restart,
        request,
        Some(&params.subject_id),
        &requester,
        core,
    ) {
        return response;
    }
    let registry = Arc::clone(&locked(core).registry);
    let subject_id = id_param(&params.subject_id, "subject_id", SubjectId::required)?;
    let subject = match subjects::current_attachment(&registry, &subject_id) {
        Ok(Some(subject)) => subject,
        Ok(None) => {
            return Err(Refusal::new(
                Code::NotFound,
                "launch subject is not recorded".to_string(),
            ));
        }
        Err(error) => return Err(Refusal::new(Code::Internal, error)),
    };
    let replacement = match params.parent_launch() {
        Ok(replacement) => replacement,
        Err(message) => {
            return Err(Refusal::new(Code::BadParams, message));
        }
    };
    match (&params.claim_id, &params.parent) {
        (Some(claim_id), _) if claim_id.as_str() != subject.claim_id.as_str() => {
            return Err(Refusal::new(
                Code::Refused,
                "the requested claim is not current; nothing was signalled",
            ));
        }
        (None, Some(parent)) => {
            let owned = match subject.managed.parent_subject_id.clone() {
                Some(parent_subject) => matches!(
                    subjects::current_attachment(&registry, &parent_subject),
                    Ok(Some(claim)) if claim.process.bound_registration_id.as_ref().map(RegistrationId::as_str) == Some(parent.as_str())
                ),
                None => false,
            };
            if !owned {
                return Err(Refusal::new(
                    Code::Refused,
                    "the subject is not this parent's child",
                ));
            }
        }
        _ => {}
    }
    // A restart retains the lead's existing pane.
    let anchor = match subject.process.bound_registration_id.as_ref() {
        Some(agent) => match registry.get(agent) {
            Ok(Some(registration)) => registration
                .request
                .location
                .and_then(|location| location.pane),
            Ok(None) => None,
            Err(error) => {
                return Err(Refusal::new(Code::Internal, error));
            }
        },
        None => None,
    };
    let Some(anchor) = anchor else {
        return Err(Refusal::new(
            Code::Refused,
            "restart needs the lead's own recorded pane as its anchor".to_string(),
        ));
    };
    // A successor launched into a branch that is being removed would arrive in a
    // place about to disappear, so the restart is refused before it signals
    // anything: the predecessor keeps running until the close decides its fate.
    super::closing::refuse_if_closing(core, &anchor)?;
    let now = store::now_ms();
    let subject_key = subject.subject_id.to_string();
    let cancel = match admit(
        ops::Operation::Restart,
        request,
        Some(&subject_key),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return *response,
        Admission::Unsupported(message) => {
            return execute_unsupported(
                ops::Operation::Restart,
                request,
                &message,
                Some(subject_key.clone()),
                core,
            );
        }
        Admission::Dispatch(_, cancel) => cancel,
    };
    let mut record = RequestRecord::new(
        &request.id,
        ops::Operation::Restart.as_str(),
        requester,
        Some(subject_key),
        now,
    )
    .with_params(request.params.clone());
    let launch_request_id = store::random_uuid();
    record.launch_request_id = Some(launch_request_id.clone());
    let settlement = restart_lead(
        core,
        &subject,
        replacement,
        &anchor,
        &cancel,
        verification,
        &launch_request_id,
    );
    write_settled(record.complete(settlement, store::now_ms()), core)
}

/// The restart's effects after admission, in order: the checks, the verified stop,
/// the successor record, then the replacement launch. Each stage reports on its own.
pub(super) fn restart_lead(
    core: &Arc<Mutex<Core>>,
    subject: &subjects::Claim,
    replacement: Option<subjects::LaunchContext>,
    anchor: &str,
    cancel: &AtomicBool,
    verification: &Arc<VerificationBudget>,
    successor: &str,
) -> Settlement {
    let now = store::now_ms();
    let (registry, backend, verifier) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            core.backend.clone(),
            Arc::clone(&core.verifier),
        )
    };
    let stopping = Arc::clone(&locked(core).stopping);
    let Some(backend) = backend else {
        return Settlement::backend_refused("no backend is wired");
    };
    if subject.process.state != subjects::State::Bound {
        return Settlement::backend_refused("the subject is not bound to a live process");
    }
    if !current_presence_grant(&registry, subject).unwrap_or(false) {
        return Settlement::backend_refused("the Pi session has no current granted presence claim");
    }
    let (Some(agent), Some(session)) = (
        subject.process.bound_registration_id.as_ref(),
        subject.session.as_deref(),
    ) else {
        return Settlement::backend_refused(
            "the subject has no session to resume; enroll it first",
        );
    };
    match backend.inventory(cancel) {
        Ok(fleet)
            if fleet
                .panes
                .iter()
                .any(|pane| pane.location.pane_id == anchor) => {}
        Ok(_) => {
            return Settlement::backend_refused(
                "the lead's pane is no longer present to anchor the replacement",
            );
        }
        Err(error) => {
            return Settlement::refused(Category::BackendUnavailable, error);
        }
    }
    match registry.context(agent, now) {
        Ok(Some(context)) if context.context.session.as_deref() == Some(session) => {}
        Ok(_) => {
            return Settlement::backend_refused(
                "the lead's published session no longer matches its recipe; enroll it again",
            );
        }
        Err(error) => return Settlement::backend_refused(error),
    }
    let program = replacement.as_ref().unwrap_or(&subject.process.launch);
    let resume = match subjects::resume_launch(program, session) {
        Ok(resume) => resume,
        Err(message) => return Settlement::backend_refused(message),
    };
    let params = SpawnParams {
        parent: agent.clone(),
        parent_session_id: session.to_string(),
        executable: resume.executable,
        argv: resume.argv,
        cwd: resume.cwd,
        env: resume.env,
        managed: subject.managed.clone(),
        continue_subject_id: None,
    };
    if let Err(message) = validate_command(&params.executable, &params.argv) {
        return Settlement::backend_refused(message);
    }
    if let Err(message) =
        subjects::check_bounds_with_metadata(&requested_launch(&params), &subject.managed)
    {
        return Settlement::backend_refused(message);
    }
    if let Err(refusal) = preflight_launch(core, &params) {
        return Settlement::backend_refused(refusal.message);
    }
    let intent = registry::SpawnRequest {
        request_id: successor.to_string(),
        parent: params.parent.clone(),
        parent_session_id: params.parent_session_id.clone(),
        parent_subject_id: params.managed.parent_subject_id.clone(),
    };
    if let Err(error) = registry.record_spawn(&intent, now) {
        return Settlement::backend_refused(error);
    }
    let identity = (subject.subject_id.clone(), subject.claim_id.clone());
    let prepared = match track_launch(
        core,
        successor,
        requested_launch(&params),
        Some(session.into()),
        Some(identity.clone()),
        subject.managed.clone(),
        now,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return Settlement::backend_refused(error),
    };
    if let Err(error) =
        registry.attach_subject(successor, &prepared.1.claim_id, &prepared.1.registration_id)
    {
        return Settlement::backend_refused(error);
    }
    let mut effects = Vec::new();
    let termination = subjects::stop(&registry, &subject.claim_id, verifier.as_ref(), now);
    if !termination
        .as_ref()
        .is_ok_and(|result| matches!(result.exit, subjects::Exit::Exited))
        && let Err(error) =
            subjects::cancel_reservation(&registry, &subject.claim_id, &prepared.1.claim_id, now)
    {
        return Settlement {
            effects,
            ..Settlement::backend_unknown(error)
        };
    }
    let termination = match termination {
        Ok(termination) => termination,
        Err(error) => {
            return Settlement {
                effects,
                ..Settlement::backend_unknown(error)
            };
        }
    };
    if termination.signal_attempted {
        effects.push("signalled previous process".to_string());
    }
    match termination.exit {
        subjects::Exit::Exited => effects.push("previous process exited".to_string()),
        subjects::Exit::Unresolved => {
            return Settlement {
                effects,
                ..Settlement::backend_refused(
                    "the previous process could not be identified; no replacement was launched",
                )
            };
        }
        subjects::Exit::Unverified(reason) => {
            return Settlement {
                effects,
                ..Settlement::backend_unknown(format!(
                    "previous process exit is not verified; no replacement was launched: {reason}"
                ))
            };
        }
    }
    effects.push(format!("successor {successor}"));
    let reuse = reusable_pane(
        core,
        &backend,
        &subject.claim_id,
        anchor,
        &stopping,
        verification,
    );
    let reused = reuse.is_some();
    let replacement = Request {
        id: successor.to_string(),
        method: ops::Operation::Spawn.as_str().to_string(),
        params: serde_json::json!({
            "parent": params.parent,
            "parent_session_id": params.parent_session_id,
            "executable": params.executable,
            "argv": params.argv,
            "cwd": params.cwd,
            "env": params.env,
        }),
    };
    let _ = spawn_child(
        &replacement,
        core,
        verification,
        &params,
        &intent,
        anchor.to_string(),
        SpawnLineage {
            session: Some(session.to_string()),
            identity: Some(identity),
            prepared: Some(prepared),
            reuse,
        },
    );
    // The predecessor's launch ended when its exit was verified above, so the pane
    // it ran in is a dead shell this daemon made permanent by arming
    // remain-on-exit. It is removed once the successor exists, because the
    // successor's pane is split from it and a split needs its target present. A
    // successor that never launched leaves the pane where the failure can be read.
    let predecessor_pane = |effects: &mut Vec<String>| {
        effects.push(
            match cleanup_launch_pane(
                core,
                &backend,
                &subject.claim_id,
                CleanupIntent::Retire,
                &stopping,
                verification,
                store::now_ms(),
            ) {
                Some(cleanup) => format!("pane {}", subjects::ResourceEnd::from(&cleanup)),
                None => "pane kept".to_string(),
            },
        );
    };
    match registry.spawn_edge(successor) {
        Ok(Some(edge)) => match edge.launched {
            RequestOutcome::Completed => {
                effects.push("launched replacement".to_string());
                if !reused {
                    predecessor_pane(&mut effects);
                }
                Settlement::completed(effects)
            }
            RequestOutcome::Unknown => {
                if !reused {
                    predecessor_pane(&mut effects);
                }
                Settlement {
                    effects,
                    ..Settlement::backend_unknown(
                        "replacement launch is unknown; it will not be retried",
                    )
                }
            }
            _ => Settlement {
                effects,
                ..Settlement::backend_refused("replacement was not launched")
            },
        },
        Ok(None) => Settlement {
            effects,
            ..Settlement::backend_refused("replacement was not recorded")
        },
        Err(error) => Settlement {
            effects,
            ..Settlement::backend_unknown(error)
        },
    }
}

/// The subject a cancel or stop names and the target it is admitted under.
pub(super) fn current_presence_grant(
    registry: &Registry,
    subject: &subjects::Claim,
) -> Result<bool, String> {
    let (Some(registration_id), Some(session)) = (
        subject.process.bound_registration_id.as_ref(),
        subject.session.as_deref(),
    ) else {
        return Ok(true);
    };
    let Some(registration) = registry.get(registration_id)? else {
        return Ok(false);
    };
    if registration.request.source != "herdsman-pi" {
        return Ok(true);
    }
    let Some(claim) = registry::presence::current(registry, registration_id)? else {
        return Ok(false);
    };
    Ok(claim.standing == registry::presence::Standing::Granted
        && claim.claim_id == subject.claim_id
        && claim.session_id.as_deref() == Some(session)
        && claim.subject_id.as_ref() == Some(&subject.subject_id))
}

pub(super) fn cancel_target(
    registry: &Registry,
    params: &SubjectParams,
) -> Result<(ClaimId, String), Refusal> {
    let internal = |error: String| Refusal::new(Code::Internal, error);
    let subject = match (&params.subject_id, &params.spawn_request_id) {
        (Some(subject_id), None) => subjects::current_attachment(
            registry,
            &id_param(subject_id, "subject_id", SubjectId::required)?,
        )
        .map_err(internal)?
        .ok_or_else(|| Refusal::new(Code::NotFound, "launch subject is not recorded"))?,
        (None, Some(request_id)) => {
            let edge = registry
                .spawn_edge(request_id)
                .map_err(internal)?
                .ok_or_else(|| Refusal::new(Code::NotFound, "spawn is not recorded"))?;
            let claim_id = edge.claim_id.ok_or_else(|| {
                Refusal::new(Code::Refused, "spawn is not a lifecycle-tracked launch")
            })?;
            let launched = subjects::read_claim(registry, &claim_id)
                .map_err(internal)?
                .ok_or_else(|| Refusal::new(Code::NotFound, "launch subject is not recorded"))?;
            let subject = subjects::current_attachment(registry, &launched.subject_id)
                .map_err(internal)?
                .ok_or_else(|| Refusal::new(Code::NotFound, "launch subject is not recorded"))?;
            if subject.process.registration_id != launched.process.registration_id {
                return Err(Refusal::new(
                    Code::Refused,
                    "the spawn's process was replaced; nothing was signalled",
                ));
            }
            subject
        }
        _ => {
            return Err(Refusal::new(
                Code::BadParams,
                "exactly one of `subject_id` or `spawn_request_id` is required",
            ));
        }
    };
    let current = subjects::read_subject_record(registry, &subject.subject_id).map_err(internal)?;
    if params.claim_id != subject.claim_id.as_str()
        || current.is_none_or(|record| record.current_claim.as_ref() != Some(&subject.claim_id))
    {
        return Err(Refusal::new(
            Code::Refused,
            "the requested claim is not current; nothing was signalled",
        ));
    }
    Ok((subject.claim_id, subject.subject_id.to_string()))
}

/// What a cancel or stop established, in the parts a reader acts on.
///
/// Nothing is described in prose: whether the execution ended, whether the pane
/// was removed, and — when a supervisor reported one — how the program ended are
/// separate answers, because a completed removal says nothing about the code and
/// a verified exit says nothing about the pane. Only a cancel names a pane,
/// because only a cancel acts on one.
pub(super) fn termination_outcome(
    termination: &subjects::Termination,
    claim: Option<&subjects::Claim>,
    operation: ops::Operation,
    cleanup: Option<&subjects::PaneCleanup>,
    completion: Option<subjects::Completion>,
) -> Settlement {
    let closed = subjects::ClosedExecution {
        executions: claim
            .map(|claim| {
                vec![subjects::EndedExecution {
                    subject_id: Some(claim.subject_id.to_string()),
                    registration_id: claim.process.registration_id.to_string(),
                    end: (&termination.exit).into(),
                    completion: completion.clone(),
                }]
            })
            .unwrap_or_default(),
        // A cancel always answers for the pane, including when the launch has
        // none of this daemon's: "nothing was removed" and "this operation never
        // removes" are different answers.
        resource: (operation == ops::Operation::Cancel).then(|| {
            cleanup.map_or_else(
                || {
                    subjects::ResourceEnd::Kept(
                        "the launch records no pane of this daemon's".into(),
                    )
                },
                subjects::ResourceEnd::from,
            )
        }),
        escalated: termination.escalated,
    };
    match &termination.exit {
        subjects::Exit::Unverified(message) => {
            Settlement::new(RequestOutcome::Unknown, None, Some(message), Vec::new())
        }
        subjects::Exit::Unresolved | subjects::Exit::Exited => Settlement::completed(Vec::new()),
    }
    .closed(closed)
}

/// Removes a launch's exit wake once its exit is recorded. Best effort: a slot
/// left behind names a launch that has ended, and the next wake removes it.
pub(super) fn retire_wake(
    backend: &Arc<dyn RuntimeProvider>,
    claim_id: &ClaimId,
    stopping: &AtomicBool,
) {
    let _ = backend.remove_exit_wake(claim_id.as_str(), stopping);
}
