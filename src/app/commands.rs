//! The dashboard's commands: one typed identity per command, the chords that run
//! it and the palette that lists it.
//!
//! The table is the single source for a palette row, the chord written beside it
//! and the keymap that resolves a key to its command. A command runs by identity,
//! so choosing one from the palette never replays a key.

use std::str::FromStr;
use std::sync::OnceLock;

use crossterm::event::KeyCode;
use strum::{AsRefStr, EnumIter, EnumString};
use tui_actions::{Action, Key, KeyEvent, Keymap, Mode};

use crate::picker::Item;

/// What the dashboard can be asked to do. The variant name is the id the keymap
/// stores, so renaming a variant changes that id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, EnumString, AsRefStr)]
pub enum CommandId {
    Focus,
    Filter,
    NextAttention,
    PreviousAttention,
    NextWorking,
    PreviousWorking,
    Fold,
    Collapse,
    Expand,
    PaneView,
    TreeOrder,
    FinishedSessions,
    BackgroundTasks,
    Details,
    FocusSwap,
    Notices,
    OpenSidebar,
    HideSidebar,
    NewWorkspace,
    LaunchConfigured,
    Restart,
    CreateContainer,
    ReloadContainer,
    PresenceStatus,
    Delete,
}

/// One command: what it is called, the chords that run it and how it is offered.
pub struct Command {
    pub id: CommandId,
    pub label: &'static str,
    /// The chords that run the command. The first is the one written beside it.
    pub chords: &'static [&'static str],
}

impl Command {
    /// Whether this command exists only where `radar sidebar` is drawn.
    pub fn sidebar_only(&self) -> bool {
        self.id == CommandId::HideSidebar
    }

    /// Whether this command is about the sidebar's own split, so the sidebar
    /// itself cannot offer it.
    pub fn launches_sidebar(&self) -> bool {
        self.id == CommandId::OpenSidebar
    }

    /// Whether this command opens a picker or a confirmation, which is all the
    /// workspace popup is for: the rest act on a view the popup does not show.
    pub fn opens_modal(&self) -> bool {
        matches!(
            self.id,
            CommandId::LaunchConfigured
                | CommandId::Restart
                | CommandId::CreateContainer
                | CommandId::NewWorkspace
                | CommandId::PresenceStatus
                | CommandId::ReloadContainer
                | CommandId::Delete
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
        if sidebar && compact && matches!(self.id, CommandId::Details | CommandId::FocusSwap) {
            return false;
        }
        true
    }
}

/// Every command the fleet view answers to, in the order the palette lists them.
pub const COMMANDS: &[Command] = &[
    Command {
        id: CommandId::Focus,
        label: "Focus the selected row",
        chords: &["Enter"],
    },
    Command {
        id: CommandId::Filter,
        label: "Filter the tree",
        chords: &["/"],
    },
    Command {
        id: CommandId::NextAttention,
        label: "Next needing attention",
        chords: &["n"],
    },
    Command {
        id: CommandId::PreviousAttention,
        label: "Previous needing attention",
        chords: &["N"],
    },
    Command {
        id: CommandId::NextWorking,
        label: "Next working",
        chords: &["w"],
    },
    Command {
        id: CommandId::PreviousWorking,
        label: "Previous working",
        chords: &["W"],
    },
    Command {
        id: CommandId::Fold,
        label: "Fold or unfold the selected row",
        chords: &["space"],
    },
    Command {
        id: CommandId::Collapse,
        label: "Collapse the selected row",
        chords: &["Left"],
    },
    Command {
        id: CommandId::Expand,
        label: "Expand the selected row",
        chords: &["Right"],
    },
    Command {
        id: CommandId::PaneView,
        label: "Cycle the pane view (agents, running, all)",
        chords: &["p"],
    },
    Command {
        id: CommandId::TreeOrder,
        label: "Cycle the tree order",
        chords: &["s"],
    },
    Command {
        id: CommandId::FinishedSessions,
        label: "Show or hide history: exited, finished and unresolved rows",
        chords: &["e"],
    },
    Command {
        id: CommandId::BackgroundTasks,
        label: "Show or hide background tasks",
        chords: &["b"],
    },
    Command {
        id: CommandId::Details,
        label: "Show or hide the details panel",
        chords: &["d"],
    },
    Command {
        id: CommandId::FocusSwap,
        label: "Move focus between tree and details",
        chords: &["Tab", "BackTab"],
    },
    Command {
        id: CommandId::Notices,
        label: "Show or dismiss notices",
        chords: &["!"],
    },
    Command {
        id: CommandId::OpenSidebar,
        label: "Open the sidebar beside this window",
        chords: &["S"],
    },
    Command {
        id: CommandId::HideSidebar,
        label: "Hide or unhide the sidebar",
        chords: &["B"],
    },
    Command {
        id: CommandId::NewWorkspace,
        label: "New workspace",
        chords: &["A"],
    },
    Command {
        id: CommandId::LaunchConfigured,
        label: "Launch in the configured pane",
        chords: &["L"],
    },
    Command {
        id: CommandId::Restart,
        label: "Restart the selected agent",
        chords: &["r"],
    },
    Command {
        id: CommandId::CreateContainer,
        label: "Create a managed container",
        chords: &["a"],
    },
    Command {
        id: CommandId::ReloadContainer,
        label: "Reload the selected container",
        chords: &["R"],
    },
    Command {
        id: CommandId::PresenceStatus,
        label: "Set presence status",
        chords: &["E"],
    },
    Command {
        id: CommandId::Delete,
        label: "Delete the selected container",
        chords: &["D"],
    },
];

/// The command a key runs in the dashboard, or `None` when the key is not one of
/// them. Modifiers are ignored, as the dashboard's keys have always ignored them.
pub fn resolve(code: KeyCode) -> Option<CommandId> {
    let hits = keymap().resolve(&KeyEvent::plain(key_of(code)?));
    hits.first()
        .and_then(|action| CommandId::from_str(action.id()).ok())
}

/// The commands a picker offers, in its own order. Filtering and ranking are the
/// picker's; this only decides what is on offer where.
pub fn items(sidebar: bool, compact: bool, modal: bool) -> Vec<Item<CommandId>> {
    COMMANDS
        .iter()
        .filter(|command| command.offered(sidebar, compact))
        .filter(|command| !modal || command.opens_modal())
        .map(|command| Item::new(command.label, command.chords.first().copied(), command.id))
        .collect()
}

/// The keymap built from [`COMMANDS`]. A bad table is a programming error caught
/// by the first key press, so the binding is checked here rather than per key.
fn keymap() -> &'static Keymap {
    static KEYMAP: OnceLock<Keymap> = OnceLock::new();
    KEYMAP.get_or_init(|| {
        let mut keymap = Keymap::new(Mode::Normal);
        for command in COMMANDS {
            for chord in command.chords {
                keymap
                    .bind(chord, Action::new(command.id.as_ref()), command.label)
                    .expect("the command table binds each chord once");
            }
        }
        keymap
    })
}

fn key_of(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::{Picker, Step};
    use crossterm::event::{KeyEvent as TermKeyEvent, KeyModifiers};
    use strum::IntoEnumIterator;

    fn offered(sidebar: bool, query: &str) -> Vec<String> {
        offered_at(sidebar, true, query)
    }

    fn offered_at(sidebar: bool, compact: bool, query: &str) -> Vec<String> {
        let mut picker = Picker::new("Commands", "", items(sidebar, compact, false));
        for ch in query.chars() {
            picker.handle_key(TermKeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        picker
            .rows()
            .into_iter()
            .map(|(label, _)| label.to_string())
            .collect()
    }

    #[test]
    fn every_command_is_listed_once_with_a_chord_and_the_table_order_survives() {
        for id in CommandId::iter() {
            let listed: Vec<&Command> = COMMANDS.iter().filter(|c| c.id == id).collect();
            assert_eq!(listed.len(), 1, "{id:?} needs exactly one table entry");
            assert!(!listed[0].chords.is_empty(), "{id:?} names no chord");
        }
        assert_eq!(offered(false, "")[0], "Focus the selected row");
    }

    #[test]
    fn keys_resolve_to_their_commands_case_sensitively() {
        assert_eq!(resolve(KeyCode::Char('n')), Some(CommandId::NextAttention));
        assert_eq!(
            resolve(KeyCode::Char('N')),
            Some(CommandId::PreviousAttention)
        );
        assert_eq!(resolve(KeyCode::Char(' ')), Some(CommandId::Fold));
        assert_eq!(resolve(KeyCode::BackTab), Some(CommandId::FocusSwap));
        assert_eq!(
            resolve(KeyCode::Char('j')),
            None,
            "j moves, it is not a command"
        );
    }

    #[test]
    fn the_workspace_popup_lists_only_the_commands_that_open_a_modal() {
        let picker = Picker::new("Commands", "", items(false, false, true));
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
    fn choosing_a_command_answers_with_its_identity() {
        let mut picker = Picker::new("Commands", "", items(false, false, false));
        for ch in "fold".chars() {
            picker.handle_key(TermKeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(
            picker.handle_key(TermKeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Step::Choose(CommandId::Fold)
        );
    }
}
