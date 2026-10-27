//! The popup picker the dashboard uses for its own choices: a query, the items
//! it selects and the highlighted one.
//!
//! It holds no widgets and no terminal: one [`Picker`] answers keys and reports
//! what was chosen, and `ui::picker` draws whatever it holds. The command palette
//! and the workspace chooser are the same element with different items.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One thing to choose: what it is called, the key or shortcut written beside
/// it, and what choosing it means.
#[derive(Debug)]
pub struct Item<T> {
    label: String,
    keys: Option<String>,
    value: T,
}

impl<T> Item<T> {
    pub fn new(label: impl Into<String>, keys: Option<impl Into<String>>, value: T) -> Self {
        Self {
            label: label.into(),
            keys: keys.map(Into::into),
            value,
        }
    }
}

/// What a key did to the picker.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<T> {
    /// Still open.
    Continue,
    /// Closed without choosing.
    Close,
    /// Closed with this choice.
    Choose(T),
}

/// An open picker.
#[derive(Debug)]
pub struct Picker<T> {
    title: &'static str,
    hint: String,
    query: String,
    index: usize,
    items: Vec<Item<T>>,
}

impl<T> Picker<T> {
    pub fn new(title: &'static str, hint: impl Into<String>, items: Vec<Item<T>>) -> Self {
        Self {
            title,
            hint: hint.into(),
            query: String::new(),
            index: 0,
            items,
        }
    }

    pub fn title(&self) -> &'static str {
        self.title
    }

    pub fn hint(&self) -> &str {
        &self.hint
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Index of the highlighted row among the displayed ones.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The items the query selects, best first.
    pub fn visible(&self) -> Vec<usize> {
        matching(&self.items, &self.query)
    }

    /// The displayed rows: a label and, when it has one, the shortcut beside it.
    pub fn rows(&self) -> Vec<(&str, Option<&str>)> {
        self.visible()
            .into_iter()
            .map(|index| {
                let item = &self.items[index];
                (item.label.as_str(), item.keys.as_deref())
            })
            .collect()
    }

    /// Handles one key. While a picker is open it owns the keyboard: letters
    /// edit the query, `Up`/`Down` (or `Ctrl+p`/`Ctrl+n`) move the highlight, and
    /// `Enter` chooses.
    pub fn handle_key(&mut self, key: KeyEvent) -> Step<T>
    where
        T: 'static,
    {
        match key.code {
            KeyCode::Esc => return Step::Close,
            KeyCode::Enter => {
                let visible = self.visible();
                let Some(index) = visible.get(self.index).copied() else {
                    return Step::Continue;
                };
                return Step::Choose(self.items.remove(index).value);
            }
            KeyCode::Up | KeyCode::BackTab => self.index = self.index.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.index += 1,
            KeyCode::Backspace => {
                self.query.pop();
                self.index = 0;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => self.index += 1,
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.index = self.index.saturating_sub(1);
            }
            KeyCode::Char(ch) => {
                self.query.push(ch);
                self.index = 0;
            }
            _ => {}
        }
        self.index = self.index.min(self.visible().len().saturating_sub(1));
        Step::Continue
    }
}

/// The items a query selects, best first: a label that starts with the query
/// ranks above one whose word starts line up, and that above one that merely
/// holds the letters in order. Ties keep the order the items were given in.
fn matching<T>(items: &[Item<T>], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(u8, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| score(&item.label, &query).map(|score| (score, index)))
        .collect();
    scored.sort_by_key(|(score, _)| *score);
    scored.into_iter().map(|(_, index)| index).collect()
}

/// How well `label` answers `query`, lower being better: 0 for a prefix, 1 when
/// every letter starts a word, 2 when the letters appear in order anywhere.
/// `None` when they do not.
fn score(label: &str, query: &str) -> Option<u8> {
    let label = label.to_lowercase();
    if label.starts_with(query) {
        return Some(0);
    }
    let mut letters = query.chars();
    let mut wanted = letters.next()?;
    let mut at_word = 0;
    for (at, ch) in label.char_indices() {
        if ch != wanted {
            continue;
        }
        if at == 0 || matches!(label.as_bytes()[at - 1], b' ' | b'-' | b'/') {
            at_word += 1;
        }
        match letters.next() {
            Some(next) => wanted = next,
            None => {
                return Some(if at_word == query.chars().count() {
                    1
                } else {
                    2
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(labels: &[(&'static str, &'static str)]) -> Picker<&'static str> {
        Picker::new(
            "Pick",
            "enter choose",
            labels
                .iter()
                .map(|(label, value)| Item::new(*label, None::<&str>, *value))
                .collect(),
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn labels<T>(picker: &Picker<T>) -> Vec<String> {
        picker
            .rows()
            .into_iter()
            .map(|(label, _)| label.to_string())
            .collect()
    }

    #[test]
    fn a_query_ranks_a_prefix_above_word_starts_and_above_letters_in_order() {
        let all = picker(&[
            ("Show or hide finished sessions", "letters"),
            ("Filter the tree", "prefix"),
            ("Fold or unfold the selected row", "letters2"),
            ("Fix the failing test", "words"),
        ]);
        assert_eq!(
            labels(&all),
            [
                "Show or hide finished sessions",
                "Filter the tree",
                "Fold or unfold the selected row",
                "Fix the failing test",
            ],
            "the given order, until a query reorders it"
        );

        let mut query = picker(&[
            ("Show or hide finished sessions", "letters"),
            ("Filter the tree", "prefix"),
            ("Fix the failing test", "words"),
        ]);
        for ch in "fi".chars() {
            assert_eq!(query.handle_key(key(KeyCode::Char(ch))), Step::Continue);
        }
        assert_eq!(
            labels(&query),
            [
                "Filter the tree",
                "Fix the failing test",
                "Show or hide finished sessions"
            ],
            "a prefix first, then word starts, then letters anywhere"
        );
    }

    #[test]
    fn the_highlight_stays_inside_the_query_and_enter_chooses_that_row() {
        let mut query = picker(&[("one", "1"), ("two", "2"), ("three", "3")]);
        query.handle_key(key(KeyCode::Down));
        query.handle_key(key(KeyCode::Down));
        assert_eq!(query.index(), 2);
        // A query narrows the list, and the highlight returns to its top.
        for ch in "th".chars() {
            query.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(labels(&query), ["three"]);
        assert_eq!(query.index(), 0);
        assert_eq!(query.handle_key(key(KeyCode::Enter)), Step::Choose("3"));

        let mut none = picker(&[("one", "1")]);
        for ch in "zzz".chars() {
            none.handle_key(key(KeyCode::Char(ch)));
        }
        assert!(labels(&none).is_empty());
        assert_eq!(
            none.handle_key(key(KeyCode::Enter)),
            Step::Continue,
            "nothing to choose"
        );
        assert_eq!(none.handle_key(key(KeyCode::Esc)), Step::Close);
    }
}
