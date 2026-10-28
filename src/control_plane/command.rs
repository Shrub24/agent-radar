//! Commands submitted in process; the serialized copy exists only for durable retry identity.

use serde::Serialize;

use super::containers::{DeleteRequest, Recipe, ReloadRequest};
use super::ops::Operation;
use super::protocol::Request;
use crate::lifecycle::CloseRequest;
use crate::runtime::CreateRequest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub requester: String,
    pub action: Action,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Close(CloseRequest),
    Cancel(Subject),
    Restart(Subject),
    Create {
        request: CreateRequest,
        label: Option<String>,
        recipe: Option<Recipe>,
    },
    Delete(DeleteRequest),
    Reload(ReloadRequest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Subject {
    pub subject_id: String,
    pub claim_id: String,
}

#[derive(Serialize)]
struct Frozen<'a, T> {
    request: &'a T,
    requester: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipe: Option<&'a Recipe>,
}

impl Command {
    pub fn new(requester: &str, action: Action) -> Self {
        Self {
            requester: requester.to_owned(),
            action,
        }
    }

    pub fn operation(&self) -> Operation {
        match self.action {
            Action::Close(_) => Operation::Close,
            Action::Cancel(_) => Operation::Cancel,
            Action::Restart(_) => Operation::Restart,
            Action::Create { .. } => Operation::CreateContainer,
            Action::Delete(_) => Operation::DeleteContainer,
            Action::Reload(_) => Operation::ReloadContainer,
        }
    }

    pub fn record_request(&self, id: &str) -> Request {
        fn params<T: Serialize>(
            request: &T,
            requester: &str,
            label: Option<&str>,
            recipe: Option<&Recipe>,
        ) -> serde_json::Value {
            serde_json::to_value(Frozen {
                request,
                requester,
                label,
                recipe,
            })
            .expect("serializable command")
        }
        let requester = &self.requester;
        let params = match &self.action {
            Action::Close(request) => params(request, requester, None, None),
            Action::Cancel(request) | Action::Restart(request) => {
                params(request, requester, None, None)
            }
            Action::Create {
                request,
                label,
                recipe,
            } => params(request, requester, label.as_deref(), recipe.as_ref()),
            Action::Delete(request) => params(request, requester, None, None),
            Action::Reload(request) => params(request, requester, None, None),
        };
        Request {
            id: id.to_owned(),
            method: self.operation().as_str().to_owned(),
            params,
        }
    }
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A creation can hold private launch argv and env.
        f.write_str(match self {
            Self::Close(_) => "Close",
            Self::Cancel(_) => "Cancel",
            Self::Restart(_) => "Restart",
            Self::Create { .. } => "Create",
            Self::Delete(_) => "Delete",
            Self::Reload(_) => "Reload",
        })
    }
}
