//! One shared interaction, addressed to its preparing host and claimed presenter.
use crate::control_plane::public_contracts::ui_session::{
    Blocking, InteractionKind, InteractionResult, InteractionStage, RefusalCode, SharedCommand,
    UiRowId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub interaction_id: String,
    pub request_id: String,
    pub command: SharedCommand,
    pub captured: Option<UiRowId>,
    pub result: InteractionResult,
    /// The frozen effect an accepted confirmation dispatches.
    pub plan: Option<super::targets::Plan>,
}

#[derive(Debug)]
/// What opens an interaction, before the gate mints its identity.
struct Opening {
    kind: InteractionKind,
    owner: String,
    command: SharedCommand,
    stage: InteractionStage,
    prompt: Option<String>,
    losses: Vec<String>,
}

#[derive(Debug)]
struct Open {
    view: Blocking,
    captured: Option<UiRowId>,
    age: u64,
    plan: Option<super::targets::Plan>,
}
type Refused = (RefusalCode, String);
fn refused<T>(code: RefusalCode, message: &str) -> Result<T, Refused> {
    Err((code, message.to_string()))
}

#[derive(Debug, Default)]
pub struct Gate {
    generation: u64,
    open: Option<Open>,
    completions: Vec<Completion>,
}

impl Gate {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
    pub fn blocking(&self) -> Option<Blocking> {
        self.open.as_ref().map(|o| o.view.clone())
    }

    /// Opens a project picker: its choices come from the invoking view, so it
    /// waits in the preparing stage for them.
    pub fn begin_project(
        &mut self,
        owner: &str,
        command: SharedCommand,
        captured: Option<UiRowId>,
    ) -> Result<String, Refused> {
        self.open(
            Opening {
                kind: InteractionKind::Project,
                owner: owner.into(),
                command,
                stage: InteractionStage::Preparing,
                prompt: None,
                losses: Vec::new(),
            },
            captured,
            None,
        )
    }

    /// Opens a confirmation over an effect that is already frozen. A confirmation
    /// has nothing to prepare, so it is claimable as soon as it opens.
    pub fn begin_confirmation(
        &mut self,
        owner: &str,
        command: SharedCommand,
        captured: Option<UiRowId>,
        prompt: Option<String>,
        losses: Vec<String>,
        plan: super::targets::Plan,
    ) -> Result<String, Refused> {
        self.open(
            Opening {
                kind: InteractionKind::Confirm,
                owner: owner.into(),
                command,
                stage: InteractionStage::AwaitingResult,
                prompt,
                losses,
            },
            captured,
            Some(plan),
        )
    }

    /// Opens one interaction. The two workflows above differ in which stage they
    /// start in and whether an effect is already frozen; both need this.
    fn open(
        &mut self,
        opening: Opening,
        captured: Option<UiRowId>,
        plan: Option<super::targets::Plan>,
    ) -> Result<String, Refused> {
        if self.open.is_some() {
            return refused(RefusalCode::Blocked, "another interaction is open");
        }
        self.generation += 1;
        let interaction_id = crate::control_plane::random_uuid();
        self.open = Some(Open {
            view: Blocking {
                interaction_id: interaction_id.clone(),
                command: opening.command,
                owner: opening.owner,
                stage: opening.stage,
                kind: opening.kind,
                prompt: opening.prompt,
                losses: opening.losses,
                presenter: None,
                choices: Vec::new(),
                note: None,
            },
            captured,
            age: 0,
            plan,
        });
        Ok(interaction_id)
    }

    fn named(&mut self, id: &str) -> Result<&mut Open, Refused> {
        match self.open.as_mut() {
            Some(o) if o.view.interaction_id == id => Ok(o),
            _ => refused(RefusalCode::NoInteraction, "that interaction is not open"),
        }
    }
    pub fn provide_choices(
        &mut self,
        by: &str,
        id: &str,
        choices: Vec<String>,
        note: Option<String>,
    ) -> Result<(), Refused> {
        let o = self.named(id)?;
        if o.view.owner != by {
            return refused(RefusalCode::Role, "only the invoking view prepares it");
        }
        if o.view.stage != InteractionStage::Preparing {
            return refused(RefusalCode::Conflict, "the choices are already fixed");
        }
        let mut unique = std::collections::HashSet::new();
        o.view.choices = choices
            .into_iter()
            .filter(|s| unique.insert(s.clone()))
            .collect();
        o.view.note = note;
        o.view.stage = InteractionStage::AwaitingResult;
        o.age = 0;
        Ok(())
    }
    pub fn claim(&mut self, by: &str, id: &str) -> Result<(), Refused> {
        let o = self.named(id)?;
        if o.view.stage != InteractionStage::AwaitingResult {
            return refused(RefusalCode::Conflict, "nothing to present yet");
        }
        match &o.view.presenter {
            Some(held) if held != by => refused(RefusalCode::Conflict, "already presented"),
            _ => {
                o.view.presenter = Some(by.into());
                Ok(())
            }
        }
    }
    pub fn resolve(
        &mut self,
        by: &str,
        id: &str,
        result: InteractionResult,
    ) -> Result<(), Refused> {
        let o = self.named(id)?;
        if o.view.stage != InteractionStage::AwaitingResult {
            return refused(RefusalCode::Conflict, "the interaction is not ready");
        }
        if o.view.presenter.as_deref() != Some(by) {
            return refused(RefusalCode::Role, "only the claimed presenter resolves it");
        }
        match (&o.view.kind, &result) {
            (InteractionKind::Project, InteractionResult::Project { path }) => {
                if !o.view.choices.contains(path) {
                    return refused(RefusalCode::BadResult, "that was not one of the choices");
                }
            }
            (InteractionKind::Confirm, InteractionResult::Confirm { accepted: false }) => {
                self.open = None;
                self.generation += 1;
                return Ok(());
            }
            (InteractionKind::Confirm, InteractionResult::Confirm { accepted: true })
                if o.plan.is_some() => {}
            _ => {
                return refused(
                    RefusalCode::BadResult,
                    "that answer does not fit this interaction",
                );
            }
        }
        let o = self.open.take().expect("named interaction");
        self.generation += 1;
        // The request identity was frozen with the effect. Minting another here
        // would run the operation under an id nothing correlates it with.
        let request_id = o
            .plan
            .as_ref()
            .map_or_else(crate::control_plane::random_uuid, |plan| {
                plan.request_id.clone()
            });
        self.completions.push(Completion {
            request_id,
            interaction_id: o.view.interaction_id,
            command: o.view.command,
            captured: o.captured,
            result,
            plan: o.plan,
        });
        Ok(())
    }
    /// Files an accepted effect that had nothing to ask first.
    pub fn enqueue(&mut self, plan: super::targets::Plan, command: SharedCommand) {
        let request_id = plan.request_id.clone();
        self.completions.push(Completion {
            interaction_id: request_id.clone(),
            request_id,
            command,
            captured: None,
            result: InteractionResult::Confirm { accepted: true },
            plan: Some(plan),
        });
    }

    pub fn cancel(&mut self, id: &str) -> Result<(), Refused> {
        self.named(id)?;
        self.open = None;
        self.generation += 1;
        Ok(())
    }
    pub fn attachment_lost(&mut self, id: &str) -> bool {
        let Some(o) = &self.open else { return false };
        if o.view.presenter.as_deref() == Some(id)
            || (o.view.owner == id && o.view.presenter.is_none())
        {
            self.open = None;
            self.generation += 1;
            return true;
        }
        false
    }
    /// Expiry is driven by the actor's clock, never a frontend's progress.
    pub fn tick(&mut self, seconds: u64) -> bool {
        let Some(o) = &mut self.open else {
            return false;
        };
        if o.view.presenter.is_some() {
            return false;
        }
        o.age += seconds;
        if o.age < 30 {
            return false;
        }
        self.open = None;
        self.generation += 1;
        true
    }
    pub fn request_id(&self) -> Option<String> {
        self.completions.last().map(|job| job.request_id.clone())
    }
    pub fn take_completions(&mut self) -> Vec<Completion> {
        std::mem::take(&mut self.completions)
    }
}
