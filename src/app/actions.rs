//! Lifecycle and container actions: the confirmation dialog, outcomes and notices.

use super::*;

impl App {
    /// Opens the confirmation to enroll the selected registered lead.
    ///
    /// The lead must be joined to its pane, have a fresh session identity from its
    /// registration, and not already be a lifecycle subject. The daemon verifies
    /// the live process again before it records anything.
    pub fn show_presence_status(&mut self, _state: &ObservationState) {
        let Some(row) = self.selected_row() else {
            self.set_focus_message(Some("nothing selected: no action target".to_string()));
            return;
        };
        let RowKind::Agent(agent) = &row.node.row.kind else {
            self.set_focus_message(Some("only a Pi session can be managed".to_string()));
            return;
        };
        let standing = agent
            .registry
            .as_ref()
            .and_then(|view| view.agent.claim_standing.as_ref());
        let message = match standing {
            Some(crate::control_plane::registry::presence::Standing::Granted) => {
                "this Pi session already has a Radar subject"
            }
            Some(crate::control_plane::registry::presence::Standing::Duplicate) => {
                "this session claim was rejected; reload or resume from its last live state"
            }
            Some(crate::control_plane::registry::presence::Standing::Reserved) => {
                "Radar reserved this session attachment; waiting for the launched process"
            }
            Some(crate::control_plane::registry::presence::Standing::Pending) => {
                "Radar is still verifying this session claim"
            }
            Some(crate::control_plane::registry::presence::Standing::Released) => {
                "this session claim was released; reload or resume to claim it again"
            }
            None => "Radar adopts Pi sessions through presence; waiting for the next report",
        };
        self.set_focus_message(Some(message.to_string()));
    }

    /// The container the selected row shows, when Radar records one for it.
    ///
    /// A row without one is a place this daemon records nothing for: there is no
    /// container to address, and inventing one from the handle would name a place
    /// Radar does not own.
    pub fn selected_container(&self) -> Option<&ContainerRef> {
        let row = self.selected_row()?;
        match &row.node.row.kind {
            RowKind::Workspace { container, .. } => container.as_ref(),
            RowKind::Agent(agent) => agent.container.as_ref(),
            RowKind::Pane(pane) => pane.container.as_ref(),
            RowKind::Task(_) | RowKind::Lifecycle(_) => None,
        }
    }

    /// Whether the outcomes panel is open, and where it is scrolled to.
    pub fn notices_open(&self) -> bool {
        self.notices_open
    }

    pub fn notice_scroll(&self) -> usize {
        self.notice_scroll
    }

    /// Opens or closes the outcomes panel. Opening starts at the top, where the
    /// newest outcome is.
    pub fn toggle_notices(&mut self) {
        self.notices_open = !self.notices_open;
        if self.notices_open {
            self.notice_scroll = 0;
        }
    }

    /// Moves the outcomes panel by whole lines. The panel clamps the offset to the
    /// text it has, so this may be called past either end.
    pub fn scroll_notices(&mut self, lines: isize) {
        self.notice_scroll = self.notice_scroll.saturating_add_signed(lines);
    }

    /// The keys a shared question answers: Escape cancels, Tab and the arrows
    /// move between the buttons, Enter activates the selected one.
    pub(super) fn handle_session_confirm_key(&mut self, key: KeyEvent) -> Option<Action> {
        // An answered question is inert until the session reports it settled.
        if self.session_confirm.as_ref()?.answered {
            return None;
        }
        let answer = self.session_confirm.as_mut()?.handle_key(key)?;
        self.answer_session_confirm(answer);
        None
    }

    /// A left click while a shared question is up. Only the two drawn buttons
    /// answer, so a click behind it can never confirm.
    pub(super) fn session_confirm_click(&mut self, at: (u16, u16)) -> Option<Action> {
        if inside(self.layout.confirm_confirm, at) {
            self.answer_session_confirm(crate::ui_session::confirm::Answer::Confirm);
        } else if inside(self.layout.confirm_cancel, at) {
            self.answer_session_confirm(crate::ui_session::confirm::Answer::Cancel);
        }
        None
    }
}
