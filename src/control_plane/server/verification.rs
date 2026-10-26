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
    let observed = verify_birth_identity(
        pid,
        verifier,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    );
    match observed {
        Ok(observed)
            if pid == claim.pid
                && observed.pid == pid
                && observed.boot_id == claim.boot_id
                && observed.start_ticks == claim.start_ticks =>
        {
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
    /// cleanup is defined.
    Cleanup(registry::SpawnExitObservation),
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
    let Some(exit) = edge.exit else {
        return DeadPane::Unresolved;
    };
    match bounded_exit_observation(backend, pane_id, stopping, budget) {
        Ok(PaneExitObservation::Exited { .. }) => DeadPane::Cleanup(exit),
        // A pane reported running, undetermined, or unreadable is not a pane whose
        // command has exited: the live refusal stands.
        Ok(PaneExitObservation::Running | PaneExitObservation::Unknown) | Err(()) => {
            DeadPane::Unresolved
        }
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
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::runtime::CloseOutcome, ()> {
    let backend = Arc::clone(backend);
    let pane_id = pane_id.to_string();
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-child-close-dispatch",
        move || backend.close_outcome(&crate::runtime::CloseTarget::Pane(pane_id), &cancel),
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

/// Closes the pane an unbound launch left behind, fenced to the very instance
/// that created it, and records what happened as that launch's own durable fact.
///
/// Closing a pane ends whatever still runs in it, so a pane is closed only once
/// nothing this daemon launched can still be alive there: the launch's process is
/// verified gone, or the pane itself is a retained, already-exited pane of this
/// edge. Anything less leaves the pane to the operator and says why. This reports
/// a cleanup, never a process exit.
///
/// Only a cleanup that was dispatched is recorded: a confirmed close, a backend
/// refusal and an unanswered dispatch are each durable facts of the launch. An
/// eligibility refusal dispatches nothing, so it records nothing and a later
/// admitted cancel re-decides it against the evidence it finds.
pub(super) fn cleanup_launch_pane(
    registry: &Arc<Registry>,
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    subject_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    now_ms: i64,
) -> Option<subjects::PaneCleanup> {
    use subjects::PaneCleanup;

    let Ok(Some(subject)) = subjects::read(registry, subject_id) else {
        return Some(PaneCleanup::Refused(
            "the tracked launch is unavailable".into(),
        ));
    };
    // The first outcome is the one that stands, so a second cancel reports it
    // rather than closing anything again.
    if let Some(recorded) = subject.process.pane_cleanup.clone() {
        return Some(recorded);
    }
    // A bound launch's pane belongs to its registered child while that child is
    // alive, so a cancel has nothing to say about it. Once the child is
    // verifiably gone the pane is a dead shell this daemon made permanent by
    // arming remain-on-exit on it, so removing it is the daemon's own cleanup.
    // A launch that is neither bound nor revoked has no pane of its own at all.
    if !matches!(
        subject.process.state,
        subjects::State::Bound | subjects::State::Revoked
    ) {
        return Some(PaneCleanup::Refused(
            "the launch is not a bound or revoked one, so its pane is not this cleanup's to remove"
                .into(),
        ));
    }
    // A subject with no spawn edge never had a pane of this daemon's: an
    // enrolled lead's pane belongs to the operator who enrolled it.
    let Ok(Some(edge)) = registry.spawn_edge(&subject.process.spawn_request_id) else {
        return None;
    };
    let Some(location) = edge.location.as_ref() else {
        return Some(PaneCleanup::Refused("the launch records no pane".into()));
    };
    let Some(pane) = location.pane.clone() else {
        return Some(PaneCleanup::Refused("the launch records no pane".into()));
    };
    let identity = backend.identity();
    if location.backend != identity.name || location.instance != identity.instance {
        return Some(PaneCleanup::Refused(
            "the recorded pane belongs to a different backend instance".into(),
        ));
    }
    // A pane id outlives the server that answered the create, so the pane is only
    // this launch's while the very process that served the create still serves.
    let Some(created_in) = edge.instance.clone() else {
        return Some(PaneCleanup::Refused(
            "the launch records no verified backend instance for its pane".into(),
        ));
    };
    match serving_identity(backend, verifier, stopping, budget) {
        Some(serving) if serving == created_in => {}
        _ => {
            return Some(PaneCleanup::Refused(
                "the backend instance serving now is not the one that created the recorded pane"
                    .into(),
            ));
        }
    }
    if !backend.primitives().contains(Primitives::EXIT_OBSERVATION) {
        return Some(PaneCleanup::Refused(
            "this backend cannot report whether the pane's command has exited".into(),
        ));
    }
    let pane_dead = matches!(
        bounded_exit_observation(backend, &pane, stopping, budget),
        Ok(PaneExitObservation::Exited { .. })
    );
    // A bound child's pane can be handed to another process once that child is
    // gone, so only the backend's own report that this very pane is dead makes
    // it a dead shell to remove. A revoked launch never had a child, so a
    // verified exit is enough.
    let exited = if subject.process.state == subjects::State::Bound {
        pane_dead
    } else {
        subject.process.identity.is_none()
            || subject.process.exited_at.is_some()
            || (edge.exit.is_some() && pane_dead)
    };
    if !exited {
        return Some(PaneCleanup::Refused(
            "the launch's process is not verified gone; closing its pane would end it unclaimed"
                .into(),
        ));
    }
    let reader = Arc::clone(backend);
    let cancel = Arc::clone(stopping);
    let outcome = run_bounded(
        budget,
        stopping,
        Instant::now() + VERIFICATION_TIMEOUT,
        "radar-pane-cleanup",
        move || reader.close_outcome(&crate::runtime::CloseTarget::Pane(pane), &cancel),
    );
    let outcome = match outcome {
        Ok(crate::runtime::CloseOutcome::Completed) => PaneCleanup::Cleaned,
        Ok(crate::runtime::CloseOutcome::Refused(message)) => PaneCleanup::Refused(message),
        Ok(crate::runtime::CloseOutcome::Unknown(message)) => PaneCleanup::Unknown(message),
        // The dispatch may have reached the backend and its answer was lost.
        Err(()) => PaneCleanup::Unknown("the cleanup did not finish inside its deadline".into()),
    };
    match subjects::record_pane_cleanup(registry, subject_id, outcome, now_ms) {
        Ok(recorded) => Some(recorded),
        // An unrecorded outcome is never claimed as a cleanup: a replay must not
        // read "cleaned" for a close nothing committed.
        Err(error) => Some(PaneCleanup::Unknown(format!(
            "the cleanup outcome could not be recorded: {error}"
        ))),
    }
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

/// What one read's observation says about a recorded location.
///
/// A pane's absence is evidence about a pane, never about the process that ran
/// in it, and an edge that recorded no pane has nothing to check — so the read
/// reports the disagreement, or its lack of confirmation, and claims nothing
/// else.
pub(super) fn observe_spawn_exits(
    core: &Arc<Mutex<Core>>,
    edges: &[SpawnEdge],
) -> Result<Vec<SpawnEdge>, String> {
    let (registry, backend, cancel) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            core.backend.clone(),
            Arc::clone(&core.stopping),
        )
    };
    let Some(backend) = backend else {
        return Ok(edges.to_vec());
    };
    if cancel.load(Ordering::SeqCst) || !backend.primitives().contains(Primitives::EXIT_OBSERVATION)
    {
        return Ok(edges.to_vec());
    }
    let pending: Vec<_> = edges
        .iter()
        .filter(|edge| edge.exit.is_none())
        .filter_map(|edge| {
            edge.location
                .as_ref()
                .and_then(|location| location.pane.as_ref())
                .cloned()
        })
        .collect();
    if pending.is_empty() {
        return Ok(edges.to_vec());
    }
    let observations = backend.exit_observations(&pending, &cancel);
    edges
        .iter()
        .map(|edge| {
            if edge.exit.is_some() {
                return Ok(edge.clone());
            }
            let Some(pane) = edge
                .location
                .as_ref()
                .and_then(|location| location.pane.as_ref())
            else {
                return Ok(edge.clone());
            };
            let Some(PaneExitObservation::Exited { code, signal }) = observations.get(pane) else {
                return Ok(edge.clone());
            };
            registry.record_spawn_exit(&edge.request_id, *code, *signal, store::now_ms())
        })
        .collect()
}
