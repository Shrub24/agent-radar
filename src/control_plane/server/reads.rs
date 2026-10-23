use super::*;

pub(super) fn read_observation(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let params: ObservationGetParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let wait = params.wait_ms.unwrap_or(0);
    if wait > crate::control_plane::observation::MAX_OBSERVATION_WAIT_MS {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!(
                    "`wait_ms` must not exceed {} ms",
                    crate::control_plane::observation::MAX_OBSERVATION_WAIT_MS
                ),
            ),
        );
    }
    let classes = match crate::control_plane::observation::Classes::parse(
        params.classes.as_deref().unwrap_or(&[]),
    ) {
        Ok(classes) => classes,
        Err(message) => {
            return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
        }
    };
    let hub = { locked(core).hub.clone() };
    let Some(hub) = hub else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                "this daemon has no observation backend, so it has nothing to publish",
            ),
        );
    };
    let epoch = hub.epoch();
    // A revision from another process is expired rather than answered as
    // current, so an old reader never sees a new epoch's revision as its own.
    if params.epoch.as_deref().is_some_and(|named| named != epoch) {
        let (revision, _) = hub.current();
        return Response::ok(
            &request.id,
            json!(ObservationGetResponse::expired(epoch, revision)),
        );
    }
    // Demand is recorded before the answer, so the class this read asked for is
    // sampled for the next revision rather than the one after next. The revision
    // this returns is the one that existed before the demand, which is what makes
    // a first read deterministic: it cannot see a sample its own demand caused.
    let mut revision = hub.demand(classes);
    if revision == 0 && params.since_revision.is_none() {
        // Nothing has ever been sampled: this read is the first demand, and a
        // snapshot is not a thing that can be invented for it.
        return Response::ok(
            &request.id,
            json!(ObservationGetResponse::not_modified(epoch, 0)),
        );
    }
    if let Some(since) = params.since_revision {
        // A revision this epoch never had is expired, not merely old.
        if since > revision {
            return Response::ok(
                &request.id,
                json!(ObservationGetResponse::expired(epoch, revision)),
            );
        }
        if since == revision && wait > 0 {
            hub.wait_for_newer(revision, Duration::from_millis(wait));
            revision = hub.current().0;
        }
        if since == revision {
            return Response::ok(
                &request.id,
                json!(ObservationGetResponse::not_modified(epoch, revision)),
            );
        }
    }
    let (current, snapshot) = hub.current();
    let Some(snapshot) = snapshot else {
        // Nothing has been sampled yet: that is not a fleet, and not a failure.
        return Response::ok(
            &request.id,
            json!(ObservationGetResponse::not_modified(epoch, current)),
        );
    };
    if !snapshot.fits() {
        return Response::ok(
            &request.id,
            json!(ObservationGetResponse::too_large(
                snapshot.epoch.clone(),
                snapshot.revision,
                crate::control_plane::observation::MAX_SNAPSHOT_BYTES,
            )),
        );
    }
    let value = match serde_json::to_value(ObservationGetResponse::snapshot(&snapshot)) {
        Ok(value) => value,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Internal,
                    format!("the observation snapshot is not serializable: {error}"),
                ),
            );
        }
    };
    Response::ok(&request.id, value)
}

/// Serves one mux read from the backend.
///
/// A read records nothing: nothing here can be executed later, so a failure is
/// only ever this answer, and the caller is told which part of the backend is
/// missing rather than given an empty result it might believe.
pub(super) fn read_backend(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    if request.method == protocol::OUTPUT_METHOD {
        return read_output(core, request);
    }
    if request.method == protocol::OBSERVE_METHOD
        && !request.params.as_object().is_some_and(|o| o.is_empty())
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`{}` accepts no params", protocol::OBSERVE_METHOD),
            ),
        );
    }
    let pane = if request.method == protocol::PROCESS_INFO_METHOD {
        let pane = match request.required_string("pane_id") {
            Ok(pane) => pane,
            Err(refusal) => return Response::refused(&request.id, &refusal),
        };
        if request
            .params
            .as_object()
            .is_none_or(|object| object.keys().any(|key| key != "pane_id"))
        {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`{}` accepts only pane_id", protocol::PROCESS_INFO_METHOD),
                ),
            );
        }
        Some(pane)
    } else {
        if !request
            .params
            .as_object()
            .is_some_and(|object| object.is_empty())
        {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`{}` accepts no params", protocol::OBSERVE_METHOD),
                ),
            );
        }
        None
    };
    let (backend, cancel) = {
        let core = locked(core);
        (core.backend.clone(), Arc::clone(&core.stopping))
    };
    let Some(backend) = backend else {
        let (code, message) = ops::read_unavailable(&request.method);
        return Response::refused(&request.id, &Refusal::new(code, message));
    };
    if let Some(refusal) = primitive_refuse(&backend, &request.method) {
        return Response::refused(&request.id, &refusal);
    }
    if let Some(pane_id) = pane {
        if !valid_identifier(&pane_id) {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BadParams, "`pane_id` is not a valid identifier"),
            );
        }
        let evidence = backend.foreground_evidence(&pane_id, &cancel);
        if cancel.load(Ordering::SeqCst) {
            Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "daemon is stopping"),
            )
        } else {
            Response::ok(&request.id, json!({"evidence": evidence}))
        }
    } else {
        match backend.inventory(&cancel) {
            Ok(_) if cancel.load(Ordering::SeqCst) => Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "daemon is stopping"),
            ),
            Ok(inventory) => Response::ok(&request.id, json!({"inventory": inventory})),

            Err(error) => {
                Response::refused(&request.id, &Refusal::new(Code::BackendUnavailable, error))
            }
        }
    }
}

/// Serves one bounded output read of one pane.
///
/// The snapshot is bounded here as well as by the backend: an answer the daemon
/// cannot write inside its deadline is no answer, so the text is cut to what one
/// line can carry and the `truncated` flag says so.
pub(super) fn read_output(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let allowed: &[&str] = &["pane_id", "source", "lines", "ansi"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "output accepts only pane_id, source, lines and ansi",
            ),
        );
    }
    let output: OutputRequest = match serde_json::from_value(request.params.clone()) {
        Ok(output) => output,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`output` params are unusable: {error}"),
                ),
            );
        }
    };
    if !valid_identifier(&output.pane_id) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`pane_id` is not a valid identifier"),
        );
    }
    if output
        .lines
        .is_some_and(|lines| lines == 0 || lines > MAX_OUTPUT_LINES)
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`lines` must be between 1 and {MAX_OUTPUT_LINES}"),
            ),
        );
    }
    let (backend, cancel) = {
        let core = locked(core);
        (core.backend.clone(), Arc::clone(&core.stopping))
    };
    let Some(backend) = backend else {
        let (code, message) = ops::read_unavailable(&request.method);
        return Response::refused(&request.id, &Refusal::new(code, message));
    };
    if let Some(refusal) = primitive_refuse(&backend, &request.method) {
        return Response::refused(&request.id, &refusal);
    }
    match backend.output(&output, &cancel) {
        OutputOutcome::Completed(_) if cancel.load(Ordering::SeqCst) => Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, "daemon is stopping"),
        ),
        OutputOutcome::Completed(read) => {
            Response::ok(&request.id, json!({ "output": bounded(read) }))
        }
        OutputOutcome::Refused(message) => {
            Response::refused(&request.id, &Refusal::new(Code::Refused, message))
        }
        OutputOutcome::Unknown(message) => Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, message),
        ),
    }
}

/// One read with its text cut to what a single answer line carries.
pub(super) fn bounded(read: OutputRead) -> OutputRead {
    if read.text.len() <= MAX_OUTPUT_BYTES {
        return read;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !read.text.is_char_boundary(end) {
        end -= 1;
    }
    OutputRead {
        text: read.text[..end].to_string(),
        truncated: true,
        revision: read.revision,
    }
}

/// The public operation projection omits launch environment values while the
/// private stored record retains them for exact replay comparison.
pub(super) fn public_record(record: RequestRecord) -> serde_json::Value {
    let mut value = serde_json::to_value(record).expect("request records serialize");
    if let Some(params) = value.get_mut("params") {
        if let Some(request) = params.get_mut("request")
            && let Some(object) = request.as_object_mut()
        {
            object.remove("env");
            object.remove("argv");
        }
        if let Some(object) = params.as_object_mut() {
            object.remove("env");
        }
    }
    value
}

/// One record, by id.
pub(super) fn read_one(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let id = match request.required_string("id") {
        Ok(id) => id,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let core = locked(core);
    match core.store.get(&id) {
        Ok(Some(record)) => Response::ok(&request.id, json!({ "request": public_record(record) })),
        Ok(None) => Response::refused(
            &request.id,
            &Refusal::new(Code::NotFound, format!("no request {id}")),
        ),
        Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
    }
}

pub(super) const DEFAULT_TASKS_LIMIT: usize = 100;
pub(super) const MAX_TASKS_LIMIT: usize = 256;

pub(super) fn read_tasks(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let params: TasksListParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let limit = params.limit.unwrap_or(DEFAULT_TASKS_LIMIT);
    if limit == 0 || limit > MAX_TASKS_LIMIT {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`limit` must be between 1 and {MAX_TASKS_LIMIT}"),
            ),
        );
    }
    let (ingest, error) = {
        let core = locked(core);
        (core.task_ingest.clone(), core.task_ingest_error.clone())
    };
    let Some(ingest) = ingest else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                error.unwrap_or_else(|| "task ingest is not configured".to_string()),
            ),
        );
    };
    let (revision, sessions, next_after) = ingest.list(params.after.as_deref(), limit);
    Response::ok(
        &request.id,
        json!({"revision": revision, "sessions": sessions, "next_after": next_after}),
    )
}

/// Recent records, newest first, bounded.
pub(super) const DEFAULT_EVENT_LIMIT: usize = 100;
pub(super) const MAX_EVENT_LIMIT: usize = 500;
pub(super) const MAX_EVENT_WAIT_MS: u64 = 1_000;
pub(super) const MAX_EVENTS_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EventsReadParams {
    #[serde(default)]
    pub(super) after: u64,
    #[serde(default)]
    pub(super) epoch: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) wait_ms: Option<u64>,
}

pub(super) fn event_read_ready(store: &Store, line: &[u8]) -> bool {
    let Ok(request) = serde_json::from_slice::<serde_json::Value>(line) else {
        return true;
    };
    if request.get("method").and_then(serde_json::Value::as_str) != Some("events.read") {
        return true;
    }
    let Some(params) = request.get("params") else {
        return true;
    };
    let Ok(params) = serde_json::from_value::<EventsReadParams>(params.clone()) else {
        return true;
    };
    let wait_ms = params.wait_ms.unwrap_or_default();
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if wait_ms == 0 || wait_ms > MAX_EVENT_WAIT_MS || limit == 0 || limit > MAX_EVENT_LIMIT {
        return true;
    }
    store.event_ready(params.after, params.epoch.as_deref())
}

pub(super) fn read_events(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    stopping: &Arc<AtomicBool>,
) -> Response {
    let params: EventsReadParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if limit == 0 || limit > MAX_EVENT_LIMIT {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`limit` must be between 1 and {MAX_EVENT_LIMIT}"),
            ),
        );
    }
    let wait = params.wait_ms.unwrap_or(0);
    if wait > MAX_EVENT_WAIT_MS {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`wait_ms` must not exceed {MAX_EVENT_WAIT_MS}"),
            ),
        );
    }
    let after = params.after;
    let requested_epoch = params.epoch.as_deref();
    let store = {
        let core = locked(core);
        core.store.clone()
    };
    let deadline = Instant::now() + Duration::from_millis(wait);
    loop {
        let generation = store.event_generation();
        let (epoch, tail, earliest, events, delivery_pending) = match store.event_snapshot() {
            Ok(cursor) => cursor,
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        };
        // Expiry is measured against the journal head: an `after` past it names
        // a position this journal never had.
        if requested_epoch.is_some_and(|requested| requested != epoch)
            || after > tail
            || after.saturating_add(1) < earliest
        {
            return Response::ok(
                &request.id,
                json!({"cursor_expired": {
                    "epoch": epoch, "cursor": {"epoch": epoch, "sequence": tail}, "earliest": earliest
                }}),
            );
        }
        // The cursor is where a consumer resumes, so it names the last event
        // actually delivered — or the request's own `after` when this answer
        // delivered none. The head is reported separately, and `more` says
        // whether a resumed read has anything left: a bounded prefix and a byte
        // bound can no longer look like an empty journal.
        let (available, cursor) = deliverable(events, after, limit, MAX_EVENTS_RESPONSE_BYTES);
        let more = tail > cursor || delivery_pending;
        if !available.is_empty()
            || wait == 0
            || stopping.load(Ordering::SeqCst)
            || Instant::now() >= deadline
        {
            return Response::ok(
                &request.id,
                json!({
                    "epoch": epoch, "events": available, "cursor": {"epoch": epoch, "sequence": cursor}, "tail": tail, "more": more, "earliest": earliest
                }),
            );
        }
        // Long polls occupy a bounded connection worker, but no Core/store lock.
        // Short condvar slices make stop cancellation prompt without starving joins.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = store.wait_for_event(generation, remaining.min(Duration::from_millis(25)));
    }
}

/// The prefix one `events.read` answer carries, and the position a consumer
/// resumes from.
///
/// The prefix is bounded by the caller's `limit` and by the bytes one answer
/// line may hold; the cursor is the last event actually delivered, so following
/// it can never skip an event either bound held back.
pub(super) fn deliverable(
    events: Vec<store::Event>,
    after: u64,
    limit: usize,
    bytes: usize,
) -> (Vec<store::Event>, u64) {
    let mut available = Vec::new();
    let mut used = 0usize;
    for event in events
        .into_iter()
        .filter(|event| event.seq > after)
        .take(limit)
    {
        used += serde_json::to_vec(&event).map_or(0, |encoded| encoded.len());
        if used > bytes {
            break;
        }
        available.push(event);
    }
    let cursor = available.last().map_or(after, |event| event.seq);
    (available, cursor)
}

pub(super) fn read_many(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    // Every record not yet completed, however old: a stop gate must see those,
    // and the recency window below cannot show them.
    if request
        .params
        .get("unfinished")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        let core = locked(core);
        return match core.store.unfinished() {
            Ok(records) => Response::ok(
                &request.id,
                json!({ "requests": records.into_iter().take(MAX_LISTED).map(public_record).collect::<Vec<_>>() }),
            ),
            Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
        };
    }
    let limit = match request.limit(DEFAULT_LISTED, MAX_LISTED) {
        Ok(limit) => limit,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let core = locked(core);
    match core.store.recent(limit) {
        Ok(records) => Response::ok(
            &request.id,
            json!({ "requests": records.into_iter().map(public_record).collect::<Vec<_>>() }),
        ),
        Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
    }
}
