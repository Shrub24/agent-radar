//! Mouse input: wheel, click and the row and tab hit tests.

use super::*;

impl App {
    /// The scroll a wheel turn asked the tree for, taken once by the main loop.
    pub fn take_scroll(&mut self) -> Option<usize> {
        self.scroll_request.take()
    }

    /// Applies one mouse event: the wheel scrolls the panel under the pointer,
    /// a left click selects the row it lands on, folds a heading it lands on, or
    /// focuses a row that was already selected. A position holding no row does
    /// nothing.
    pub fn handle_mouse(&mut self, event: MouseEvent) -> Option<Action> {
        let at = (event.column, event.row);
        // A picker is modal for the pointer too: a click behind it reaches
        // nothing.
        if self.picker_open() {
            return None;
        }
        if self.confirmation.is_some() {
            // The dialog is modal: the tree behind it answers no pointer event.
            return match event.kind {
                MouseEventKind::Down(MouseButton::Left) => self.confirmation_click(at),
                _ => None,
            };
        }
        match event.kind {
            MouseEventKind::ScrollDown => {
                self.wheel(at, true);
                None
            }
            MouseEventKind::ScrollUp => {
                self.wheel(at, false);
                None
            }
            MouseEventKind::Down(MouseButton::Left) => self.click(at),
            _ => None,
        }
    }

    /// A wheel turn: the page under the pointer scrolls its own rows, the tree
    /// moves the list's first drawn row, and a turn anywhere else does nothing.
    pub(super) fn wheel(&mut self, at: (u16, u16), down: bool) {
        const STEP: i32 = 3;
        if inside(self.layout.details, at) {
            self.scroll_page(if down { STEP } else { -STEP });
            return;
        }
        if !inside(Some(self.layout.tree_panel), at) {
            return;
        }
        let visible = self.layout.tree_content.height as usize;
        let max = self.visible_rows().len().saturating_sub(visible);
        let offset = self.layout.offset;
        let next = if down {
            (offset + STEP as usize).min(max)
        } else {
            offset.saturating_sub(STEP as usize)
        };
        if next != offset {
            self.scroll_request = Some(next);
        }
    }

    /// A left click. A heading folds whether or not it was selected; an agent
    /// branch's disclosure cell folds that branch and sends nothing; a row that
    /// was already selected is the one the click acts on.
    pub(super) fn click(&mut self, at: (u16, u16)) -> Option<Action> {
        // A page's tab is the page: clicking one shows it and hands the panel
        // the keyboard, which is what makes the tabs the pointer's way into the
        // details.
        if let Some(page) = self.page_tab_under(at) {
            self.detail_page = page;
            self.details_focused = true;
            return None;
        }
        // A marker is the block it labels: clicking one opens or closes that
        // block alone, and never acts on the row.
        if let Some(key) = self.disclosure_marker_under(at) {
            self.details_focused = true;
            self.toggle_block(&key);
            return None;
        }
        // A process row is the process it names, and a process row's marker is
        // that row's branch: neither acts on the fleet, the mux or the owner.
        if self.process_click(at) {
            return None;
        }
        if inside(self.layout.details, at) {
            // Reading a page is not acting on its row: a click in the panel
            // only hands it the keyboard.
            self.details_focused = true;
            return None;
        }
        if inside(Some(self.layout.tree_panel), at) {
            self.details_focused = false;
        }
        let (id, disclosure) = self.row_under(at)?;
        if matches!(id, RowId::Workspace(_)) {
            self.select(Some(id));
            self.toggle_fold();
            return None;
        }
        if disclosure {
            // The pointer said where, not which row to act on: the disclosure
            // cell folds this branch alone and leaves the backend alone.
            self.toggle_branch(&id);
            return None;
        }
        let already = self.selected.as_ref() == Some(&id);
        self.select(Some(id));
        if already {
            return self.focus_selected();
        }
        None
    }

    /// The page tab a position lands on, from the rectangles the last draw
    /// reported.
    pub(super) fn page_tab_under(&self, at: (u16, u16)) -> Option<DetailPage> {
        DetailPage::ALL
            .into_iter()
            .find(|page| inside(self.layout.detail_tabs[page.index()], at))
    }

    /// The row drawn at a position, and whether the position falls on its
    /// disclosure cell. A folded row's descendants are not drawn, so they
    /// cannot be under the pointer.
    pub(super) fn row_under(&self, at: (u16, u16)) -> Option<(RowId, bool)> {
        let content = self.layout.tree_content;
        if !inside(Some(content), at) {
            return None;
        }
        let index = self.layout.offset + (at.1 - content.y) as usize;
        let row = self.visible_rows().into_iter().nth(index)?;
        let column = at.0.saturating_sub(content.x);
        let marker = row.marker_column();
        let disclosure = row.has_children && (marker..marker + DISCLOSURE_WIDTH).contains(&column);
        Some((row.id.clone(), disclosure))
    }

    /// Folds or unfolds one branch by identity, without moving the selection.
    /// Ignored while a filter is active, as the fold keys are: the filtered
    /// view shows matching ancestry on purpose, so a fold could not take effect.
    pub(super) fn toggle_branch(&mut self, id: &RowId) {
        if self.needle().is_some() {
            return;
        }
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.clone());
        }
        self.reconcile_selection();
    }
}
