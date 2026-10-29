use super::*;
use crate::control_plane::answer as answers;
use crate::control_plane::public_contracts::subject::{
    ChildGetParams, ChildrenParams, GetParams, GetResult, ListParams, ListResult, Selection,
};

pub(super) fn child_record(
    registry: &Registry,
    registration_id: &str,
    subject_id: &str,
) -> Result<subjects::SubjectRecord, Refusal> {
    let parent = registry::presence::attachment(registry, registration_id)
        .map_err(|error| Refusal::new(Code::Internal, error))?
        .ok_or_else(|| Refusal::new(Code::Refused, "caller has no granted subject"))?;
    let record = subjects::read_subject_record(registry, subject_id)
        .map_err(|error| Refusal::new(Code::Internal, error))?
        .ok_or_else(|| Refusal::new(Code::NotFound, "subject is not recorded"))?;
    if record.managed.parent_subject_id.as_deref() != Some(parent.subject_id.as_str()) {
        return Err(Refusal::new(
            Code::Refused,
            "subject is not this caller's child",
        ));
    }
    Ok(record)
}

pub(super) fn assignment_request(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    use crate::control_plane::public_contracts::subject::{
        AssignmentAcquire, AssignmentPublish, AssignmentRetire,
    };
    let registry = Arc::clone(&locked(core).registry);
    let _transition = registry.transition();
    let now = store::now_ms();
    macro_rules! child {
        ($params:expr) => {
            if let Err(refusal) =
                child_record(&registry, &$params.registration_id, &$params.subject_id)
            {
                return Response::refused(&request.id, &refusal);
            }
        };
    }
    match request.method.as_str() {
        "subject.assignment.acquire" => {
            let params: AssignmentAcquire = match decode_params(request) {
                Ok(p) => p,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            child!(params);
            match registry.acquire_writer(
                &AcquireRequest {
                    target_id: params.subject_id,
                    channel: Channel::Assignment,
                    publisher: params.publisher,
                    replace: params.replace,
                },
                now,
            ) {
                Ok(writer) => respond(&request.id, Ok(answers::Writer { writer })),
                Err(error) => registry_error(request, error),
            }
        }
        "subject.assignment.publish" => {
            let params: AssignmentPublish = match decode_params(request) {
                Ok(p) => p,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            child!(params);
            match registry.publish(
                &registry::PublishRequest {
                    target_id: params.subject_id,
                    channel: Channel::Assignment,
                    update: params.update,
                },
                now,
            ) {
                Ok(record) => respond(
                    &request.id,
                    Ok(answers::Channel::new(
                        record.view(now, registry.serving_epoch()),
                    )),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "subject.assignment.retire" => {
            let params: AssignmentRetire = match decode_params(request) {
                Ok(p) => p,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            child!(params);
            match registry.retire(
                &params.subject_id,
                Channel::Assignment,
                &params.writer_handle,
                now,
            ) {
                Ok(record) => respond(
                    &request.id,
                    Ok(answers::Channel::new(
                        record.view(now, registry.serving_epoch()),
                    )),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        _ => unreachable!("classified assignment operation"),
    }
}

pub(super) fn subject_read_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let (registry, verifier, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let deadline = Instant::now() + VERIFICATION_TIMEOUT;
    let project = |record| {
        crate::control_plane::subject_read::read(&registry, record, store::now_ms(), |process| {
            verify_bounded(process, &verifier, &stopping, verification, deadline)
        })
    };
    if matches!(
        request.method.as_str(),
        "subject.get" | "subject.child.get" | "subject.child.inspect"
    ) {
        let record = if request.method != "subject.get" {
            let params: ChildGetParams = match decode_params(request) {
                Ok(params) => params,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            match child_record(&registry, &params.registration_id, &params.subject_id) {
                Ok(record) => record,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            }
        } else {
            let params: GetParams = match decode_params(request) {
                Ok(params) => params,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            match subjects::read_subject_record(&registry, &params.subject_id) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::NotFound, "subject is not recorded"),
                    );
                }
                Err(error) => return registry_error(request, error),
            }
        };
        let claim = record.current_claim.clone();
        let projected = project(record);
        // A read that verified a process was already in flight when the daemon
        // started stopping: name the reason rather than answer with the
        // evidence a cancelled verification happened to leave.
        if stopping.load(Ordering::SeqCst) {
            return Response::refused(&request.id, &Refusal::new(Code::Busy, "daemon is stopping"));
        }
        return match projected {
            Ok(subject) if request.method == "subject.child.inspect" => {
                use crate::control_plane::public_contracts::subject::AttachmentState;
                let Some(pane) = subject
                    .location
                    .as_ref()
                    .and_then(|location| location.pane.as_ref())
                else {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::Refused, "child has no verified live pane"),
                    );
                };
                if subject.state != AttachmentState::Live {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::Refused, "child is not live"),
                    );
                }
                let output = match output_read(
                    core,
                    OutputRequest {
                        pane_id: pane.clone(),
                        source: crate::runtime::OutputSource::RecentUnwrapped,
                        lines: Some(80),
                        ansi: false,
                    },
                ) {
                    Ok(output) => output,
                    Err(refusal) => return Response::refused(&request.id, &refusal),
                };
                let foreground = foreground_read(core, pane).ok();
                let latest = match subjects::read_subject_record(&registry, &subject.subject_id) {
                    Ok(Some(record)) if record.current_claim == claim => project(record),
                    _ => {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(Code::Refused, "child changed during inspection"),
                        );
                    }
                };
                if !matches!(latest, Ok(ref latest) if latest.state == AttachmentState::Live && latest.location == subject.location)
                {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::Refused, "child changed during inspection"),
                    );
                }
                respond(
                    &request.id,
                    Ok(answers::Inspected {
                        subject,
                        output,
                        foreground,
                    }),
                )
            }
            Ok(subject) => respond(&request.id, Ok(GetResult { subject })),
            Err(error) => registry_error(request, error),
        };
    }
    let (params, parent) = if request.method == "subject.children" {
        let params: ChildrenParams = match decode_params(request) {
            Ok(params) => params,
            Err(refusal) => return Response::refused(&request.id, &refusal),
        };
        let attachment = match registry::presence::attachment(&registry, &params.registration_id) {
            Ok(Some(attachment)) => attachment,
            Ok(None) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::Refused, "caller has no granted subject"),
                );
            }
            Err(error) => return registry_error(request, error),
        };
        (
            ListParams {
                selection: params.selection.unwrap_or(Selection::All),
                after: params.after,
                limit: params.limit,
            },
            Some(attachment.subject_id),
        )
    } else {
        let params = match decode_params::<ListParams>(request) {
            Ok(params) => params,
            Err(refusal) => return Response::refused(&request.id, &refusal),
        };
        (params, None)
    };
    if params
        .limit
        .is_some_and(|limit| limit == 0 || limit > MAX_REGISTRY_PAGE)
    {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "limit must be between 1 and 100"),
        );
    }
    if let Some(after) = &params.after
        && let Err(error) = registry::validate_registration_id(after)
    {
        return registry_error(request, error);
    }
    let limit = params.limit.unwrap_or(50);
    let mut entries = Vec::new();
    let mut after = params.after;
    let mut used = 0;
    loop {
        let page =
            match subjects::subject_records_after(&registry, after.as_deref(), MAX_REGISTRY_PAGE) {
                Ok(page) => page,
                Err(error) => return registry_error(request, error),
            };
        let complete = page.len() < MAX_REGISTRY_PAGE;
        for record in page {
            after = Some(record.subject_id.clone());
            if parent
                .as_deref()
                .is_some_and(|parent| record.managed.parent_subject_id.as_deref() != Some(parent))
            {
                continue;
            }
            let entry = match project(record) {
                Ok(entry) => entry,
                Err(error) => return registry_error(request, error),
            };
            if !params.selection.includes(entry.state) {
                continue;
            }
            let size = match serde_json::to_vec(&entry) {
                Ok(bytes) => bytes.len(),
                Err(error) => return registry_error(request, error.to_string()),
            };
            if entries.len() == limit || used + size > MAX_LIST_RESPONSE_BYTES {
                if entries.is_empty() {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::Refused, "one subject exceeds the response bound"),
                    );
                }
                let next = entries.last().map(
                    |entry: &crate::control_plane::public_contracts::subject::Subject| {
                        entry.subject_id.clone()
                    },
                );
                return respond(
                    &request.id,
                    Ok(ListResult {
                        subjects: entries,
                        next,
                    }),
                );
            }
            used += size;
            entries.push(entry);
        }
        if complete {
            return respond(
                &request.id,
                Ok(ListResult {
                    subjects: entries,
                    next: None,
                }),
            );
        }
    }
}
