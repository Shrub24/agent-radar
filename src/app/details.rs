//! Detail pages, their scrolling and the disclosure blocks inside them.

use super::*;

impl App {
    /// How far the page on screen is scrolled, in the rows the panel draws.
    pub fn details_scroll(&self) -> u16 {
        self.detail_scroll[self.detail_page.index()]
    }

    /// The page of the selected row the panel shows.
    pub fn detail_page(&self) -> DetailPage {
        self.detail_page
    }

    /// Shows a page. Every page keeps its own scroll, so returning to one finds
    /// it where it was left. The process-row mode belongs to the Processes page:
    /// showing another page leaves it.
    pub fn select_page(&mut self, page: DetailPage) {
        if page != self.detail_page {
            self.process.navigating = false;
        }
        self.detail_page = page;
        self.clamp_disclosure_target();
    }

    /// `←`/`→` in the details: the previous or next page, wrapping at the ends.
    pub fn cycle_page(&mut self, forward: bool) {
        self.select_page(if forward {
            self.detail_page.next()
        } else {
            self.detail_page.previous()
        });
    }

    /// Whether the details panel holds the keyboard.
    pub fn details_focused(&self) -> bool {
        self.details_focused
    }

    /// Tab: hands the keyboard to the other panel. A hidden details panel is not
    /// a panel to hand it to, so the tree keeps it. The process-row mode needs
    /// the keyboard in this panel, so handing it over leaves the mode.
    pub fn toggle_detail_focus(&mut self) {
        if self.shows_details() {
            self.details_focused = !self.details_focused;
            self.process.navigating = false;
        }
    }

    /// Scrolls the page on screen by `rows`, clamped to the rows the last draw
    /// reported.
    pub fn scroll_page(&mut self, rows: i32) {
        let max = self.scroll_max() as i32;
        let page = self.detail_page.index();
        self.detail_scroll[page] = (self.detail_scroll[page] as i32 + rows).clamp(0, max) as u16;
    }

    /// Scrolls the page on screen to `offset`, clamped like [`Self::scroll_page`].
    pub fn scroll_page_to(&mut self, offset: u16) {
        let max = self.scroll_max();
        let page = self.detail_page.index();
        self.detail_scroll[page] = offset.min(max);
    }

    /// How far the page on screen can be scrolled: what is left of its content
    /// once the viewport is full.
    pub(super) fn scroll_max(&self) -> u16 {
        self.layout
            .details_rows
            .saturating_sub(self.layout.details_viewport as usize) as u16
    }

    /// What one `PageUp`/`PageDown` moves.
    pub(super) fn page_step(&self) -> i32 {
        self.layout.details_viewport.max(1) as i32
    }

    /// The blocks the page on screen offers, in the order it draws them.
    pub fn disclosures(&self) -> Vec<Disclosure> {
        self.disclosures_on(self.detail_page)
    }

    /// The blocks `page` offers, in the order it draws them.
    pub(super) fn disclosures_on(&self, page: DetailPage) -> Vec<Disclosure> {
        let Some(row) = self.selected_row() else {
            return Vec::new();
        };
        match page {
            DetailPage::Overview => match &row.node.row.kind {
                RowKind::Agent(agent)
                    if agent
                        .facts
                        .assignment
                        .as_deref()
                        .is_some_and(assignment_is_long) =>
                {
                    vec![Disclosure::Assignment]
                }
                _ => Vec::new(),
            },
            DetailPage::Tasks => collapsible_tasks(&row)
                .into_iter()
                .map(Disclosure::Task)
                .collect(),
            // The blocks hold the facts of a process, and the page draws those
            // only from a current inventory: a stale-source row offers nothing
            // to open, so no key answers to a marker that is not drawn.
            DetailPage::Processes if self.source_current && process_detail_is_drawn(&row) => {
                let mut blocks = vec![Disclosure::Process];
                if process_table_is_drawn(&row) {
                    blocks.push(Disclosure::ProcessTable);
                }
                blocks
            }
            DetailPage::Processes | DetailPage::Source => Vec::new(),
        }
    }

    /// Whether a block is open. A block the selected row no longer offers reads
    /// closed.
    pub fn disclosure_open(&self, key: &Disclosure) -> bool {
        self.expanded_disclosures.contains(key)
    }

    /// The block the keyboard is on, among the page's.
    pub fn disclosure_target(&self) -> Option<Disclosure> {
        self.disclosures().get(self.disclosure_target).cloned()
    }

    /// Space: the page's next block, wrapping. A page with nothing to open
    /// moves nothing.
    pub fn cycle_disclosure(&mut self) {
        let count = self.disclosures().len();
        if count > 0 {
            self.disclosure_target = (self.disclosure_target + 1) % count;
        }
    }

    /// Enter, and a click on a marker: opens or closes one block, and leaves the
    /// fleet alone.
    pub fn toggle_disclosure(&mut self) {
        if let Some(key) = self.disclosure_target() {
            self.toggle_block(&key);
        }
    }

    /// Opens or closes one named block, and puts the keyboard on it. Closing the
    /// process table leaves the process-row mode: the mode is the table's.
    pub fn toggle_block(&mut self, key: &Disclosure) {
        let open = !self.expanded_disclosures.remove(key);
        if open {
            self.expanded_disclosures.insert(key.clone());
        } else if key == &Disclosure::ProcessTable {
            self.process.navigating = false;
        }
        let keys = self.disclosures();
        if let Some(index) = keys.iter().position(|offered| offered == key) {
            self.disclosure_target = index;
        }
    }

    /// The block whose marker was drawn at a position, if any.
    pub(super) fn disclosure_marker_under(&self, at: (u16, u16)) -> Option<Disclosure> {
        self.layout
            .disclosure_markers
            .iter()
            .find(|(_, marker)| inside(Some(*marker), at))
            .map(|(key, _)| key.clone())
    }

    /// Drops the expansions the selected row no longer offers: a task that
    /// vanished takes its expansion with it, and one that survives keeps it.
    pub(super) fn prune_disclosures(&mut self) {
        let offered: Vec<Disclosure> = DetailPage::ALL
            .into_iter()
            .flat_map(|page| self.disclosures_on(page))
            .collect();
        self.expanded_disclosures
            .retain(|key| offered.contains(key));
        self.clamp_disclosure_target();
    }

    /// Keeps the keyboard's block inside the blocks the page on screen offers,
    /// so a marker is always drawn for the block Enter would open.
    pub(super) fn clamp_disclosure_target(&mut self) {
        let count = self.disclosures().len();
        self.disclosure_target = if count == 0 {
            0
        } else {
            self.disclosure_target.min(count - 1)
        };
    }

    /// Shows or hides the details panel. A hidden panel cannot hold the
    /// keyboard, so hiding it hands the keyboard back to the tree.
    pub fn toggle_details(&mut self) {
        self.details_hidden = !self.details_hidden;
        if self.details_hidden {
            self.details_focused = false;
            self.process.navigating = false;
        }
    }
}
