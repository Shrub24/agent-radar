use super::*;

/// Validates, records and dispatches one pane input.
///
/// Input is what a client asks a pane to receive: literal text, or named key
/// presses, in one of the two shapes and never both. The payload is validated
/// here, at the wire boundary, before anything is recorded or dispatched; what
/// survives validation is input this seam can carry without deciding for the
/// caller what a terminal would do with it.
pub(super) fn execute_input(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let input: InputRequest = match serde_json::from_value(frozen) {
        Ok(input) => input,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not an input request: {error}"),
                ),
            );
        }
    };
    if let Err(message) = input.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let effect = match &input.payload {
        InputPayload::Text { .. } => "sent text",
        InputPayload::Keys { .. } => "sent keys",
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let target = input.pane_id.clone();
    let now = store::now_ms();
    let operation = ops::Operation::Input;
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
    let done = match backend.input(&input, &cancel) {
        InputOutcome::Completed => record.complete(
            Settlement::completed(vec![effect.to_string()]),
            store::now_ms(),
        ),
        InputOutcome::Refused(message) => {
            record.complete(Settlement::backend_refused(&message), store::now_ms())
        }
        InputOutcome::Unknown(message) => {
            record.complete(Settlement::backend_unknown(&message), store::now_ms())
        }
    };
    write_settled(request, done, core)
}

/// Validates, records and dispatches one publisher's report.
///
/// A report is a recorded mutation like any other — admitted once, started before
/// dispatch, never replayed — but it changes no mux state: it carries one
/// publisher's own facts to the backend as the caller gave them. Nothing here
/// derives an agent fact, merges the report into a store or claims a location; the
/// record keeps the request's own params, so what a client reads back is what it
/// reported. The record's target is the location reported about, so a report
/// conflicts with another mutation of that pane instead of racing it.
///
/// Where the backend cannot report, the primitive gate refuses before dispatch
/// and the record says so, rather than the daemon substituting an operation.
pub(super) fn execute_report(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let report: ReportRequest = match serde_json::from_value(frozen) {
        Ok(report) => report,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a report: {error}"),
                ),
            );
        }
    };
    if let Err(message) = report.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let target = report.target().to_string();
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Report;
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
    let done = match backend.report(&report, &cancel) {
        ReportOutcome::Completed => record.complete(
            Settlement::completed(vec![report.effect()]),
            store::now_ms(),
        ),
        ReportOutcome::Refused(message) => {
            record.complete(Settlement::backend_refused(&message), store::now_ms())
        }
        ReportOutcome::Unknown(message) => {
            record.complete(Settlement::backend_unknown(&message), store::now_ms())
        }
    };
    write_settled(request, done, core)
}
