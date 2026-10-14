//! The bounded public pointer journal is a subscriber, not the log's authority.
//! Its source checkpoint and delivered pointers commit together, so a failed
//! delivery retries from the same authoritative events without duplicating them.
//! Container transitions are delivered as pointers too: a subscriber learns that
//! a container changed and reads the record, exactly as it does for a subject.

use super::{Event, MAX_EVENT_BYTES, MAX_EVENTS, Store};
use crate::control_plane::events::{Event as LifecycleEvent, Log, Transition};
use crate::control_plane::lifecycle::Origin;

const DELIVERY_BATCH: usize = 256;

impl Store {
    pub(crate) fn event_ready(&self, after: u64, epoch: Option<&str>) -> bool {
        match self.event_snapshot() {
            Ok((current, tail, earliest, _, pending)) => {
                epoch.is_some_and(|requested| requested != current)
                    || after > tail
                    || after.saturating_add(1) < earliest
                    || tail > after
                    || pending
            }
            Err(_) => true,
        }
    }

    pub(crate) fn event_snapshot(&self) -> Result<(String, u64, u64, Vec<Event>, bool), String> {
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut journal = self.load_journal()?;
        let log = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if journal.source_seq > log.tail() {
            return Err("delivery checkpoint exceeds its source log".into());
        }
        let events = log.read_after(journal.source_seq, DELIVERY_BATCH)?;
        if !events.is_empty() {
            for source in &events {
                let mut event = pointer(source, &log);
                event.seq = journal.next_seq;
                journal.next_seq += 1;
                journal.events.push(event);
                journal.source_seq = source.seq;
            }
            let mut bytes =
                serde_json::to_vec(&journal).map_err(|_| "cannot encode event delivery")?;
            while journal.events.len() > MAX_EVENTS || bytes.len() > MAX_EVENT_BYTES {
                if journal.events.len() <= 1 {
                    break;
                }
                journal.events.remove(0);
                bytes = serde_json::to_vec(&journal).map_err(|_| "cannot encode event delivery")?;
            }
            self.write_journal(&journal)?;
        }
        let pending = journal.source_seq < log.tail();
        let tail = journal.next_seq.saturating_sub(1);
        let earliest = journal
            .events
            .first()
            .map_or(journal.next_seq, |event| event.seq);
        Ok((journal.epoch, tail, earliest, journal.events, pending))
    }
}

fn pointer(event: &LifecycleEvent, log: &Log) -> Event {
    let kind = match &event.transition {
        Transition::Created(_) => "lifecycle.created",
        Transition::Enrolled(_) => "lifecycle.enrolled",
        Transition::Imported(_) => "lifecycle.imported",
        Transition::Captured(_) => "lifecycle.captured",
        Transition::Bound { .. } => "lifecycle.bound",
        Transition::Revoked => "lifecycle.revoked",
        Transition::SignalRequested => "lifecycle.signal_requested",
        Transition::ExitVerified => "lifecycle.exited",
        Transition::Observed(_) => "lifecycle.observed",
        Transition::PaneCleanup(_) => "lifecycle.pane_cleanup",
        Transition::SuccessorReserved { .. } => "lifecycle.successor_reserved",
        Transition::OperationAdmitted(_) => "lifecycle.operation.admitted",
        Transition::OperationStarted(_) => "lifecycle.operation.started",
        Transition::OperationSettled(_) => "lifecycle.settled",
        Transition::OperationImported(_) => "lifecycle.operation.imported",
        Transition::ContainerCreated(_) => "container.created",
        Transition::ContainerObserved(_) => "container.observed",
        Transition::ContainerRemoved => "container.removed",
    };
    let facts = event.subject_id.as_deref().and_then(|id| log.facts(id));
    let agent_id = match &event.transition {
        Transition::Bound { agent_id } => Some(agent_id.clone()),
        Transition::Enrolled(facts) | Transition::Imported(facts) => facts.bound_agent_id.clone(),
        _ => None,
    };
    Event {
        seq: 0,
        kind: kind.into(),
        subject_id: event.subject_id.clone(),
        lifecycle_event_id: Some(event.event_id.clone()),
        agent_id,
        spawn: facts
            .filter(|facts| facts.origin == Origin::Spawned)
            .map(|facts| facts.spawn_request_id.clone()),
        request_id: event
            .operation_id
            .clone()
            .or_else(|| match &event.transition {
                Transition::Enrolled(facts) => Some(facts.spawn_request_id.clone()),
                Transition::SuccessorReserved { spawn_request_id } => {
                    Some(spawn_request_id.clone())
                }
                _ => None,
            }),
        at: event.at.clone(),
    }
}
