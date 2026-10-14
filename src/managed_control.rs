//! Subject-targeted lifecycle controls for the TUI.
//!
//! One confirmed request runs off the UI thread and keeps its request id for
//! readback. An unanswered request is unknown; the client never retries it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::control_plane::client::{LifecycleAnswer, LifecycleControlRequest, LifecycleControls};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Cancel,
    Stop,
    Restart,
    /// Makes a registered lead the daemon did not launch a lifecycle subject.
    Enroll,
}

impl Operation {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Enroll => "enroll",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub operation: Operation,
    pub request_id: String,
    pub subject_id: String,
    pub child_close: Option<crate::registry_observation::ChildCloseTarget>,
    pub close_request_id: Option<String>,
    /// The registration and session an enrollment names. `subject_id` is the
    /// registration's agent id then, because no subject exists yet.
    pub enrollment: Option<Enrollment>,
}

/// What the operator saw and confirmed for an enrollment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enrollment {
    pub agent_id: String,
    pub session: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub operation: Operation,
    pub request_id: String,
    pub subject_id: String,
    pub result: Result<LifecycleAnswer, String>,
    pub close_request_id: Option<String>,
    pub close_result: Option<Result<LifecycleAnswer, String>>,
}

struct Execution {
    primary: Result<LifecycleAnswer, String>,
    close: Option<Result<LifecycleAnswer, String>>,
}

fn recover(
    controls: &dyn LifecycleControls,
    request_id: &str,
    result: Result<LifecycleAnswer, String>,
    cancel: &AtomicBool,
) -> Result<LifecycleAnswer, String> {
    match result {
        Ok(answer) => Ok(answer),
        Err(original) => match controls.read_operation(request_id, cancel) {
            Ok(Some(answer)) => Ok(answer),
            Ok(None) => Err(original),
            Err(readback) => Err(format!("{original}; readback unavailable: {readback}")),
        },
    }
}

fn execute(
    controls: &dyn LifecycleControls,
    dispatched: &Control,
    cancel: &AtomicBool,
) -> Execution {
    let request = LifecycleControlRequest {
        request_id: dispatched.request_id.clone(),
        subject_id: dispatched.subject_id.clone(),
    };
    let primary = match (dispatched.operation, dispatched.enrollment.as_ref()) {
        (Operation::Enroll, Some(enrollment)) => controls.enroll_agent(
            &dispatched.request_id,
            &enrollment.agent_id,
            &enrollment.session,
            cancel,
        ),
        (Operation::Enroll, None) => Err("an enrollment names no registration".to_string()),
        (operation, _) => match operation {
            Operation::Cancel => controls.cancel_subject(&request, cancel),
            Operation::Stop => controls.stop_subject(&request, cancel),
            Operation::Restart => controls.restart_subject(&request, cancel),
            Operation::Enroll => unreachable!("matched above"),
        },
    };
    let primary = recover(controls, &dispatched.request_id, primary, cancel);
    let close = match (
        dispatched.operation,
        primary
            .as_ref()
            .ok()
            .is_some_and(|answer| answer.outcome() == "completed"),
        dispatched.child_close.as_ref(),
        dispatched.close_request_id.as_deref(),
    ) {
        (Operation::Cancel, true, Some(target), Some(close_id)) => Some(recover(
            controls,
            close_id,
            controls.close_child(close_id, target, cancel),
            cancel,
        )),
        _ => None,
    };
    Execution { primary, close }
}

struct Request {
    outcome: Receiver<Execution>,
    worker: JoinHandle<()>,
}

pub struct LifecycleControl {
    controls: Option<Arc<dyn LifecycleControls>>,
    in_flight: Option<(Control, Request)>,
}

impl LifecycleControl {
    pub fn new(controls: Option<Arc<dyn LifecycleControls>>) -> Self {
        Self {
            controls,
            in_flight: None,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    pub fn start(&mut self, control: Control) -> Result<(), String> {
        if self.in_flight.is_some() {
            return Err("another lifecycle control is still running".into());
        }
        let Some(controls) = self.controls.clone() else {
            return Err("this run has no daemon lifecycle controls".into());
        };
        if control.child_close.is_some() && !controls.supports_child_close() {
            return Err("the daemon cannot serve spawn.close.v1; no process was cancelled".into());
        }
        let (sender, outcome) = mpsc::channel();
        let worker = {
            let dispatched = control.clone();
            thread::Builder::new()
                .name(format!("radar-lifecycle-{}", control.operation.verb()))
                .spawn(move || {
                    let cancel = AtomicBool::new(false);
                    let execution = execute(controls.as_ref(), &dispatched, &cancel);
                    let _ = sender.send(execution);
                })
                .expect("lifecycle control thread")
        };
        self.in_flight = Some((control, Request { outcome, worker }));
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Finished> {
        let (_, request) = self.in_flight.as_mut()?;
        let Execution {
            primary: result,
            close: close_result,
        } = match request.outcome.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Execution {
                primary: Err("the lifecycle request stopped unexpectedly".to_string()),
                close: None,
            },
        };
        let (control, request) = self.in_flight.take().expect("checked above");
        let _ = request.worker.join();
        Some(Finished {
            operation: control.operation,
            request_id: control.request_id,
            subject_id: control.subject_id,
            result,
            close_request_id: control.close_request_id,
            close_result,
        })
    }

    pub fn shutdown(&mut self) {
        if let Some((_, request)) = self.in_flight.take() {
            // Finish the bounded cancel/readback and optional close calls before
            // process exit; interrupting here could leave the pane uncleaned.
            let _ = request.worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    use super::*;

    struct FakeControls {
        calls: Mutex<Vec<String>>,
        cancel_outcome: &'static str,
        cancel_loses_reply: bool,
        can_close_child: bool,
    }

    impl FakeControls {
        fn answer(outcome: &str) -> LifecycleAnswer {
            LifecycleAnswer {
                request: serde_json::json!({"outcome": outcome}),
            }
        }
    }

    impl LifecycleControls for FakeControls {
        fn supports_child_close(&self) -> bool {
            self.can_close_child
        }

        fn read_operation(
            &self,
            request_id: &str,
            _: &AtomicBool,
        ) -> Result<Option<LifecycleAnswer>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("read:{request_id}"));
            Ok(self
                .cancel_loses_reply
                .then(|| Self::answer(self.cancel_outcome)))
        }

        fn close_child(
            &self,
            request_id: &str,
            target: &crate::registry_observation::ChildCloseTarget,
            _: &AtomicBool,
        ) -> Result<LifecycleAnswer, String> {
            self.calls.lock().unwrap().push(format!(
                "close:{request_id}:{}:{}:{}",
                target.spawn_request_id, target.source, target.incarnation
            ));
            Ok(Self::answer("completed"))
        }

        fn cancel_subject(
            &self,
            request: &LifecycleControlRequest,
            cancel: &AtomicBool,
        ) -> Result<LifecycleAnswer, String> {
            if cancel.load(Ordering::SeqCst) {
                return Err("TUI shutdown interrupted the operation".into());
            }
            self.calls.lock().unwrap().push(format!(
                "cancel:{}:{}",
                request.request_id, request.subject_id
            ));
            if self.cancel_loses_reply {
                Err("reply lost".into())
            } else {
                Ok(Self::answer(self.cancel_outcome))
            }
        }

        fn stop_subject(
            &self,
            request: &LifecycleControlRequest,
            cancel: &AtomicBool,
        ) -> Result<LifecycleAnswer, String> {
            self.cancel_subject(request, cancel)
        }

        fn restart_subject(
            &self,
            request: &LifecycleControlRequest,
            cancel: &AtomicBool,
        ) -> Result<LifecycleAnswer, String> {
            self.cancel_subject(request, cancel)
        }

        fn enroll_agent(
            &self,
            request_id: &str,
            agent_id: &str,
            session: &str,
            _: &AtomicBool,
        ) -> Result<LifecycleAnswer, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("enroll:{request_id}:{agent_id}:{session}"));
            Ok(Self::answer("completed"))
        }
    }

    #[test]
    fn an_enrollment_names_its_registration_and_session_and_never_cancels() {
        let controls = FakeControls {
            calls: Mutex::new(Vec::new()),
            cancel_outcome: "completed",
            cancel_loses_reply: false,
            can_close_child: true,
        };
        let enroll = Control {
            operation: Operation::Enroll,
            request_id: "enroll-id".into(),
            subject_id: "agent-1".into(),
            child_close: None,
            close_request_id: None,
            enrollment: Some(Enrollment {
                agent_id: "agent-1".into(),
                session: "11111111-1111-4111-8111-111111111111".into(),
            }),
        };
        let execution = execute(&controls, &enroll, &AtomicBool::new(false));
        assert_eq!(execution.primary.unwrap().outcome(), "completed");
        assert!(execution.close.is_none());
        assert_eq!(
            *controls.calls.lock().unwrap(),
            ["enroll:enroll-id:agent-1:11111111-1111-4111-8111-111111111111"]
        );
        // An enrollment that names no registration sends nothing.
        let none = Control {
            enrollment: None,
            ..enroll
        };
        let execution = execute(&controls, &none, &AtomicBool::new(false));
        assert!(execution.primary.is_err());
        // The failure is read back by its id, but no second enrollment is sent.
        assert_eq!(
            controls
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("enroll:"))
                .count(),
            1
        );
    }

    fn control() -> Control {
        Control {
            operation: Operation::Cancel,
            request_id: "cancel-id".into(),
            subject_id: "subject-id".into(),
            child_close: Some(crate::registry_observation::ChildCloseTarget {
                spawn_request_id: "spawn-id".into(),
                source: "worker".into(),
                incarnation: "11111111-1111-4111-8111-111111111111".into(),
            }),
            close_request_id: Some("close-id".into()),
            enrollment: None,
        }
    }

    #[test]
    fn missing_spawn_close_contract_refuses_before_process_cancellation() {
        let controls = Arc::new(FakeControls {
            calls: Mutex::new(Vec::new()),
            cancel_outcome: "completed",
            cancel_loses_reply: false,
            can_close_child: false,
        });
        let mut worker = LifecycleControl::new(Some(controls.clone()));
        let error = worker
            .start(control())
            .expect_err("close contract is required first");
        assert!(error.contains("no process was cancelled"));
        assert!(controls.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn cancellation_precedes_the_separate_spawn_edge_pane_close() {
        let controls = FakeControls {
            calls: Mutex::new(Vec::new()),
            cancel_outcome: "completed",
            cancel_loses_reply: false,
            can_close_child: true,
        };
        let execution = execute(&controls, &control(), &AtomicBool::new(false));
        assert_eq!(execution.primary.unwrap().outcome(), "completed");
        assert_eq!(execution.close.unwrap().unwrap().outcome(), "completed");
        assert_eq!(
            *controls.calls.lock().unwrap(),
            [
                "cancel:cancel-id:subject-id",
                "close:close-id:spawn-id:worker:11111111-1111-4111-8111-111111111111"
            ]
        );
    }

    #[test]
    fn a_confirmed_operation_and_pane_cleanup_finish_during_tui_shutdown() {
        let controls = Arc::new(FakeControls {
            calls: Mutex::new(Vec::new()),
            cancel_outcome: "completed",
            cancel_loses_reply: false,
            can_close_child: true,
        });
        let mut worker = LifecycleControl::new(Some(controls.clone()));
        worker.start(control()).expect("start confirmed control");
        worker.shutdown();
        assert_eq!(
            *controls.calls.lock().unwrap(),
            [
                "cancel:cancel-id:subject-id",
                "close:close-id:spawn-id:worker:11111111-1111-4111-8111-111111111111"
            ]
        );
    }

    #[test]
    fn a_lost_cancel_reply_is_read_back_before_cleanup_without_reissuing_cancel() {
        let controls = FakeControls {
            calls: Mutex::new(Vec::new()),
            cancel_outcome: "completed",
            cancel_loses_reply: true,
            can_close_child: true,
        };
        let execution = execute(&controls, &control(), &AtomicBool::new(false));
        assert_eq!(execution.primary.unwrap().outcome(), "completed");
        assert_eq!(execution.close.unwrap().unwrap().outcome(), "completed");
        assert_eq!(
            *controls.calls.lock().unwrap(),
            [
                "cancel:cancel-id:subject-id",
                "read:cancel-id",
                "close:close-id:spawn-id:worker:11111111-1111-4111-8111-111111111111"
            ]
        );
    }

    #[test]
    fn refused_cancellation_never_dispatches_spawn_close() {
        let controls = FakeControls {
            calls: Mutex::new(Vec::new()),
            cancel_outcome: "refused",
            cancel_loses_reply: false,
            can_close_child: true,
        };
        let execution = execute(&controls, &control(), &AtomicBool::new(false));
        assert_eq!(execution.primary.unwrap().outcome(), "refused");
        assert!(execution.close.is_none());
        assert_eq!(
            *controls.calls.lock().unwrap(),
            ["cancel:cancel-id:subject-id"]
        );
    }
}
