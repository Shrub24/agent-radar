//! The process table's local selection, folding and navigation.

use super::*;

impl App {
    /// The process table of one fleet row, or `None` where the page draws none.
    pub(crate) fn process_table(&self, row: &VisibleRow<'_>) -> Option<ProcessTable> {
        let Some(ForegroundEvidence::NonShell { local, .. }) = described_foreground(row) else {
            return None;
        };
        ProcessTable::of(&self.process.folded, local.resources.as_ref()?)
    }

    /// The process table of the row the page is showing: what the page draws, and
    /// the rows the reader's cursor moves through.
    pub(crate) fn process_table_on_page(&self) -> Option<ProcessTable> {
        let row = self.selected_row()?;
        self.process_table(&row)
    }

    /// The process the page's compact details describe, when the reader has
    /// picked one that is not the row's own process.
    pub(crate) fn process_selection(&self) -> Option<&ProcessIdentity> {
        self.process.selected.as_ref()
    }

    /// The process the table marks as the reader's row: the picked process, or
    /// the table's own process when they have picked none.
    pub(crate) fn process_cursor_on(&self, table: &ProcessTable) -> ProcessIdentity {
        self.process
            .selected
            .clone()
            .unwrap_or_else(|| table.root.clone())
    }

    /// Whether process-row navigation is on.
    pub(crate) fn process_navigating(&self) -> bool {
        self.process.navigating
    }

    /// The branches the reader folded, by birth identity.
    pub(crate) fn process_folded(&self) -> &HashSet<ProcessIdentity> {
        &self.process.folded
    }

    /// `t`: enters or leaves process-row navigation. Leaving is always allowed;
    /// entering needs the keyboard, the Processes page and the table open, so a
    /// page with no table has no mode to enter.
    pub fn toggle_process_navigation(&mut self) {
        if self.process.navigating {
            self.process.navigating = false;
            return;
        }
        if !self.details_focused
            || self.detail_page != DetailPage::Processes
            || !self
                .expanded_disclosures
                .contains(&Disclosure::ProcessTable)
        {
            return;
        }
        let Some(table) = self.process_table_on_page() else {
            return;
        };
        self.process.navigating = true;
        self.process.root = Some(table.root.clone());
    }

    /// `j`/`k` and the arrows in the mode: the next or previous drawn row. A move
    /// past either end stays where it is — the mode never wraps the table.
    pub fn move_process(&mut self, delta: i32) {
        let Some(table) = self.process_table_on_page() else {
            return;
        };
        let rows: Vec<ProcessIdentity> = table.visible().map(|row| row.identity.clone()).collect();
        let cursor = self.process_cursor_on(&table);
        let next = match rows.iter().position(|row| *row == cursor) {
            Some(index) if delta < 0 => index.saturating_sub(1),
            Some(index) => (index + 1).min(rows.len().saturating_sub(1)),
            None => 0,
        };
        if let Some(identity) = rows.get(next) {
            self.select_process(Some(identity.clone()));
        }
    }

    /// Home/End in the mode: the first or the last drawn row.
    pub fn reach_process(&mut self, last: bool) {
        let Some(table) = self.process_table_on_page() else {
            return;
        };
        let row = if last {
            table.visible().last()
        } else {
            table.visible().next()
        };
        if let Some(row) = row {
            self.select_process(Some(row.identity.clone()));
        }
    }

    /// Enter and Space in the mode: folds the selected row's branch away, or
    /// opens it again. A row with no process beneath it has no branch to fold.
    pub fn fold_selected_process(&mut self) {
        let Some(table) = self.process_table_on_page() else {
            return;
        };
        let cursor = self.process_cursor_on(&table);
        if !table.row(&cursor).is_some_and(|row| row.has_children) {
            return;
        }
        self.fold_process(&cursor);
    }

    /// Folds or opens one branch, by identity. The cursor stays on a row the
    /// table draws: a process the fold put out of sight is selected as its
    /// branch.
    pub fn fold_process(&mut self, identity: &ProcessIdentity) {
        if !self.process.folded.remove(identity) {
            self.process.folded.insert(identity.clone());
        }
        self.anchor_process();
    }

    /// Selects a process row by identity. The row is scrolled into sight from
    /// the page the *next* draw produces, because a selection's own compact
    /// block changes the page it is drawn on. The row's own process is the
    /// page's root: picked or not, the page draws it the same way.
    pub(super) fn select_process(&mut self, identity: Option<ProcessIdentity>) {
        let root = self.process_table_on_page().map(|table| table.root);
        self.process.selected = match (identity, root) {
            (Some(identity), Some(root)) if identity == root => None,
            (identity, _) => identity,
        };
        self.process.reveal = true;
    }

    /// Keeps the cursor on a drawn row: a selected process that is gone, or that
    /// a fold put out of sight, is selected as its branch — the table's own
    /// process when nothing above it is left.
    pub(super) fn anchor_process(&mut self) {
        let Some(table) = self.process_table_on_page() else {
            return;
        };
        let Some(selected) = self.process.selected.clone() else {
            return;
        };
        let anchor = table.anchor(&selected).clone();
        self.process.selected = (anchor != table.root).then_some(anchor);
    }

    /// Scrolls the panel the least that puts the cursor's row in sight, measured
    /// from the layout of the last draw. A refresh never comes here: the viewport
    /// is moved by a key, not by the fleet changing under it.
    pub(super) fn reveal_process(&mut self) {
        let viewport = self.layout.details_viewport as usize;
        if viewport == 0 {
            return;
        }
        let Some(table) = self.process_table_on_page() else {
            return;
        };
        let cursor = self.process_cursor_on(&table);
        let Some(&(_, row, _)) = self
            .layout
            .process_rows
            .iter()
            .find(|(identity, _, _)| *identity == cursor)
        else {
            return;
        };
        let offset = self.details_scroll() as usize;
        let target = if row < offset {
            row
        } else if row + 1 > offset + viewport {
            row + 1 - viewport
        } else {
            return;
        };
        self.scroll_page_to(target as u16);
    }

    /// Brings the reader's place in the process table onto the observation just
    /// applied: the folds and the cursor are about the processes they were made
    /// in, and a process that replaced one of them inherits none of it.
    pub(super) fn reconcile_process(&mut self) {
        let Some(table) = self.process_table_on_page() else {
            // The page draws no table now: there is no row for a cursor to be on,
            // and the folds were about a tree that is no longer observed.
            self.process = ProcessCursor::default();
            return;
        };
        if self.process.root.as_ref() != Some(&table.root) {
            // The page's own process is another process now.
            self.process = ProcessCursor {
                root: Some(table.root.clone()),
                ..ProcessCursor::default()
            };
            return;
        }
        let observed: HashSet<&ProcessIdentity> =
            table.rows.iter().map(|row| &row.identity).collect();
        self.process
            .folded
            .retain(|identity| observed.contains(identity));
        self.anchor_process();
    }

    /// A left click on the process table: a fold marker folds that branch alone,
    /// and the row it landed on is selected and entered. Both hand the details
    /// the keyboard, and neither acts on the fleet or on a process.
    pub(super) fn process_click(&mut self, at: (u16, u16)) -> bool {
        if let Some(identity) = self.process_fold_under(at) {
            self.details_focused = true;
            self.fold_process(&identity);
            return true;
        }
        let Some(identity) = self.process_row_under(at) else {
            return false;
        };
        self.details_focused = true;
        self.select_process(Some(identity));
        self.process.navigating = true;
        true
    }

    /// The process row a position lands on, from the rectangles the last draw
    /// reported. A rectangle belongs to the draw that recorded it, so the row is
    /// resolved against the table the page draws now: a row a refresh dropped is
    /// not something a click can land on.
    pub(super) fn process_row_under(&self, at: (u16, u16)) -> Option<ProcessIdentity> {
        let identity = self
            .layout
            .process_rows
            .iter()
            .find(|(_, _, rect)| inside(*rect, at))?
            .0
            .clone();
        self.table_has(&identity).then_some(identity)
    }

    /// The fold marker a position lands on, when the row it labels is still
    /// drawn.
    pub(super) fn process_fold_under(&self, at: (u16, u16)) -> Option<ProcessIdentity> {
        let identity = self
            .layout
            .process_folds
            .iter()
            .find(|(_, rect)| inside(Some(*rect), at))?
            .0
            .clone();
        self.table_has(&identity).then_some(identity)
    }

    /// Whether the table the page draws now still has this row, drawn.
    pub(super) fn table_has(&self, identity: &ProcessIdentity) -> bool {
        self.process_table_on_page()
            .is_some_and(|table| table.visible().any(|row| &row.identity == identity))
    }
}
