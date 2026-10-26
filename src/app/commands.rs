//! The dashboard's commands and the keys that run them.
//!
//! One table is the single source for both the palette's list and what a chosen
//! entry does: an entry carries the key that performs it, and the palette runs
//! that key, so a hotkey shown here cannot drift from the binding.

use crossterm::event::KeyCode;

use crate::picker::Item;

/// One command: what it does, the key that does it, and how that key is written.
pub struct Command {
    pub label: &'static str,
    pub keys: &'static str,
    pub key: KeyCode,
}

impl Command {
    /// Whether this command exists only where `radar sidebar` is drawn.
    pub fn sidebar_only(&self) -> bool {
        matches!(self.key, KeyCode::Char('B' | 'Z'))
    }

    /// Whether this command is about the sidebar's own split, so the sidebar
    /// itself cannot offer it.
    pub fn launches_sidebar(&self) -> bool {
        self.key == KeyCode::Char('S')
    }

    /// Whether this command opens a picker or a confirmation, which is all the
    /// workspace popup is for: the rest act on a view the popup does not show.
    pub fn opens_modal(&self) -> bool {
        matches!(
            self.key,
            KeyCode::Char('r' | 'a' | 'A' | 'L' | 'E' | 'D' | 'R')
        )
    }

    /// Both presentations manage the same fleet. Only commands for absent
    /// panels and the sidebar's own host controls differ.
    pub fn offered(&self, sidebar: bool, compact: bool) -> bool {
        if self.sidebar_only() {
            return sidebar;
        }
        if self.launches_sidebar() {
            return !sidebar;
        }
        if sidebar && compact && matches!(self.key, KeyCode::Char('d') | KeyCode::Tab) {
            return false;
        }
        true
    }
}

/// Every command the fleet view answers to, in the order the palette lists them.
pub const COMMANDS: &[Command] = &[
    Command {
        label: "Focus the selected row",
        keys: "Enter",
        key: KeyCode::Enter,
    },
    Command {
        label: "Filter the tree",
        keys: "/",
        key: KeyCode::Char('/'),
    },
    Command {
        label: "Next needing attention",
        keys: "n",
        key: KeyCode::Char('n'),
    },
    Command {
        label: "Previous needing attention",
        keys: "N",
        key: KeyCode::Char('N'),
    },
    Command {
        label: "Next working",
        keys: "w",
        key: KeyCode::Char('w'),
    },
    Command {
        label: "Previous working",
        keys: "W",
        key: KeyCode::Char('W'),
    },
    Command {
        label: "Fold or unfold the selected row",
        keys: "space",
        key: KeyCode::Char(' '),
    },
    Command {
        label: "Collapse the selected row",
        keys: "Left",
        key: KeyCode::Left,
    },
    Command {
        label: "Expand the selected row",
        keys: "Right",
        key: KeyCode::Right,
    },
    Command {
        label: "Cycle the pane view (agents, running, all)",
        keys: "p",
        key: KeyCode::Char('p'),
    },
    Command {
        label: "Cycle the tree order",
        keys: "s",
        key: KeyCode::Char('s'),
    },
    Command {
        label: "Show or hide finished sessions",
        keys: "e",
        key: KeyCode::Char('e'),
    },
    Command {
        label: "Show or hide background tasks",
        keys: "b",
        key: KeyCode::Char('b'),
    },
    Command {
        label: "Show or hide the details panel",
        keys: "d",
        key: KeyCode::Char('d'),
    },
    Command {
        label: "Move focus between tree and details",
        keys: "Tab",
        key: KeyCode::Tab,
    },
    Command {
        label: "Show or dismiss notices",
        keys: "!",
        key: KeyCode::Char('!'),
    },
    Command {
        label: "Open the sidebar beside this window",
        keys: "S",
        key: KeyCode::Char('S'),
    },
    Command {
        label: "Hide or unhide the sidebar",
        keys: "B",
        key: KeyCode::Char('B'),
    },
    Command {
        label: "New workspace",
        keys: "A",
        key: KeyCode::Char('A'),
    },
    Command {
        label: "Launch in the configured pane",
        keys: "L",
        key: KeyCode::Char('L'),
    },
    Command {
        label: "Restart the selected agent",
        keys: "r",
        key: KeyCode::Char('r'),
    },
    Command {
        label: "Create a managed container",
        keys: "a",
        key: KeyCode::Char('a'),
    },
    Command {
        label: "Reload the selected container",
        keys: "R",
        key: KeyCode::Char('R'),
    },
    Command {
        label: "Set presence status",
        keys: "E",
        key: KeyCode::Char('E'),
    },
    Command {
        label: "Delete the selected container",
        keys: "D",
        key: KeyCode::Char('D'),
    },
];

/// The commands a picker offers, in its own order. Filtering and ranking are the
/// picker's; this only decides what is on offer where.
pub fn items(sidebar: bool, compact: bool, modal: bool) -> Vec<Item<KeyCode>> {
    COMMANDS
        .iter()
        .filter(|command| command.offered(sidebar, compact))
        .filter(|command| !modal || command.opens_modal())
        .map(|command| Item::new(command.label, Some(command.keys), command.key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::{Picker, Step};
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn offered(sidebar: bool, query: &str) -> Vec<String> {
        offered_at(sidebar, true, query)
    }

    fn offered_at(sidebar: bool, compact: bool, query: &str) -> Vec<String> {
        let mut picker = Picker::new("Commands", "", items(sidebar, compact, false));
        for ch in query.chars() {
            picker.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        picker
            .rows()
            .into_iter()
            .map(|(label, _)| label.to_string())
            .collect()
    }

    #[test]
    fn every_command_names_the_key_it_runs_and_the_table_order_survives_an_empty_query() {
        for command in COMMANDS {
            assert!(!command.label.is_empty());
            assert!(!command.keys.is_empty());
        }
        assert_eq!(offered(false, "")[0], "Focus the selected row");
    }

    #[test]
    fn the_workspace_popup_lists_only_the_commands_that_open_a_modal() {
        let mut picker = Picker::new("Commands", "", items(false, false, true));
        let labels: Vec<String> = picker
            .rows()
            .into_iter()
            .map(|(label, _)| label.to_string())
            .collect();
        assert!(labels.iter().any(|label| label.contains("Restart")));
        assert!(labels.iter().any(|label| label.contains("Delete")));
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("Fold") || label.contains("pane view")),
            "a view the popup does not show has nothing to fold: {labels:?}"
        );
        let _ = &mut picker;
    }

    #[test]
    fn the_sidebar_offers_only_what_it_draws() {
        let sidebar = offered(true, "");
        assert!(sidebar.contains(&"Hide or unhide the sidebar".to_string()));
        assert!(sidebar.iter().any(|label| label.contains("Restart")));
        assert!(!sidebar.iter().any(|label| label.contains("details panel")));
        // Zoom restores the details panel; management is available at either width.
        let zoomed = offered_at(true, false, "");
        assert!(zoomed.iter().any(|label| label.contains("details panel")));
        assert!(zoomed.iter().any(|label| label.contains("Restart")));
        assert!(zoomed.iter().any(|label| label.contains("workspace")));
        assert!(
            !offered(true, "restart").is_empty(),
            "the sidebar manages the fleet"
        );
        assert!(
            !offered(false, "").contains(&"Hide or unhide the sidebar".to_string()),
            "and no sidebar command outside it"
        );
        assert!(
            !offered(false, "restart").is_empty(),
            "the dashboard keeps them"
        );
    }

    #[test]
    fn choosing_a_command_answers_with_its_key() {
        let mut picker = Picker::new("Commands", "", items(false, false, false));
        for ch in "fold".chars() {
            picker.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(
            picker.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Step::Choose(KeyCode::Char(' '))
        );
    }
}
