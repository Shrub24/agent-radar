//! The dashboard's commands and the keys that run them.
//!
//! One table is the single source for both the palette's list and what a chosen
//! entry does: an entry carries the key that performs it, and the palette runs
//! that key, so a hotkey shown here cannot drift from the binding.

use crossterm::event::KeyCode;

/// One command: what it does, the key that does it, and how that key is written.
pub struct Command {
    pub label: &'static str,
    pub keys: &'static str,
    pub key: KeyCode,
}

impl Command {
    /// Whether this command exists only where `radar sidebar` is drawn.
    pub fn sidebar_only(&self) -> bool {
        self.key == KeyCode::Char('B')
    }

    /// Whether this command is offered by a view. The sidebar omits the
    /// lifecycle actions and everything that needs a details panel, because it
    /// draws neither.
    pub fn offered(&self, sidebar: bool) -> bool {
        if self.sidebar_only() {
            return sidebar;
        }
        if !sidebar {
            return true;
        }
        !matches!(
            self.key,
            KeyCode::Char('r' | 'a' | 'A' | 'L' | 'E' | 'D' | 'R' | 'd')
                | KeyCode::Tab
                | KeyCode::BackTab
        )
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

/// The commands a query selects, best first: a label whose words start with the
/// query ranks above one that merely contains its letters in order.
pub fn matching(query: &str, sidebar: bool) -> Vec<&'static Command> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return COMMANDS
            .iter()
            .filter(|command| command.offered(sidebar))
            .collect();
    }
    let mut scored: Vec<(u8, &'static Command)> = COMMANDS
        .iter()
        .filter(|command| command.offered(sidebar))
        .filter_map(|command| score(command.label, &query).map(|score| (score, command)))
        .collect();
    // Stable, so commands matching equally well keep the table's order.
    scored.sort_by_key(|(score, _)| *score);
    scored.into_iter().map(|(_, command)| command).collect()
}

/// How well `label` answers `query`, lower being better: 0 for a prefix, 1 when
/// every word start in the query's letters line up, and the matched position
/// otherwise. `None` when the letters do not appear in order.
fn score(label: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    if label.starts_with(query) {
        return Some(0);
    }
    let mut letters = query.chars();
    let mut wanted = letters.next()?;
    let mut words = 0;
    for (at, ch) in label.chars().enumerate() {
        if ch == wanted {
            if at == 0 || label.as_bytes()[at - 1] == b' ' || label.as_bytes()[at - 1] == b'-' {
                words += 1;
            }
            match letters.next() {
                Some(next) => wanted = next,
                None => return Some(if words == query.chars().count() { 1 } else { 2 }),
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_selects_its_commands_in_order_of_match_and_sidebar_commands_stay_apart() {
        let labels = |query: &str| -> Vec<&'static str> {
            matching(query, false)
                .into_iter()
                .map(|command| command.label)
                .collect()
        };
        assert_eq!(labels("")[0], "Focus the selected row", "the table order");
        assert_eq!(labels("fold")[0], "Fold or unfold the selected row");
        assert_eq!(
            labels("attention"),
            ["Next needing attention", "Previous needing attention"]
        );
        assert!(
            !labels("").contains(&"Hide or unhide the sidebar"),
            "sidebar-only commands are absent outside the sidebar"
        );
        let sidebar = |query: &str| -> Vec<&'static str> {
            matching(query, true)
                .into_iter()
                .map(|command| command.label)
                .collect()
        };
        assert!(sidebar("").contains(&"Hide or unhide the sidebar"));
        assert_eq!(sidebar("restart"), Vec::<&str>::new(), "no lifecycle");
        assert_eq!(sidebar("workspace"), Vec::<&str>::new(), "no new workspace");
        assert_eq!(sidebar("details"), Vec::<&str>::new(), "no details panel");
        assert!(
            !labels("restart").is_empty(),
            "the dashboard still offers it"
        );
        assert!(!labels("zzz").iter().any(|label| label.contains("zzz")));
        assert_eq!(matching("zzz", false).len(), 0);
    }

    #[test]
    fn every_command_names_the_key_it_runs() {
        for command in COMMANDS {
            assert!(!command.label.is_empty());
            assert!(!command.keys.is_empty());
        }
    }
}
