//! Logical containers: the workspace, tab and pane slots the fleet is grouped by.
//!
//! A container is identified by its own durable id. The backend's name, the
//! verified incarnation of the process serving the handle, and the raw handle are
//! a binding — evidence that a container exists somewhere, never the container's
//! identity. A handle that reappears under a different serving incarnation is not
//! the recorded container.
//!
//! Private recipe data is deliberately absent from these records. A recipe lives
//! in owner-only storage and never enters a container record, a read or an event.

use serde::{Deserialize, Serialize};

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::control_plane::events::{Log, Source, Transition};
use crate::control_plane::registry::Registry;
use crate::control_plane::registry::publication::write_atomic;
use crate::lifecycle::CloseRequest;
use crate::model::{FleetObservation, ProcessIdentity};
use crate::runtime::{BackendIdentity, CloseTarget, CreateRequest, CreatedLocation};

/// What a container is in the hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Workspace,
    Tab,
    Pane,
}

impl Kind {
    /// The close target a handle of this kind is deleted by.
    pub fn target(self, handle: &str) -> CloseTarget {
        match self {
            Self::Workspace => CloseTarget::Workspace(handle.to_string()),
            Self::Tab => CloseTarget::Tab(handle.to_string()),
            Self::Pane => CloseTarget::Pane(handle.to_string()),
        }
    }
}

/// The frozen request one container reload carries: the container whose slot runs
/// its recorded recipe again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReloadRequest {
    pub container_id: String,
}

/// The frozen request one container deletion carries.
///
/// The operator confirms a container and the location and occupant they saw it
/// hold, so a deletion acts on the container that was confirmed rather than on
/// whatever holds that place when the daemon gets to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteRequest {
    pub container_id: String,
    /// The target and occupant frozen at confirmation, revalidated before the
    /// effect as the generic close does.
    pub frozen: CloseRequest,
}

/// Who made the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// This daemon created it, so its recipe and its reload are ours.
    Created,
    /// Only observed in the backend. Never recorded as ours, never reloaded
    /// from an invented recipe.
    Observed,
}

/// Where a container's handle lives, and which incarnation answered for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub backend: String,
    /// The verified process incarnation serving the handle, when one was
    /// established. A handle alone cannot be told apart from a later server
    /// reusing it, so a binding that names no incarnation is never verified.
    pub incarnation: Option<ProcessIdentity>,
    pub handle: String,
}

/// One logical container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Container {
    pub id: String,
    pub kind: Kind,
    pub parent: Option<String>,
    pub label: Option<String>,
    /// The operation that created it, so a replayed creation is answered with the
    /// same container instead of creating a second one.
    pub created_by: Option<String>,
    /// Whether a recipe is held for this container's pane, in owner-only storage.
    ///
    /// The record says only that one is held: a command and its environment are not
    /// public facts, and no read carries them.
    #[serde(default)]
    pub recipe: bool,
    pub provenance: Provenance,
    /// Absent when creation could not establish it: the container is recorded
    /// unresolved rather than bound to a guessed handle.
    pub binding: Option<Binding>,
    pub created_at: String,
    pub observed_at: Option<String>,
    /// A source-labelled reason the binding cannot be trusted right now.
    pub unresolved: Option<String>,
    pub removed_at: Option<String>,
    pub recorded_at: String,
}

/// What a reconciliation read saw for a container's recorded binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observed {
    /// The recorded instance still serves the recorded handle.
    Present,
    /// No backend reports the handle.
    Missing,
    /// The handle exists, but under a different serving instance.
    Replaced,
    /// The incarnation serving the handle could not be verified, so nothing is
    /// concluded.
    Unavailable,
}

/// One reconciliation result for one container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerObservation {
    pub state: Observed,
}

impl ContainerObservation {
    /// Applies one observation without rewriting identity or binding. A
    /// reconciliation reports drift; it never repairs a binding or adopts a
    /// handle.
    pub fn apply_to(&self, container: &mut Container, at: &str) {
        container.observed_at = Some(at.to_owned());
        container.unresolved = match self.state {
            Observed::Present => None,
            Observed::Missing => Some("no backend reports this container's handle".to_owned()),
            Observed::Replaced => Some(
                "this container's handle now exists under a different serving incarnation"
                    .to_owned(),
            ),
            Observed::Unavailable => Some(
                "the incarnation serving this container's handle could not be verified".to_owned(),
            ),
        };
    }
}

impl Container {
    /// What a fresh inventory says about this container's binding.
    ///
    /// A binding with no recorded incarnation, or a handle whose incarnation
    /// cannot be verified now, is unavailable rather than present: an unfenced
    /// handle cannot be told apart from a later server reusing it.
    pub fn observed(&self, seen: &[Binding]) -> ContainerObservation {
        let Some(recorded) = self.binding.as_ref() else {
            return ContainerObservation {
                state: Observed::Unavailable,
            };
        };
        let Some(recorded) = recorded.incarnation.as_ref() else {
            return ContainerObservation {
                state: Observed::Unavailable,
            };
        };
        let mut matched = false;
        for candidate in seen
            .iter()
            .filter(|candidate| candidate.backend == self.binding_backend())
            .filter(|candidate| candidate.handle == self.binding_handle())
        {
            let Some(incarnation) = candidate.incarnation.as_ref() else {
                // The handle is reported, but the process serving it was not
                // verified, so this snapshot confirms nothing.
                return ContainerObservation {
                    state: Observed::Unavailable,
                };
            };
            if incarnation == recorded {
                return ContainerObservation {
                    state: Observed::Present,
                };
            }
            matched = true;
        }
        ContainerObservation {
            state: if matched {
                Observed::Replaced
            } else {
                Observed::Missing
            },
        }
    }

    fn binding_backend(&self) -> &str {
        self.binding
            .as_ref()
            .map(|binding| binding.backend.as_str())
            .unwrap_or_default()
    }

    fn binding_handle(&self) -> &str {
        self.binding
            .as_ref()
            .map(|binding| binding.handle.as_str())
            .unwrap_or_default()
    }

    /// The read a consumer sees: identity, binding and provenance as separate
    /// facts, and nothing private.
    pub fn public(&self) -> PublicContainer {
        PublicContainer {
            container_id: self.id.clone(),
            kind: self.kind,
            parent: self.parent.clone(),
            label: self.label.clone(),
            created_by: self.created_by.clone(),
            recipe: self.recipe,
            provenance: self.provenance,
            binding: self.binding.clone(),
            created_at: self.created_at.clone(),
            observed_at: self.observed_at.clone(),
            unresolved: self.unresolved.clone(),
            removed_at: self.removed_at.clone(),
        }
    }
}

/// One container as a consumer reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicContainer {
    pub container_id: String,
    pub kind: Kind,
    pub parent: Option<String>,
    pub label: Option<String>,
    /// The operation that created it, for a caller reconciling its own requests.
    pub created_by: Option<String>,
    /// Whether a recipe is held for this container's pane, so a caller knows the
    /// slot can be reloaded without reading private storage.
    pub recipe: bool,
    pub provenance: Provenance,
    pub binding: Option<Binding>,
    pub created_at: String,
    pub observed_at: Option<String>,
    pub unresolved: Option<String>,
    pub removed_at: Option<String>,
}

/// A container's stored record, read back through the log's replay.
pub fn read(registry: &Registry, id: &str) -> Result<Option<Container>, String> {
    let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    Ok(log.container(id).cloned())
}

/// The container one creation made, so a replay is answered with it rather than
/// with a second creation.
pub fn created_by(registry: &Registry, request_id: &str) -> Option<Container> {
    let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    log.container_ids()
        .filter_map(|id| log.container(id))
        .find(|container| container.created_by.as_deref() == Some(request_id))
        .cloned()
}

/// One page of containers in id order, with `after` as the continuation.
pub fn list_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Container>, String> {
    let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    let mut ids: Vec<&String> = log
        .container_ids()
        .filter(|id| after.is_none_or(|after| id.as_str() > after))
        .collect();
    ids.sort();
    ids.truncate(limit);
    Ok(ids
        .into_iter()
        .filter_map(|id| log.container(id).cloned())
        .collect())
}

/// The creation a runtime confirmed, as the daemon records it.
pub struct Confirmed<'a> {
    /// The identity the container is recorded under. The caller states it, so a
    /// recipe can be held under it before the record that names it is written.
    pub id: String,
    /// Whether a recipe was held for this container's pane.
    pub recipe: bool,
    /// What the caller asked for. Only the identity named here is recorded.
    pub request: &'a CreateRequest,
    /// What the runtime said it made.
    pub created: &'a CreatedLocation,
    pub backend: &'a BackendIdentity,
    /// The verified incarnation serving the runtime, when one was established.
    pub incarnation: Option<ProcessIdentity>,
    /// Radar's own name for the container. The backend is never told it.
    pub label: Option<String>,
    /// The operation that made it.
    pub created_by: Option<String>,
}

/// Records the container a confirmed creation made.
///
/// Only the identity the request asked for is recorded. The other identities a
/// runtime reports are context, and recording them would duplicate a container
/// that already exists. The parent link is recorded only when it resolves to a
/// container this daemon already records; an unresolved parent stays absent
/// rather than being guessed.
pub fn record_created(
    registry: &Registry,
    confirmed: Confirmed<'_>,
    at: &str,
) -> Result<Vec<Container>, String> {
    let Confirmed {
        id,
        recipe,
        request,
        created,
        backend,
        incarnation,
        label,
        created_by,
    } = confirmed;
    let (kind, handle, target) = match request {
        CreateRequest::Workspace { .. } => (Kind::Workspace, created.workspace_id.as_deref(), None),
        CreateRequest::Tab { workspace_id, .. } => (
            Kind::Tab,
            created.tab_id.as_deref(),
            created
                .workspace_id
                .as_deref()
                .or(Some(workspace_id.as_str())),
        ),
        CreateRequest::PaneSplit { pane_id, .. } => (
            Kind::Pane,
            created.pane_id.as_deref(),
            Some(pane_id.as_str()),
        ),
    };
    // The runtime did not name what it made, so there is no container to record:
    // a container is never invented from the request.
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    let parent = match kind {
        Kind::Workspace => None,
        Kind::Tab => recorded_container(&log, backend, incarnation.as_ref(), target),
        // A new pane sits in the tab its target pane is in, so the parent is the
        // target's recorded parent rather than the target itself.
        Kind::Pane => recorded_container(&log, backend, incarnation.as_ref(), target)
            .and_then(|id| log.container(&id).and_then(|c| c.parent.clone())),
    };
    let container = Container {
        id,
        kind,
        parent,
        label,
        created_by,
        recipe,
        provenance: Provenance::Created,
        binding: Some(Binding {
            backend: backend.name.clone(),
            incarnation,
            handle: handle.to_owned(),
        }),
        created_at: at.to_owned(),
        observed_at: None,
        unresolved: None,
        removed_at: None,
        recorded_at: at.to_owned(),
    };
    log.append(
        &container.id,
        at,
        Source::TopologyControl,
        Transition::ContainerCreated(container.clone()),
    )?;
    Ok(vec![container])
}

/// Records a container, as observed, for every place of this server's own
/// inventory that has none, and returns how many it recorded.
///
/// The inventory is the one server this daemon serves, so adoption reaches
/// nothing outside it. A place is adopted only under a verified serving
/// incarnation: an unfenced handle cannot be told from a later server reusing it.
/// An adopted container is `Observed`: it has no recipe, so it is never reloaded,
/// and it is never claimed as Radar's. Places already recorded, including by a
/// creation, are left exactly as they are, so a pass over an unchanged server
/// appends nothing.
pub fn adopt(
    registry: &Registry,
    observation: &FleetObservation,
    backend: &BackendIdentity,
    incarnation: Option<&ProcessIdentity>,
    at: &str,
) -> Result<usize, String> {
    let Some(incarnation) = incarnation else {
        return Ok(0);
    };
    let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    let mut adopted = 0;
    // Parents first, so a child resolves its parent in the same pass.
    let mut places: Vec<(Kind, &str, Option<&str>)> = Vec::new();
    for workspace in &observation.workspaces {
        places.push((Kind::Workspace, workspace.workspace_id.as_str(), None));
    }
    for tab in &observation.tabs {
        places.push((
            Kind::Tab,
            tab.tab_id.as_str(),
            Some(tab.workspace_id.as_str()),
        ));
    }
    for pane in &observation.panes {
        places.push((
            Kind::Pane,
            pane.location.pane_id.as_str(),
            Some(pane.location.tab_id.as_str()),
        ));
    }
    for (kind, handle, parent) in places {
        if recorded_container(&log, backend, Some(incarnation), Some(handle)).is_some() {
            continue;
        }
        let parent = recorded_container(&log, backend, Some(incarnation), parent);
        let container = Container {
            id: crate::control_plane::store::random_uuid(),
            kind,
            parent,
            label: None,
            created_by: None,
            recipe: false,
            provenance: Provenance::Observed,
            binding: Some(Binding {
                backend: backend.name.clone(),
                incarnation: Some(incarnation.clone()),
                handle: handle.to_owned(),
            }),
            created_at: at.to_owned(),
            observed_at: Some(at.to_owned()),
            unresolved: None,
            removed_at: None,
            recorded_at: at.to_owned(),
        };
        log.append(
            &container.id,
            at,
            Source::TopologyObservation,
            Transition::ContainerCreated(container.clone()),
        )?;
        adopted += 1;
    }
    Ok(adopted)
}

/// The container this daemon records for a handle under one serving incarnation.
///
/// A handle under a different incarnation is a different container, a removed
/// container is never matched again, and a create that established no incarnation
/// resolves only a container that established none either.
fn recorded_container(
    log: &Log,
    backend: &BackendIdentity,
    incarnation: Option<&ProcessIdentity>,
    handle: Option<&str>,
) -> Option<String> {
    let handle = handle?;
    log.container_ids()
        .filter_map(|id| log.container(id))
        .find(|container| {
            container.removed_at.is_none()
                && container.binding.as_ref().is_some_and(|binding| {
                    binding.backend == backend.name
                        && binding.handle == handle
                        && binding.incarnation.as_ref() == incarnation
                })
        })
        .map(|container| container.id.clone())
}

/// The handles one backend inventory reports, and the incarnation serving them.
///
/// The snapshot is the current server's own view, and every handle in it belongs
/// to the incarnation verified alongside it. A handle recorded under a previous
/// server is then a different container rather than a present one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub backend: String,
    pub incarnation: Option<ProcessIdentity>,
    pub workspaces: Vec<String>,
    pub tabs: Vec<String>,
    pub panes: Vec<String>,
}

impl Inventory {
    pub fn from_observation(
        observation: &FleetObservation,
        backend: &BackendIdentity,
        incarnation: Option<ProcessIdentity>,
    ) -> Self {
        Self {
            backend: backend.name.clone(),
            incarnation,
            workspaces: observation
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id.clone())
                .collect(),
            tabs: observation
                .tabs
                .iter()
                .map(|tab| tab.tab_id.clone())
                .collect(),
            panes: observation
                .panes
                .iter()
                .map(|pane| pane.location.pane_id.clone())
                .collect(),
        }
    }

    pub fn bindings(&self, kind: Kind) -> Vec<Binding> {
        let handles = match kind {
            Kind::Workspace => &self.workspaces,
            Kind::Tab => &self.tabs,
            Kind::Pane => &self.panes,
        };
        handles
            .iter()
            .map(|handle| Binding {
                backend: self.backend.clone(),
                incarnation: self.incarnation.clone(),
                handle: handle.clone(),
            })
            .collect()
    }
}

/// The command a container's pane runs, held privately and never reported.
pub type Recipe = crate::control_plane::lifecycle::LaunchContext;

/// The most one private recipe may occupy, as the private store's own bound.
const MAX_RECIPE_BYTES: usize = 8 * 1024;

/// Where one container's private recipe is held.
///
/// The private directory is the daemon's owner-only store: a recipe is described
/// by no record, returned by no read and written into no event.
fn recipe_path(registry: &Registry, id: &str) -> PathBuf {
    registry
        .subjects()
        .join("private")
        .join(format!("container-{id}.json"))
}

/// Holds one container's recipe privately, before the record that says so.
pub fn write_recipe(registry: &Registry, id: &str, recipe: &Recipe) -> Result<(), String> {
    let directory = registry.subjects().join("private");
    crate::control_plane::registry::prepare_directory(&directory)?;
    let bytes = serde_json::to_vec_pretty(recipe)
        .map_err(|_| "cannot encode a container recipe".to_string())?;
    write_atomic(
        &directory,
        &recipe_path(registry, id),
        &bytes,
        MAX_RECIPE_BYTES,
    )
}

/// The recipe this container's pane runs, when one is held.
pub fn read_recipe(registry: &Registry, id: &str) -> Result<Option<Recipe>, String> {
    let path = recipe_path(registry, id);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("container recipe {id}: {error}")),
        Ok(_) => {}
    }
    let bytes = crate::control_plane::registry::read_record_file(&path, MAX_RECIPE_BYTES)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("not a container recipe: {error}"))
}

/// A container and everything recorded inside it, in id order.
///
/// Deleting a container removes what it holds, so the caller marks the whole
/// subtree rather than leaving records of places that no longer exist.
pub fn subtree(registry: &Registry, id: &str) -> Result<Vec<String>, String> {
    let containers: Vec<Container> = {
        let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
        log.container_ids()
            .filter_map(|id| log.container(id).cloned())
            .collect()
    };
    if !containers.iter().any(|container| container.id == id) {
        return Err(format!("container {id} is not recorded"));
    }
    let mut members: BTreeSet<String> = BTreeSet::from([id.to_string()]);
    let mut frontier = vec![id.to_string()];
    while let Some(parent) = frontier.pop() {
        for container in &containers {
            if container.parent.as_deref() == Some(parent.as_str())
                && members.insert(container.id.clone())
            {
                frontier.push(container.id.clone());
            }
        }
    }
    Ok(members.into_iter().collect())
}

/// Records one container as removed by an operator's deletion.
///
/// The binding is kept: the record still says which handle it was, and a handle
/// reappearing under another serving instance is a different container. Nothing
/// here dispatches, and a container already recorded as removed is reported as it
/// stands rather than appended twice.
pub fn record_removed(registry: &Registry, id: &str, at: &str) -> Result<Container, String> {
    let previous = read(registry, id)?.ok_or_else(|| format!("container {id} is not recorded"))?;
    if previous.removed_at.is_some() {
        return Ok(previous);
    }
    let mut next = previous.clone();
    next.removed_at = Some(at.to_string());
    {
        let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
        log.append_container_checked(
            id,
            at,
            Source::TopologyControl,
            Transition::ContainerRemoved,
            &next,
        )?;
    }
    Ok(next)
}

/// What one reconciliation pass did, so a caller reads counts rather than prose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileOutcome {
    pub present: usize,
    pub missing: usize,
    pub replaced: usize,
    pub unavailable: usize,
    /// Containers whose record moved while the pass ran, so their observation was
    /// not written over the newer record.
    pub skipped: usize,
}

/// Reconciles recorded containers against one inventory snapshot.
///
/// Each observation is a recorded fact, and the record is never rewritten,
/// adopted or deleted: a container the backend no longer reports stays recorded
/// and marked. Nothing is dispatched, so a reconciliation cannot close, reload
/// or create anything.
pub fn reconcile(
    registry: &Registry,
    inventory: &Inventory,
    at: &str,
    limit: usize,
) -> Result<ReconcileOutcome, String> {
    let ids: Vec<String> = {
        let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<String> = log.container_ids().cloned().collect();
        ids.sort();
        ids.truncate(limit);
        ids
    };
    let mut outcome = ReconcileOutcome::default();
    for id in ids {
        let previous = read(registry, &id)?.ok_or("container is not recorded")?;
        // A removed container is history, and a container with no binding has
        // nothing to reconcile: neither is observed again.
        if previous.removed_at.is_some() || previous.binding.is_none() {
            outcome.skipped += 1;
            continue;
        }
        let observation = previous.observed(&inventory.bindings(previous.kind));
        // The check is against the state this observation should produce, so a
        // record another pass changed in the meantime is not written over.
        let mut next = previous.clone();
        observation.apply_to(&mut next, at);
        let appended = {
            let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
            log.append_container_checked(
                &id,
                at,
                Source::TopologyObservation,
                Transition::ContainerObserved(observation),
                &next,
            )
        };
        if let Err(error) = appended {
            // A record that moved under this pass keeps its newer observation;
            // anything else is a storage failure the caller must see.
            if read(registry, &id)?.as_ref() != Some(&previous) {
                outcome.skipped += 1;
                continue;
            }
            return Err(error);
        }
        match observation.state {
            Observed::Present => outcome.present += 1,
            Observed::Missing => outcome.missing += 1,
            Observed::Replaced => outcome.replaced += 1,
            Observed::Unavailable => outcome.unavailable += 1,
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Location, Pane, Tab, Workspace};
    use crate::runtime::SplitDirection;

    /// A verified serving incarnation, told apart by the pid it names.
    fn incarnation(pid: i32) -> ProcessIdentity {
        ProcessIdentity {
            boot_id: "boot-1".into(),
            pid,
            start_ticks: 100,
        }
    }

    fn binding(handle: &str, pid: i32) -> Binding {
        Binding {
            backend: "fake".into(),
            incarnation: Some(incarnation(pid)),
            handle: handle.into(),
        }
    }

    fn backend() -> BackendIdentity {
        BackendIdentity::named("fake")
    }

    fn container(binding: Option<Binding>) -> Container {
        Container {
            id: crate::control_plane::store::random_uuid(),
            kind: Kind::Pane,
            parent: None,
            label: None,
            created_by: None,
            recipe: false,
            provenance: Provenance::Created,
            binding,
            created_at: "1000".into(),
            observed_at: None,
            unresolved: None,
            removed_at: None,
            recorded_at: "1000".into(),
        }
    }

    #[test]
    fn an_unfenced_binding_is_never_called_present() {
        assert_eq!(container(None).observed(&[]).state, Observed::Unavailable);
        let unfenced = container(Some(Binding {
            backend: "fake".into(),
            incarnation: None,
            handle: "wA:p1".into(),
        }));
        // The handle is in the inventory, but nothing says which server made it.
        assert_eq!(
            unfenced.observed(&[binding("wA:p1", 42)]).state,
            Observed::Unavailable
        );
        // The record names its incarnation, but the snapshot's was not verified.
        assert_eq!(
            container(Some(binding("wA:p1", 42)))
                .observed(&[Binding {
                    backend: "fake".into(),
                    incarnation: None,
                    handle: "wA:p1".into(),
                }])
                .state,
            Observed::Unavailable
        );
    }

    #[test]
    fn a_reused_handle_is_replaced_and_an_absent_one_is_missing() {
        let recorded = container(Some(binding("wA:p1", 42)));
        assert_eq!(
            recorded.observed(&[binding("wA:p1", 42)]).state,
            Observed::Present
        );
        assert_eq!(
            recorded.observed(&[binding("wA:p1", 43)]).state,
            Observed::Replaced
        );
        assert_eq!(
            recorded.observed(&[binding("wA:p9", 42)]).state,
            Observed::Missing
        );
        assert_eq!(recorded.observed(&[]).state, Observed::Missing);
    }

    #[test]
    fn an_observation_reports_drift_without_rewriting_the_binding() {
        let mut recorded = container(Some(binding("wA:p1", 42)));
        let before = recorded.binding.clone();
        ContainerObservation {
            state: Observed::Replaced,
        }
        .apply_to(&mut recorded, "2000");
        assert_eq!(
            recorded.binding, before,
            "a reconciliation never rewrites a binding"
        );
        assert_eq!(recorded.observed_at.as_deref(), Some("2000"));
        assert!(recorded.unresolved.is_some());
        ContainerObservation {
            state: Observed::Present,
        }
        .apply_to(&mut recorded, "3000");
        assert!(recorded.unresolved.is_none());
        assert_eq!(recorded.observed_at.as_deref(), Some("3000"));
    }

    #[test]
    fn a_removed_container_is_never_matched_by_handle_again() {
        let directory = std::env::temp_dir().join(format!(
            "radar-containers-{}",
            crate::control_plane::store::random_uuid()
        ));
        let mut log = Log::open(&directory).expect("a container log");
        let created = container(Some(binding("wA:p1", 42)));
        let id = created.id.clone();
        log.append(
            &id,
            "1000",
            Source::TopologyControl,
            Transition::ContainerCreated(created),
        )
        .expect("a recorded container");
        assert_eq!(
            recorded_container(&log, &backend(), Some(&incarnation(42)), Some("wA:p1")).as_deref(),
            Some(id.as_str())
        );
        assert_eq!(
            recorded_container(&log, &backend(), Some(&incarnation(43)), Some("wA:p1")),
            None,
            "a handle under another incarnation is not this container"
        );
        log.append(
            &id,
            "2000",
            Source::TopologyControl,
            Transition::ContainerRemoved,
        )
        .expect("a removal");
        assert_eq!(
            recorded_container(&log, &backend(), Some(&incarnation(42)), Some("wA:p1")),
            None,
            "a removed container is not the container a handle names"
        );
        assert!(log.container(&id).expect("the record").removed_at.is_some());
        let _ = std::fs::remove_dir_all(&directory);
    }

    fn record(registry: &Registry, request: CreateRequest, created: CreatedLocation) -> Container {
        record_created(
            registry,
            Confirmed {
                id: crate::control_plane::store::random_uuid(),
                recipe: false,
                request: &request,
                created: &created,
                backend: &backend(),
                incarnation: Some(incarnation(42)),
                label: None,
                created_by: None,
            },
            "1000",
        )
        .expect("a recorded container")
        .remove(0)
    }

    #[test]
    fn an_inventory_carries_the_current_instance_for_every_handle() {
        let observation = FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wA".into(),
                label: None,
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wA:t1".into(),
                workspace_id: "wA".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: Location {
                    workspace_id: "wA".into(),
                    tab_id: "wA:t1".into(),
                    pane_id: "wA:p1".into(),
                },
                label: None,
                title: None,
            }],
            agents: Vec::new(),
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
        };
        let inventory =
            Inventory::from_observation(&observation, &backend(), Some(incarnation(42)));
        assert_eq!(inventory.bindings(Kind::Workspace), [binding("wA", 42)]);
        assert_eq!(inventory.bindings(Kind::Tab), [binding("wA:t1", 42)]);
        assert_eq!(inventory.bindings(Kind::Pane), [binding("wA:p1", 42)]);
    }

    #[test]
    fn reconciling_reports_drift_and_keeps_every_record() {
        let directory = std::env::temp_dir().join(format!(
            "radar-containers-{}",
            crate::control_plane::store::random_uuid()
        ));
        let registry = Registry::open_at(&directory, 1000).expect("a registry");
        let pane = record(
            &registry,
            CreateRequest::PaneSplit {
                pane_id: "wA:p1".into(),
                direction: SplitDirection::Right,
                focus: false,
            },
            CreatedLocation {
                pane_id: Some("wA:p2".into()),
                ..CreatedLocation::default()
            },
        );
        let tab = record(
            &registry,
            CreateRequest::Tab {
                workspace_id: "wA".into(),
                focus: false,
            },
            CreatedLocation {
                workspace_id: Some("wA".into()),
                tab_id: Some("wA:t2".into()),
                ..CreatedLocation::default()
            },
        );

        // The pane is reported by the instance that recorded it; the tab is not
        // reported at all.
        let observed = reconcile(
            &registry,
            &Inventory {
                backend: "fake".into(),
                incarnation: Some(incarnation(42)),
                panes: vec!["wA:p2".into()],
                ..Inventory::default()
            },
            "2000",
            16,
        )
        .expect("a reconciliation");
        assert_eq!(
            observed,
            ReconcileOutcome {
                present: 1,
                missing: 1,
                ..ReconcileOutcome::default()
            }
        );
        let after = read(&registry, &pane.id).expect("a read").expect("a pane");
        assert_eq!(after.observed_at.as_deref(), Some("2000"));
        assert!(after.unresolved.is_none());
        assert_eq!(
            after.binding, pane.binding,
            "an observation never rewrites a binding"
        );
        let gone = read(&registry, &tab.id).expect("a read").expect("a tab");
        assert!(
            gone.unresolved
                .as_deref()
                .is_some_and(|reason| reason.contains("no backend reports")),
            "{gone:?}"
        );
        assert!(
            gone.removed_at.is_none(),
            "a container the backend no longer reports keeps its record"
        );
        assert_eq!(list_after(&registry, None, 16).expect("a page").len(), 2);

        // The same handles under another server are different containers.
        let observed = reconcile(
            &registry,
            &Inventory {
                backend: "fake".into(),
                incarnation: Some(incarnation(43)),
                tabs: vec!["wA:t2".into()],
                panes: vec!["wA:p2".into()],
                ..Inventory::default()
            },
            "3000",
            16,
        )
        .expect("a reconciliation");
        assert_eq!(
            observed,
            ReconcileOutcome {
                replaced: 2,
                ..ReconcileOutcome::default()
            }
        );
        assert!(
            read(&registry, &pane.id)
                .expect("a read")
                .expect("a pane")
                .unresolved
                .as_deref()
                .is_some_and(|reason| reason.contains("different serving incarnation"))
        );
        assert_eq!(
            list_after(&registry, None, 16).expect("a page").len(),
            2,
            "reconciling records observations, not containers"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
