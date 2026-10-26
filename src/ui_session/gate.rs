//! The interaction gate: at most one shared interaction blocks the session.
//!
//! A command that needs an answer opens the gate and captures what it was
//! invoked on, so a later selection change cannot retarget it. The gate then
//! moves from Preparing (the invoking frontend gathers choices) to
//! AwaitingResult (one presenter answers) and closes on the first result or
//! cancellation; the loser of that race finds nothing to answer. Every open and
//! close advances the generation that stamps ordinary input.

use crate::control_plane::public_contracts::ui_session::{
    Blocking, InteractionResult, InteractionStage, RefusalCode, SharedCommand, UiRowId,
};

/// What an accepted result sets in motion. The reducer only records it; the
/// actor hands it to the domain service, off the session thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub interaction_id: String,
    pub command: SharedCommand,
    /// The row the command was invoked on, as it was then.
    pub captured: Option<UiRowId>,
    pub result: InteractionResult,
}

#[derive(Debug)]
struct Open {
    interaction_id: String,
    command: SharedCommand,
    captured: Option<UiRowId>,
    /// The attachment that invoked the command.
    owner: String,
    stage: InteractionStage,
    choices: Vec<String>,
    presenter: Option<String>,
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
        self.open.as_ref().map(|open| Blocking {
            interaction_id: open.interaction_id.clone(),
            command: open.command,
            stage: open.stage,
            claimed: open.presenter.is_some(),
        })
    }

    /// Opens the gate for `command`, invoked by `owner` on `captured`.
    pub fn begin(
        &mut self,
        owner: &str,
        command: SharedCommand,
        captured: Option<UiRowId>,
    ) -> Result<String, Refused> {
        if self.open.is_some() {
            return refused(RefusalCode::Blocked, "another interaction is open");
        }
        self.generation += 1;
        let interaction_id = format!("i-{}", self.generation);
        self.open = Some(Open {
            interaction_id: interaction_id.clone(),
            command,
            captured,
            owner: owner.to_string(),
            stage: InteractionStage::Preparing,
            choices: Vec::new(),
            presenter: None,
        });
        Ok(interaction_id)
    }

    fn named(&mut self, interaction_id: &str) -> Result<&mut Open, Refused> {
        match self.open.as_mut() {
            Some(open) if open.interaction_id == interaction_id => Ok(open),
            _ => refused(RefusalCode::NoInteraction, "that interaction is not open"),
        }
    }

    /// The owner's gathered choices fix what a result may be.
    pub fn provide_choices(
        &mut self,
        by: &str,
        interaction_id: &str,
        choices: Vec<String>,
    ) -> Result<(), Refused> {
        let open = self.named(interaction_id)?;
        if open.owner != by {
            return refused(RefusalCode::Role, "only the invoking view prepares it");
        }
        if open.stage != InteractionStage::Preparing {
            return refused(RefusalCode::Conflict, "the choices are already fixed");
        }
        let mut unique = Vec::with_capacity(choices.len());
        for choice in choices {
            if !unique.contains(&choice) {
                unique.push(choice);
            }
        }
        open.choices = unique;
        open.stage = InteractionStage::AwaitingResult;
        Ok(())
    }

    /// One presenter holds the claim; the same one may claim again.
    pub fn claim(&mut self, by: &str, interaction_id: &str) -> Result<(), Refused> {
        let open = self.named(interaction_id)?;
        if open.stage != InteractionStage::AwaitingResult {
            return refused(RefusalCode::Conflict, "nothing to present yet");
        }
        match &open.presenter {
            Some(held) if held != by => refused(RefusalCode::Conflict, "already presented"),
            _ => {
                open.presenter = Some(by.to_string());
                Ok(())
            }
        }
    }

    /// Accepts the first valid result and closes the gate.
    pub fn resolve(
        &mut self,
        interaction_id: &str,
        result: InteractionResult,
    ) -> Result<(), Refused> {
        let open = self.named(interaction_id)?;
        if open.stage != InteractionStage::AwaitingResult {
            return refused(
                RefusalCode::Conflict,
                "the interaction is not ready for a result",
            );
        }
        let InteractionResult::Project { path } = &result;
        if !open.choices.contains(path) {
            return refused(RefusalCode::BadResult, "that was not one of the choices");
        }
        let open = self.open.take().expect("the interaction was just named");
        self.generation += 1;
        self.completions.push(Completion {
            interaction_id: open.interaction_id,
            command: open.command,
            captured: open.captured,
            result,
        });
        Ok(())
    }

    /// Closes the gate without a result. Any view may cancel.
    pub fn cancel(&mut self, interaction_id: &str) -> Result<(), Refused> {
        self.named(interaction_id)?;
        self.open = None;
        self.generation += 1;
        Ok(())
    }

    /// An attachment is gone. Its unfinished preparation cannot be completed by
    /// anyone else, so it ends; a presenter that left gives up its claim and
    /// another may take it.
    pub fn attachment_lost(&mut self, id: &str) -> bool {
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        if open.owner == id && open.stage == InteractionStage::Preparing {
            self.open = None;
            self.generation += 1;
            return true;
        }
        if open.presenter.as_deref() == Some(id) {
            open.presenter = None;
            return true;
        }
        false
    }

    pub fn take_completions(&mut self) -> Vec<Completion> {
        std::mem::take(&mut self.completions)
    }
}
