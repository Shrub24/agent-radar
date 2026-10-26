//! The confirmation a shared interaction asks, and the keys that answer it.
//!
//! The session freezes the question, what answering it loses and the effect it
//! dispatches, so every view asks the same thing and an answer names the
//! interaction rather than re-deriving a target. This type holds only what is
//! local: which button is highlighted.

use crossterm::event::{KeyCode, KeyEvent};

/// What the operator answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Confirm,
    Cancel,
}

/// One session-owned confirmation, as presented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    pub interaction_id: String,
    /// The operation, for the dialog's title.
    pub title: &'static str,
    pub prompt: String,
    /// What agreeing loses, one short line each.
    pub losses: Vec<String>,
    pub confirm_selected: bool,
    /// Set once an answer was submitted, so a republication does not ask again.
    pub answered: bool,
}

impl Confirm {
    pub fn new(
        interaction_id: String,
        title: &'static str,
        prompt: String,
        losses: Vec<String>,
    ) -> Self {
        Self {
            interaction_id,
            title,
            prompt,
            losses,
            // Cancel is the default: the destructive button is never under the
            // pointer or the return key by accident.
            confirm_selected: false,
            answered: false,
        }
    }

    /// Escape cancels, Tab and the arrows move between the buttons, Enter
    /// activates the selected one. Everything else is swallowed, so nothing
    /// reaches the tree while the question is up.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Answer> {
        match key.code {
            KeyCode::Esc => Some(Answer::Cancel),
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down => {
                self.confirm_selected = !self.confirm_selected;
                None
            }
            KeyCode::Enter => Some(if self.confirm_selected {
                Answer::Confirm
            } else {
                Answer::Cancel
            }),
            _ => None,
        }
    }
}
