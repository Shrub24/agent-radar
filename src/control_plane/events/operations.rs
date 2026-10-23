use super::{Event, Log, Source, Transition, VERSION};
use crate::control_plane::store::{RecordState, RequestRecord, random_uuid};
use crate::model::SessionUuid;

pub fn managed(method: &str) -> bool {
    matches!(
        method,
        "lifecycle.cancel" | "lifecycle.stop" | "lifecycle.enroll" | "lifecycle.restart"
    )
}

fn public(mut record: RequestRecord) -> RequestRecord {
    record.params = None;
    record
}

impl Log {
    pub fn operation_ids(&self) -> impl Iterator<Item = &String> {
        self.operations.keys()
    }

    pub fn operation(&self, id: &str) -> Option<&RequestRecord> {
        self.operations.get(id)
    }

    pub fn record_operation(&mut self, record: &RequestRecord) -> Result<(), String> {
        let subject_id = record
            .params
            .as_ref()
            .and_then(|params| params.get("request"))
            .and_then(|request| request.get("subject_id"))
            .and_then(|id| id.as_str())
            .filter(|id| SessionUuid::parse(id).is_some())
            .map(str::to_owned);
        let record = public(record.clone());
        if self.operations.get(&record.id) == Some(&record) {
            return Ok(());
        }
        let transition = match record.state {
            RecordState::Pending | RecordState::Claimed => {
                Transition::OperationAdmitted(record.clone())
            }
            RecordState::Started => Transition::OperationStarted(record.clone()),
            RecordState::Completed => Transition::OperationSettled(record.clone()),
        };
        let event = Event {
            version: VERSION,
            seq: self.next_seq,
            event_id: random_uuid(),
            subject_id,
            operation_id: Some(record.id.clone()),
            causation_id: self.last_operation_event.get(&record.id).cloned(),
            at: record
                .completed_at
                .clone()
                .unwrap_or_else(crate::control_plane::store::now),
            source: Source::ProcessControl,
            transition,
        };
        self.validate_operation(&event)?;
        self.persist(&event)?;
        self.replay_operation(&event)
    }

    fn validate_operation<'a>(&self, event: &'a Event) -> Result<&'a RequestRecord, String> {
        let record = match &event.transition {
            Transition::OperationAdmitted(record)
            | Transition::OperationStarted(record)
            | Transition::OperationSettled(record) => record,
            _ => return Err("operation event has no operation payload".into()),
        };
        if event.operation_id.as_deref() != Some(&record.id)
            || SessionUuid::parse(&record.id).is_none()
            || !managed(&record.method)
            || record.params.is_some()
        {
            return Err("invalid or private operation event payload".into());
        }
        if event
            .causation_id
            .as_ref()
            .is_some_and(|id| self.last_operation_event.get(&record.id) != Some(id))
        {
            return Err("operation event causation is invalid".into());
        }
        let prior = self.operations.get(&record.id);
        if let Some(prior) = prior
            && (prior.method != record.method
                || prior.requester != record.requester
                || prior.target != record.target
                || prior.requested_at != record.requested_at
                || prior.expires_at != record.expires_at)
        {
            return Err("operation event changes its admitted identity".into());
        }
        let valid = match (&event.transition, prior) {
            (Transition::OperationAdmitted(_), None) => {
                record.state == RecordState::Pending && record.outcome.is_none()
            }
            (Transition::OperationAdmitted(_), Some(prior)) => {
                prior.state == RecordState::Pending
                    && record.state == RecordState::Claimed
                    && record.outcome.is_none()
            }
            (Transition::OperationStarted(_), Some(prior)) => {
                matches!(prior.state, RecordState::Pending | RecordState::Claimed)
                    && record.state == RecordState::Started
                    && record.outcome.is_none()
            }
            (Transition::OperationSettled(_), prior) => {
                record.state == RecordState::Completed
                    && record.outcome.is_some()
                    && record.completed_at.is_some()
                    && prior.is_none_or(|prior| {
                        prior.state != RecordState::Completed
                            || prior.outcome
                                == Some(crate::control_plane::store::RequestOutcome::Unknown)
                    })
            }
            _ => false,
        };
        if !valid {
            return Err("invalid operation transition".into());
        }
        Ok(record)
    }

    pub(super) fn replay_operation(&mut self, event: &Event) -> Result<(), String> {
        let record = self.validate_operation(event)?.clone();
        self.last_operation_event
            .insert(record.id.clone(), event.event_id.clone());
        self.operations.insert(record.id.clone(), record);
        Ok(())
    }
}

/// Updates only public result fields. Private admission params stay in the
/// immutable request seed for exact same-ID replay comparisons.
pub fn apply(projection: &RequestRecord, seed: &mut RequestRecord) -> Result<(), String> {
    if projection.id != seed.id
        || projection.method != seed.method
        || projection.target != seed.target
        || projection.requester != seed.requester
        || projection.requested_at != seed.requested_at
        || projection.expires_at != seed.expires_at
    {
        return Err("private operation seed does not match the event projection".into());
    }
    seed.state = projection.state;
    seed.outcome = projection.outcome;
    seed.category = projection.category.clone();
    seed.message = projection.message.clone();
    seed.effects = projection.effects.clone();
    seed.created = projection.created.clone();
    seed.launch_request_id = projection.launch_request_id.clone();
    seed.completed_at = projection.completed_at.clone();
    Ok(())
}
