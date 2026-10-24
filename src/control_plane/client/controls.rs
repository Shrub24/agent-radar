//! The lifecycle and container controls the dashboard requests of the daemon.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::super::containers::PublicContainer;
use super::super::ops;

use super::*;

/// The container controls a daemon serves, for the TUI to request and read back.
///
/// Every call carries the caller's own request id, because the answer to a
/// confirmed control can be lost: the operator reads that id back rather than the
/// client dispatching a second effect. A control the daemon does not serve is
/// refused before it is sent, and nothing here falls back to another transport.
pub trait LifecycleControls: Send + Sync {
    fn supports_child_close(&self) -> bool;
    fn read_operation(
        &self,
        request_id: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<LifecycleAnswer>, String>;
    fn close_child(
        &self,
        request_id: &str,
        target: &crate::registry_observation::ChildCloseTarget,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    fn cancel_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    fn stop_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    fn restart_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    /// Enrolls a registered lead the daemon did not launch as a lifecycle subject.
    fn enroll_agent(
        &self,
        _request_id: &str,
        _registration_id: &str,
        _session: &str,
        _cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        Err("this run cannot enroll a lead".to_string())
    }

    /// Ends an unresolved record's recovery obligation, releasing only the
    /// administrative block it holds.
    ///
    /// The record is named rather than targeted: it is the one holding the target,
    /// so it cannot be reached through the target. Nothing is dispatched, and no
    /// outcome is established — the recorded uncertainty stands.
    fn abandon_record(
        &self,
        _request_id: &str,
        _held_request_id: &str,
        _cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        Err("this run cannot abandon a record".to_string())
    }
}

/// One subject-targeted request; pane ids are not lifecycle targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleControlRequest {
    pub request_id: String,
    pub subject_id: String,
    pub claim_id: String,
}

/// The operation record the daemon committed, without private launch data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleAnswer {
    pub request: Value,
}

impl LifecycleAnswer {
    pub fn outcome(&self) -> &str {
        self.request
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    }

    pub fn message(&self) -> Option<&str> {
        self.request.get("message").and_then(Value::as_str)
    }
}

pub trait ContainerControls: Send + Sync {
    /// Splits a pane slot from a pane, running a command in it when one is given.
    fn create_container(
        &self,
        request: &ContainerCreate,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String>;
    /// Deletes one container Radar records, as the operator confirmed it.
    fn delete_container(
        &self,
        request: &ContainerDelete,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String>;
    /// Runs a pane container's recorded recipe again in its own slot.
    fn reload_container(
        &self,
        request: &ContainerReload,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String>;
}

/// What one container creation makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerCreateKind {
    /// A new workspace, which needs no existing place to start from, starting in
    /// `directory` when the operator chose one.
    Workspace { directory: Option<String> },
    /// A tab in this workspace.
    Tab { workspace_id: String },
    /// A pane slot split from this pane.
    PaneSplit { pane_id: String },
}

/// A container creation, with Radar’s own name for the slot and the command it
/// runs.
///
/// No `Debug`: a recipe is private launch data, and printing one would put a
/// command and its environment in a diagnostic.
#[derive(Clone, PartialEq, Eq)]
pub struct ContainerCreate {
    /// The caller’s own id: the durable key of this operation.
    pub request_id: String,
    /// What the creation makes, and where.
    pub kind: ContainerCreateKind,
    pub label: Option<String>,
    /// The command the slot runs, in the daemon's own recipe shape. It is held
    /// privately by the daemon and never read back.
    pub recipe: Option<super::super::containers::Recipe>,
}

/// A container deletion: the container, and the target the operator confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerDelete {
    pub request_id: String,
    /// Set by a second `D` on the same place: an agent inside it held the first
    /// deletion, and the operator has now asked for it to be closed anyway.
    pub force: bool,
    pub container_id: String,
    /// The frozen target and occupant, as the daemon's close contract takes it:
    /// the daemon refuses the deletion when that place no longer holds it.
    pub frozen: Value,
}

/// A container reload: the pane container whose recorded recipe runs again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerReload {
    pub request_id: String,
    pub container_id: String,
}

/// What the daemon answered for one container control: its own record, the
/// container it made or acted on, and the containers a deletion removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerAnswer {
    pub request: Value,
    /// The container a creation made, or a reload acted on, unchanged.
    pub container: Option<PublicContainer>,
    /// The containers a deletion marked removed, in id order.
    pub removed: Vec<PublicContainer>,
}

impl std::fmt::Debug for ContainerCreate {
    /// Names what the control is, and never what it runs: a recipe is private
    /// launch data, so it is reported as held rather than printed.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerCreate")
            .field("request_id", &self.request_id)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("recipe", &self.recipe.as_ref().map(|_| "held"))
            .finish()
    }
}

impl ContainerAnswer {
    /// The outcome the record carries: `completed`, `refused` or `unknown`.
    pub fn outcome(&self) -> &str {
        self.request
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    }

    /// The category the record grouped its refusal under, when it wrote one.
    pub fn category(&self) -> Option<&str> {
        self.request.get("category").and_then(Value::as_str)
    }

    /// The record's own message, when it wrote one.
    pub fn message(&self) -> Option<&str> {
        self.request.get("message").and_then(Value::as_str)
    }
}

impl DaemonRuntime {
    fn lifecycle_control(
        &self,
        contract: ops::Contract,
        method: &str,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(contract) {
            return Err(self.describe(format!("does not provide `{}`", contract.as_str())));
        }
        let result = exchange_with_id_at(
            &self.socket,
            &request.request_id,
            method,
            json!({"request": {"subject_id": request.subject_id, "claim_id": request.claim_id}, "requester": REQUESTER}),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
        })
    }

    /// One container control, sent under the caller's own request id.
    ///
    /// The contract it needs is checked before the request is sent, so a caller
    /// refuses work this daemon does not serve instead of discovering it mid-effect.
    fn container_control(
        &self,
        contract: ops::Contract,
        request_id: &str,
        method: &str,
        params: Value,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        if !self.serves(contract) {
            return Err(self.describe(format!("does not provide `{}`", contract.as_str())));
        }
        let result = exchange_with_id_at(&self.socket, request_id, method, params, cancel)
            .map_err(|failure| failure.message)?;
        let container = self.public_container(result.get("container"))?;
        let removed = self.public_container_list(result.get("removed"))?;
        Ok(ContainerAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
            container,
            removed,
        })
    }
}

impl DaemonRuntime {
    /// One container the daemon named, when it named one.
    fn public_container(&self, value: Option<&Value>) -> Result<Option<PublicContainer>, String> {
        value
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|error| self.describe(format!("answered an unreadable container: {error}")))
    }

    /// The containers the daemon named, in the order it named them.
    fn public_container_list(&self, value: Option<&Value>) -> Result<Vec<PublicContainer>, String> {
        let Some(values) = value.and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        values
            .iter()
            .map(|value| serde_json::from_value(value.clone()))
            .collect::<Result<Vec<PublicContainer>, _>>()
            .map_err(|error| self.describe(format!("answered an unreadable container: {error}")))
    }
}

impl LifecycleControls for DaemonRuntime {
    fn supports_child_close(&self) -> bool {
        self.serves(ops::Contract::SpawnClose)
    }

    fn read_operation(
        &self,
        request_id: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<LifecycleAnswer>, String> {
        let result = self
            .call("mux.request.get", json!({"id": request_id}), cancel)
            .map_err(|failure| failure.message)?;
        Ok(result
            .get("request")
            .cloned()
            .map(|request| LifecycleAnswer { request }))
    }

    fn close_child(
        &self,
        request_id: &str,
        target: &crate::registry_observation::ChildCloseTarget,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(ops::Contract::SpawnClose) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::SpawnClose.as_str()
            )));
        }
        let result = exchange_with_id_at(
            &self.socket,
            request_id,
            "spawn.close",
            json!({
                "spawn_request_id": target.spawn_request_id,
                "source": target.source,
                "incarnation": target.incarnation,
                "intent": "cancel",
            }),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("close").cloned().unwrap_or(Value::Null),
        })
    }

    fn cancel_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.lifecycle_control(
            ops::Contract::LifecycleCancel,
            "lifecycle.cancel",
            request,
            cancel,
        )
    }

    fn stop_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.lifecycle_control(
            ops::Contract::LifecycleStop,
            "lifecycle.stop",
            request,
            cancel,
        )
    }

    fn restart_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.lifecycle_control(
            ops::Contract::LifecycleRestart,
            "lifecycle.restart",
            request,
            cancel,
        )
    }

    fn enroll_agent(
        &self,
        request_id: &str,
        registration_id: &str,
        session: &str,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.enroll_registered(request_id, registration_id, session, cancel)
    }

    fn abandon_record(
        &self,
        request_id: &str,
        held_request_id: &str,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(ops::Contract::LifecycleAbandon) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::LifecycleAbandon.as_str()
            )));
        }
        let result = exchange_with_id_at(
            &self.socket,
            request_id,
            "lifecycle.abandon",
            json!({"request_id": held_request_id, "requester": REQUESTER}),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
        })
    }
}

impl DaemonRuntime {
    /// One enrollment, sent under the caller's own request id.
    fn enroll_registered(
        &self,
        request_id: &str,
        registration_id: &str,
        session: &str,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(ops::Contract::LifecycleEnroll) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::LifecycleEnroll.as_str()
            )));
        }
        let result = exchange_with_id_at(
            &self.socket,
            request_id,
            "lifecycle.enroll",
            json!({"request": {"registration_id": registration_id, "session": session}, "requester": REQUESTER}),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
        })
    }
}

impl ContainerControls for DaemonRuntime {
    fn create_container(
        &self,
        request: &ContainerCreate,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let request_kind = match &request.kind {
            ContainerCreateKind::Workspace { directory } => {
                let mut request = json!({"kind": "workspace", "focus": false});
                if let Some(directory) = directory {
                    request["directory"] = json!(directory);
                }
                request
            }
            ContainerCreateKind::Tab { workspace_id } => json!({
                "kind": "tab",
                "workspace_id": workspace_id,
                "focus": false,
            }),
            ContainerCreateKind::PaneSplit { pane_id } => json!({
                "kind": "pane_split",
                "pane_id": pane_id,
                "direction": "right",
                "focus": false,
            }),
        };
        let mut params = json!({
            "request": request_kind,
            "requester": REQUESTER,
        });
        if let Some(label) = &request.label {
            params["label"] = json!(label);
        }
        if let Some(recipe) = &request.recipe {
            params["recipe"] = serde_json::to_value(recipe)
                .map_err(|error| self.describe(format!("cannot carry the recipe: {error}")))?;
        }
        self.container_control(
            ops::Contract::TopologyCreate,
            &request.request_id,
            "topology.create",
            params,
            cancel,
        )
    }

    fn delete_container(
        &self,
        request: &ContainerDelete,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let params = json!({
            "request": {
                "container_id": request.container_id,
                "frozen": request.frozen,
            },
            "requester": REQUESTER,
        });
        self.container_control(
            ops::Contract::TopologyDelete,
            &request.request_id,
            "topology.delete",
            params,
            cancel,
        )
    }

    fn reload_container(
        &self,
        request: &ContainerReload,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let params = json!({
            "request": {"container_id": request.container_id},
            "requester": REQUESTER,
        });
        self.container_control(
            ops::Contract::TopologyReload,
            &request.request_id,
            "topology.reload",
            params,
            cancel,
        )
    }
}
