//! Terminal presentation: the fleet tree, the selected-row detail panel and
//! the source status line.
//!
//! This module is the display boundary. Every string it draws comes from the
//! runtime and is therefore terminal-untrusted: [`sanitize`] strips control
//! characters (including ESC) before they reach a [`ratatui::text::Span`], so a
//! label can never move the cursor or change terminal modes. Nothing here
//! inspects Herdr tokens — rows and details are rendered from
//! [`crate::tree`] facts.
//!
//! [`render`] is stateless apart from the caller-owned [`ListState`], which
//! carries the tree's scroll offset across frames; the caller must keep one
//! for the lifetime of the view.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, Geometry, VisibleRow};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bus::Task;
use crate::model::{AgentState, ForegroundEvidence, HerdsmanFacts, SessionIdentity, TerminalMode};
use crate::observation::{ObservationState, RetentionBasis, SourceFreshness};
use crate::theme;
use crate::tree::{AgentRow, RowKind};

/// Height reserved for the detail panel when the terminal has room for it.
///
/// The panel lists one field per line and long identities wrap, so this is the
/// smallest height that shows an agent's full detail at a typical width.
const DETAIL_HEIGHT: u16 = 13;

/// Widest content column. Wider terminals get symmetric side padding rather
/// than a tree stretched across a wall of empty cells.
/// The gutter between the terminal edge and the panels. A couple of columns
/// stops the borders sitting on the edge; anything more is width the panels
/// could be using.
const GUTTER: u16 = 2;

/// Below this width the details stack under the tree instead of sitting beside
/// it: two panels that narrow stop being readable.
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 80;

/// How wide the details column is when it sits beside the tree.
const DETAIL_MIN_WIDTH: u16 = 30;
const DETAIL_MAX_WIDTH: u16 = 60;

/// Renders the whole overview into `frame`.
///
/// `tick` is the animation frame a working mark is drawn on. The caller owns it,
/// and owns deciding whether the terminal needs redrawing at all, so this stays
/// stateless apart from the scroll state.
pub fn render(
    frame: &mut Frame,
    state: &ObservationState,
    app: &App,
    list_state: &mut ListState,
    tick: usize,
) -> Geometry {
    let area = content_column(frame.area());
    let hints = footer(app, area.width as usize);
    let (body, hint_area) = regions(area, hints.len() as u16);
    if hint_area.height > 0 {
        frame.render_widget(Paragraph::new(hints), hint_area);
    }

    let details = app.shows_details().then(|| detail_lines(state, app));
    let (tree_area, detail_area) = body_areas(body, details.as_deref(), app);

    let rows = app.visible_rows();
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::palette().border))
        .title(tree_title(state, app));
    let tree_content = block.inner(tree_area);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_message(state, app))
                .wrap(Wrap { trim: true })
                .block(block),
            tree_area,
        );
    } else {
        let items: Vec<ListItem> = rows
            .iter()
            .map(|row| row_item(row, tick, tree_area.width.saturating_sub(2) as usize))
            .collect();
        list_state.select(app.selected_index());
        let list = List::new(items).block(block).highlight_style(
            Style::new()
                .bg(theme::palette().selection)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, tree_area, list_state);
    }

    if let (Some(detail_area), Some(lines)) = (detail_area, &details) {
        frame.render_widget(
            Paragraph::new(lines.clone())
                .wrap(Wrap { trim: false })
                .scroll((app.details_scroll(), 0))
                .block(
                    Block::bordered()
                        .border_style(Style::new().fg(theme::palette().border))
                        .title("Details"),
                ),
            detail_area,
        );
    }

    Geometry {
        tree_panel: tree_area,
        tree_content,
        details: detail_area,
        details_lines: details.as_ref().map_or(0, Vec::len),
        offset: list_state.offset(),
    }
}

/// The tree and, when they are shown, the details.
///
/// Details sit beside the tree rather than under it: the tree is a list of short
/// rows and the details are a few long lines, so a column costs the tree less
/// than a stack of rows does. A terminal too narrow for two readable panels
/// stacks them instead, sized to the content so nothing is clipped.
fn body_areas(body: Rect, details: Option<&[Line<'_>]>, app: &App) -> (Rect, Option<Rect>) {
    let Some(detail) = details else {
        return (body, None);
    };
    debug_assert!(app.shows_details(), "details are only laid out when shown");

    if body.width >= SIDE_BY_SIDE_MIN_WIDTH {
        let width = (body.width / 3).clamp(DETAIL_MIN_WIDTH, DETAIL_MAX_WIDTH);
        let [tree, detail_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(width)]).areas(body);
        return (tree, Some(detail_area));
    }

    // Stacked: as many rows as the content needs, counting the rows a long
    // identity will actually wrap onto, and never more than half the body.
    let columns = body.width.saturating_sub(2).max(1) as u32;
    let rows: u16 = detail
        .iter()
        .map(|line| (line.width() as u32).div_ceil(columns).max(1) as u16)
        .sum();
    let cap = if body.height >= DETAIL_HEIGHT + 4 {
        DETAIL_HEIGHT + 2
    } else {
        body.height / 2
    };
    let height = rows.saturating_add(2).min(cap);
    let [tree, detail_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height)]).areas(body);
    (tree, Some(detail_area))
}

/// The content column: the terminal, less a small gutter, and no wider column
/// in the middle of it. A dashboard that centres itself leaves the margins to
/// grow with the terminal and spends width on nothing.
fn content_column(area: Rect) -> Rect {
    if area.width <= GUTTER * 2 + 20 {
        return area;
    }
    Rect {
        x: area.x + GUTTER,
        width: area.width - GUTTER * 2,
        ..area
    }
}

/// Status row, tree/detail body and hint footer.
///
/// Chrome is given up before content on a terminal too short to hold both: the
/// footer disappears below four rows and its spacer below six, so a one-row
/// terminal still draws the source state rather than an empty cell.
fn regions(area: Rect, wanted: u16) -> (Rect, Rect) {
    // Chrome is dropped before content: a short terminal keeps one hint line,
    // and a two-line set needs room for it.
    let hints_height = if area.height >= 8 {
        wanted.min(2)
    } else {
        u16::from(area.height >= 4)
    };
    let hints = Rect {
        y: area.bottom().saturating_sub(hints_height),
        height: hints_height,
        ..area
    };
    let body_bottom = if area.height >= 6 {
        hints.y.saturating_sub(1)
    } else {
        hints.y
    };
    let body = Rect {
        y: area.y,
        height: body_bottom.saturating_sub(area.y),
        ..area
    };
    (body, hints)
}

/// The footer line: the last focus message while one is up, otherwise the keys
/// this view answers to.
///
/// A message takes this line rather than the source diagnostic's place: the
/// freshness of the fleet is a property of everything drawn and stays in the
/// tree heading and the details.
fn footer(app: &App, width: usize) -> Vec<Line<'static>> {
    let palette = theme::palette();
    if let Some(message) = app.focus_message() {
        return vec![Line::from(Span::styled(
            sanitize(message),
            Style::new().fg(palette.failed),
        ))];
    }
    if app.is_filter_editing() {
        let pairs = vec![
            ("type", "filter".to_string()),
            ("Enter", "keep".to_string()),
            ("Esc", "clear".to_string()),
        ];
        return wrap_hints(&pairs, width);
    }

    // The keys in the order a reader needs them. Full wording first; a terminal
    // that cannot hold it in two lines gets the terse wording instead, so the
    // state words are the first thing to go rather than the last keys.
    let full = vec![
        ("j/k", String::new()),
        ("spc", "fold".to_string()),
        ("Enter", "focus".to_string()),
        ("/", "filter".to_string()),
        ("n/N", "next".to_string()),
        ("w/W", "work".to_string()),
        ("s", app.order().label().to_string()),
        ("p", app.pane_view().label().to_string()),
        ("d", toggle_label(app.shows_details(), "details")),
        ("e", toggle_label(app.shows_finished(), "finished")),
        ("q", "quit".to_string()),
    ];
    let terse = vec![
        ("j/k", String::new()),
        ("spc", "fold".to_string()),
        ("Enter", "focus".to_string()),
        ("/", "filter".to_string()),
        ("n", "next".to_string()),
        ("w", "work".to_string()),
        ("s", app.order().label().to_string()),
        ("p", app.pane_view().label().to_string()),
        ("d", "details".to_string()),
        ("e", "finished".to_string()),
        ("q", "quit".to_string()),
    ];
    match wrap_hints(&full, width) {
        lines if lines.len() <= 2 => lines,
        _ => wrap_hints(&terse, width),
    }
}

/// What a toggle's hint says: what pressing it does while it is on, and what
/// would appear while it is off.
fn toggle_label(shown: bool, noun: &str) -> String {
    if shown {
        format!("hide {noun}")
    } else {
        noun.to_string()
    }
}

/// Lays the hints out over as many lines as `width` allows, keeping their
/// order. A line that runs out of room starts the next one.
fn wrap_hints(pairs: &[(&str, String)], width: usize) -> Vec<Line<'static>> {
    let palette = theme::palette();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (key, action) in pairs {
        let entry = key.chars().count() + usize::from(!action.is_empty()) + action.chars().count();
        let separator = if spans.is_empty() { 0 } else { 3 };
        if !spans.is_empty() && used + separator + entry > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        if used > 0 {
            spans.push(Span::styled(" · ", Style::new().fg(palette.muted)));
            used += 3;
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(palette.subtle).add_modifier(Modifier::BOLD),
        ));
        used += key.chars().count();
        if !action.is_empty() {
            spans.push(Span::raw(format!(" {action}")));
            used += 1 + action.chars().count();
        }
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// The fleet heading: what the panel is, and how current its facts are.
///
/// Source freshness belongs here rather than on a line of its own: it is a
/// property of everything the panel shows. The failure itself is written out in
/// the details, where there is room for it.
fn tree_title(state: &ObservationState, app: &App) -> Line<'static> {
    let palette = theme::palette();
    let (word, style) = match state.source_freshness() {
        SourceFreshness::Pending => ("pending", Style::new().fg(palette.subtle)),
        SourceFreshness::Current => ("current", Style::new().fg(palette.muted)),
        // Loud when it matters: the words that say the facts are not current
        // are the last thing that should recede.
        SourceFreshness::Stale { .. } => (
            "STALE",
            Style::new()
                .fg(palette.retained)
                .add_modifier(Modifier::BOLD),
        ),
        SourceFreshness::Unavailable { .. } => (
            "UNAVAILABLE",
            Style::new().fg(palette.failed).add_modifier(Modifier::BOLD),
        ),
    };
    let mut spans = vec![
        Span::raw("Fleet "),
        Span::styled(format!("· {word}"), style),
    ];
    if !app.filter_query().is_empty() {
        spans.push(Span::styled(
            format!(" — filter: {}", sanitize(app.filter_query())),
            Style::new().fg(palette.muted),
        ));
    }
    if app.bus_diagnostic().is_some() {
        // Attention rather than failure: the fleet is observed as usual and the
        // bus alone is off. What went wrong is written in the details.
        spans.push(Span::styled(
            " · bus off",
            Style::new().fg(palette.retained),
        ));
    }
    Line::from(spans)
}

fn empty_message(state: &ObservationState, app: &App) -> String {
    if app.visible_rows().is_empty()
        && app.filter_query().is_empty()
        && state
            .inventory()
            .is_some_and(|inventory| inventory.workspaces.is_empty() && inventory.panes.is_empty())
    {
        return "no workspaces reported by the runtime".to_string();
    }
    match state.source_freshness() {
        SourceFreshness::Pending => "waiting for the first collection…".to_string(),
        SourceFreshness::Unavailable { diagnostic } => {
            format!("source unavailable: {}", sanitize(diagnostic))
        }
        _ if !app.filter_query().is_empty() => "no rows match the filter".to_string(),
        _ => "no visible rows".to_string(),
    }
}

fn row_item(row: &VisibleRow<'_>, tick: usize, width: usize) -> ListItem<'static> {
    let palette = theme::palette();
    let mut spans: Vec<Span<'static>> = Vec::new();
    // The span that gives way when the row is wider than the panel: the name,
    // so the state, age and model on its right stay readable.
    let mut flex: Option<usize> = None;
    if row.depth > 0 {
        spans.push(Span::raw("  ".repeat(row.depth)));
    }
    spans.push(Span::styled(
        fold_marker(row),
        Style::new().fg(palette.subtle),
    ));
    match &row.node.row.kind {
        RowKind::Workspace { number, .. } => {
            // A heading, not body text: weight and the heading's own ink are
            // what a terminal can give it — a cell has no font size.
            spans.push(Span::styled(
                sanitize(row.node.row.title()),
                Style::new()
                    .fg(palette.heading)
                    .add_modifier(Modifier::BOLD),
            ));
            if let Some(number) = number {
                spans.push(Span::styled(
                    format!(" · {number}"),
                    Style::new().fg(palette.subtle),
                ));
            }
        }
        RowKind::Agent(agent) => {
            let retained = agent.retained.is_some();
            let name = agent.name.as_deref();
            let (mark, mark_colour) = theme::agent_state(&agent.state, name, retained, tick);
            spans.push(Span::styled(
                format!("{mark} "),
                Style::new().fg(mark_colour),
            ));
            if let Some(logo) = theme::logo(name) {
                spans.push(Span::styled(
                    format!("{logo} "),
                    Style::new().fg(theme::agent_ink(name, retained)),
                ));
            }
            // The row's own text carries the state too, with weight spent on
            // the lifecycle that is moving: colour is what a glance reads, and
            // the mark alone is one cell of it.
            let title_style = theme::agent_text(&agent.state, name, retained);
            flex = Some(spans.len());
            spans.push(Span::styled(sanitize(&agent.title), title_style));
            // Routine states are carried by their marks; exceptional states
            // keep a word. Missing facts leave no dangling separators. A
            // background badge counts what is unresolved, not what is running:
            // a task that has exited is still work its pane is waiting on.
            let unresolved = agent.facts.background_task_ids().len();
            let tail = [
                (!matches!(
                    agent.state,
                    crate::model::AgentState::Working
                        | crate::model::AgentState::Waiting
                        | crate::model::AgentState::Idle
                ))
                .then(|| sanitize(agent.state.word())),
                agent.facts.assigned_for.map(duration),
                agent.facts.model_and_thinking().map(|m| sanitize(&m)),
                (unresolved > 0).then(|| {
                    let moving =
                        agent.retained.is_none() && agent.facts.background_running.unwrap_or(0) > 0;
                    match theme::command_frames().filter(|_| moving) {
                        Some(frames) => format!("{} {unresolved} bg", theme::frame(frames, tick)),
                        None => format!("{unresolved} bg"),
                    }
                }),
            ];
            for part in tail.into_iter().flatten() {
                spans.push(Span::styled(
                    format!(" · {part}"),
                    Style::new().fg(palette.subtle),
                ));
            }
            if let Some(basis) = &agent.retained {
                spans.push(Span::styled(
                    format!(" · retained ({})", basis_label(basis)),
                    Style::new().fg(palette.retained),
                ));
            }
        }
        RowKind::Pane(pane) => {
            let title = sanitize(&pane.title);
            let id = sanitize(&pane.pane_id);
            // Two columns, as an agent row has: what the pane is doing now, and
            // what it is. The first moves while a command runs, so a busy pane
            // is visibly busy rather than only labelled; the second is the
            // program's own mark where Radar knows it, the terminal mode where
            // it does not, and nothing for a pane with nothing in it.
            let process = pane.command();
            let lead = match (process.is_some(), theme::command_frames()) {
                (true, Some(frames)) => theme::frame(frames, tick).to_string(),
                _ => theme::pane_mark().to_string(),
            };
            let identity = process.as_ref().map(|command| {
                theme::process_mark(program_of(command))
                    .unwrap_or_else(|| match pane.terminal() {
                        TerminalMode::FullScreen => "▣",
                        TerminalMode::Line => "❯",
                        TerminalMode::Unknown => "·",
                    })
                    .to_string()
            });
            if let Some(process) = process {
                // A command in the foreground is the truth about the pane
                // whatever the label says: the pane is in use again, and the
                // details are where the finished session stays visible.
                let ink = palette.muted;
                spans.push(Span::styled(format!("{lead} "), Style::new().fg(ink)));
                if let Some(identity) = &identity {
                    spans.push(Span::styled(
                        format!("{identity} "),
                        Style::new().fg(palette.subtle),
                    ));
                }
                flex = Some(spans.len());
                spans.push(Span::styled(sanitize(&process), Style::new().fg(ink)));
                if let Some(running) = pane.running_for() {
                    spans.push(Span::styled(
                        format!(" · {}", duration(running)),
                        Style::new().fg(palette.subtle),
                    ));
                }
            } else if let Some(session) = &pane.exited {
                // A finished session wearing the pane's label. It keeps its own
                // name and vendor — the only thing linking the pane to the work
                // that was done in it — and recedes, because nothing here is
                // current.
                let ink = palette.subtle;
                spans.push(Span::styled(format!("{EXITED} "), Style::new().fg(ink)));
                let logo = session
                    .mark
                    .map(|mark| format!("{mark} "))
                    .unwrap_or_else(|| "  ".into());
                spans.push(Span::styled(logo, Style::new().fg(ink)));
                flex = Some(spans.len());
                spans.push(Span::styled(sanitize(&session.title), Style::new().fg(ink)));
                spans.push(Span::styled(" · exited", Style::new().fg(ink)));
            } else {
                let ink = palette.subtle;
                spans.push(Span::styled(format!("{lead} "), Style::new().fg(ink)));
                if title == id {
                    // An untitled pane reports its own id as the title; nothing
                    // else is left to name it by.
                    spans.push(Span::styled(format!("pane {id}"), Style::new().fg(ink)));
                } else {
                    flex = Some(spans.len());
                    spans.push(Span::styled(title, Style::new().fg(ink)));
                }
            }
        }
    }
    ListItem::new(Line::from(fit(spans, flex, width)))
}

/// The program a foreground command runs: the first word of the line, without
/// its directory.
fn program_of(command: &str) -> &str {
    let word = command.split_whitespace().next().unwrap_or(command);
    word.rsplit('/').next().unwrap_or(word)
}

/// Narrows `spans` to `width` cells, spending the `flex` span first.
///
/// A row wider than its panel would otherwise be cut wherever the edge falls,
/// which is usually the state and the model, the parts a glance is for. The
/// flexible span loses its tail to a `…`; if the rest alone is still too wide
/// the row is cut at the edge, also with a `…`.
fn fit(mut spans: Vec<Span<'static>>, flex: Option<usize>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= width {
        return spans;
    }
    let mut excess = total - width;
    if let Some(index) = flex {
        let own = spans[index].width();
        // Keep at least one cell of the name besides the ellipsis.
        let take = excess.min(own.saturating_sub(2));
        if take > 0 {
            let target = own - take - 1;
            spans[index].content = clip(&spans[index].content, target, true).into();
            excess -= take;
        }
    }
    if excess == 0 {
        return spans;
    }
    // Still too wide: cut the row at the edge.
    let mut remaining = width.saturating_sub(1);
    let mut out = Vec::new();
    for span in spans {
        if remaining == 0 {
            break;
        }
        let w = span.width();
        if w <= remaining {
            remaining -= w;
            out.push(span);
        } else {
            let content = clip(&span.content, remaining, false);
            out.push(Span::styled(content, span.style));
            remaining = 0;
        }
    }
    out.push(Span::raw("…"));
    out
}

/// The longest prefix of `text` that fits `cells`, plus `…` when asked.
fn clip(text: &str, cells: usize, ellipsis: bool) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = Span::raw(ch.to_string()).width();
        if used + w > cells {
            break;
        }
        out.push(ch);
        used += w;
    }
    if ellipsis {
        out.push('…');
    }
    out
}

/// The fold affordance. A leaf keeps the same two columns as a branch so the
/// state marks below it stay in one column — a dot in every leaf row is a mark
/// the eye has to learn and then ignore.
fn fold_marker(row: &VisibleRow<'_>) -> &'static str {
    match (row.has_children, row.collapsed) {
        (true, true) => "▸ ",
        (true, false) => "▾ ",
        (false, _) => "  ",
    }
}

/// The mark for a session that has gone: a state no source reports, drawn
/// where a state mark goes.
const EXITED: &str = "⊘";

/// How long something has been running, in as few characters as say it — the
/// difference between a build that just started and one worth going to look at.
///
/// Seconds are kept below the hour, as Herdsman's own fleet view shows an
/// assignment's age: a build two minutes in is not two minutes old to anyone
/// deciding whether to go and look at it.
fn duration(running: Duration) -> String {
    let seconds = running.as_secs();
    let minutes = seconds / 60;
    if seconds < 60 {
        format!("{seconds}s")
    } else if minutes < 60 {
        format!("{minutes}m{:02}s", seconds % 60)
    } else if minutes < 60 * 24 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{}d{}h", minutes / (60 * 24), minutes % (60 * 24) / 60)
    }
}

/// What the foreground program has done to the terminal, in words.
fn terminal_line(mode: TerminalMode) -> String {
    match mode {
        TerminalMode::FullScreen => "full screen — the program is drawing the pane".into(),
        TerminalMode::Line => "line mode — the shell still owns the terminal".into(),
        TerminalMode::Unknown => unavailable(),
    }
}

fn basis_label(basis: &RetentionBasis) -> &'static str {
    match basis {
        RetentionBasis::ShellForeground => "shell foreground",
        RetentionBasis::Unverified => "unverified",
    }
}

/// Detail-panel content for the selected row.
fn detail_lines(state: &ObservationState, app: &App) -> Vec<Line<'static>> {
    let Some(row) = app.selected_row() else {
        return vec![Line::from("no row selected")];
    };
    let mut lines = vec![Line::from(Span::styled(
        sanitize(row.node.row.title()),
        Style::new().add_modifier(Modifier::BOLD),
    ))];

    match &row.node.row.kind {
        RowKind::Workspace {
            workspace_id,
            label,
            number,
        } => {
            lines.push(field("kind", "workspace".to_string()));
            lines.push(field(
                "workspace",
                location_text(label.as_deref(), workspace_id),
            ));
            lines.push(field(
                "number",
                number.map(|n| n.to_string()).unwrap_or_else(unavailable),
            ));
        }
        RowKind::Pane(pane) => {
            lines.push(field(
                "kind",
                match pane.exited {
                    Some(_) => "pane (finished session)".to_string(),
                    None => "pane".to_string(),
                },
            ));
            lines.push(field(
                "state",
                match &pane.exited {
                    Some(_) => "exited — no agent reported on this pane".to_string(),
                    None => unavailable(),
                },
            ));
            lines.push(field(
                "location",
                location_line(
                    (pane.workspace_label.as_deref(), &pane.workspace_id),
                    (pane.tab_label.as_deref(), &pane.tab_id),
                    &pane.pane_id,
                ),
            ));
            if let Some(session) = &pane.exited {
                lines.push(field("session", sanitize(&session.title)));
                // The row is the session now, so this is what the pane is
                // instead of it.
                lines.push(field("pane now", sanitize(&pane.title)));
            }
            lines.push(field(
                "foreground",
                match &pane.foreground {
                    Some(ForegroundEvidence::NonShell { .. }) => {
                        pane.command().unwrap_or_else(unavailable)
                    }
                    Some(ForegroundEvidence::Shell) => "shell — nothing in the foreground".into(),
                    Some(ForegroundEvidence::Inconclusive) => {
                        "unknown — PID fields disagree".into()
                    }
                    None => unavailable(),
                },
            ));
            lines.push(field("terminal", terminal_line(pane.terminal())));
            lines.push(field(
                "running for",
                pane.running_for().map(duration).unwrap_or_else(unavailable),
            ));
            lines.push(field("agent", unavailable()));
        }
        RowKind::Agent(agent) => {
            lines.push(field(
                "kind",
                match &agent.retained {
                    Some(basis) => format!("agent (retained — {})", basis_label(basis)),
                    None => "agent (current)".to_string(),
                },
            ));
            lines.push(field(
                "location",
                location_line(
                    (agent.workspace_label.as_deref(), &agent.workspace_id),
                    (agent.tab_label.as_deref(), &agent.tab_id),
                    &agent.pane_id,
                ),
            ));
            lines.push(field(
                "agent",
                agent
                    .name
                    .as_deref()
                    .map(sanitize)
                    .unwrap_or_else(unavailable),
            ));
            let status = agent
                .status
                .as_ref()
                .map(|status| sanitize(&status.to_string()))
                .unwrap_or_else(unavailable);
            // The row draws the state derived from the pane; the panel says so
            // and names the owner's projection beside it when there is one.
            lines.push(field(
                "state",
                format!("{} (derived from the pane)", sanitize(agent.state.word())),
            ));
            if let Some(projected) = &agent.facts.state {
                lines.push(field(
                    "assignment",
                    format!(
                        "{} (owner projection)",
                        sanitize(AgentState::from(projected).word())
                    ),
                ));
            }
            lines.push(match &agent.retained {
                Some(_) => field("status (last observed)", status),
                None => field("status", status),
            });
            lines.extend(herdsman_lines(&agent.facts));
            lines.extend(bus_lines(app, agent));
            // Freshness and identity last: a long reported session path wraps,
            // and it must not push the required fields out of a small panel.
            lines.push(detail_freshness(state, &row));
            lines.push(field(
                "identity",
                session_text(agent.session.as_ref()).unwrap_or_else(unavailable),
            ));
            if let Some(line) = bus_diagnostic(app) {
                lines.push(line);
            }
            return lines;
        }
    }

    lines.push(detail_freshness(state, &row));
    if let Some(line) = bus_diagnostic(app) {
        lines.push(line);
    }
    lines
}

/// Freshness for the selected row: a retained association is not current, and
/// a stale/unavailable source is stated on every row.
fn detail_freshness(state: &ObservationState, row: &VisibleRow<'_>) -> Line<'static> {
    let retained = match &row.node.row.kind {
        RowKind::Agent(agent) => agent.retained.as_ref(),
        _ => None,
    };
    let source = match state.source_freshness() {
        SourceFreshness::Pending => "source: waiting for the first collection".to_string(),
        SourceFreshness::Current => "source: current".to_string(),
        SourceFreshness::Stale { diagnostic } => {
            format!("source: stale — {}", sanitize(diagnostic))
        }
        SourceFreshness::Unavailable { diagnostic } => {
            format!("source: unavailable — {}", sanitize(diagnostic))
        }
    };
    let text = match retained {
        // The basis is stated once, in `kind`: this line only has to say that
        // the facts are no longer current.
        Some(_) => format!("observation: retained, not currently observed; {source}"),
        None => format!("observation: current observation; {source}"),
    };
    let style =
        if retained.is_some() || !matches!(state.source_freshness(), SourceFreshness::Current) {
            Style::new().fg(theme::palette().retained)
        } else {
            Style::new().fg(theme::palette().done)
        };
    Line::from(Span::styled(text, style))
}

fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::new().fg(theme::palette().subtle),
        ),
        Span::raw(value),
    ])
}

/// The Herdsman facts the details carry, each drawn only while the source
/// publishes it: a fact nobody reported is absent here, never a placeholder.
///
/// Every value is text another process wrote — an assignment's display text
/// above all — so each is sanitized like any other runtime string.
fn herdsman_lines(facts: &HerdsmanFacts) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(role) = facts.role.as_deref() {
        lines.push(field("role", sanitize(role)));
    }
    // Herdr's own display agent is where a worker's agent definition comes
    // from; it earns a line only when it says more than the role already does.
    if let Some(definition) = facts.definition.as_deref()
        && Some(definition) != facts.role.as_deref()
    {
        lines.push(field("definition", sanitize(definition)));
    }
    if let Some(assignment) = facts.assignment.as_deref() {
        lines.push(field("assignment", sanitize(assignment)));
    }
    if let Some(assigned) = facts.assigned_for {
        lines.push(field("assigned for", duration(assigned)));
    }
    if let Some(awaiting) = facts.awaiting() {
        lines.push(field("awaiting", sanitize(&awaiting)));
    }
    if !facts.background_tasks.is_empty() {
        lines.push(field(
            "background",
            sanitize(&facts.background_tasks.join(", ")),
        ));
    }
    // The publisher's own count speaks only for its running tasks; it is kept
    // beside the list so `0 running` with work outstanding reads as what it is.
    if let Some(running) = facts.background_running {
        lines.push(field("background running", running.to_string()));
    }
    if let Some(started) = facts.background_started.as_deref() {
        lines.push(field("background started", sanitize(started)));
    }
    if let Some(model) = facts.model.as_deref() {
        lines.push(field("model", sanitize(model)));
    }
    if let Some(provider) = facts.provider.as_deref() {
        lines.push(field("provider", sanitize(provider)));
    }
    if let Some(thinking) = facts.thinking.as_deref() {
        lines.push(field("thinking", sanitize(thinking)));
    }
    if let Some(usage) = facts.context_usage.as_deref() {
        lines.push(field("context usage", sanitize(usage)));
    }
    if let Some(name) = facts.session_name.as_deref() {
        lines.push(field("session name", sanitize(name)));
    }
    if let Some(run) = facts.run.as_deref() {
        lines.push(field("run", sanitize(run)));
    }
    if let Some(request) = facts.request.as_deref() {
        lines.push(field("request", sanitize(request)));
    }
    if let Some(ask) = facts.ask.as_deref() {
        lines.push(field("ask", sanitize(ask)));
    }
    lines
}

fn unavailable() -> String {
    "unavailable".to_string()
}

/// The bus tasks attached to this agent row, and the note a disagreement with
/// the pane tokens earns.
///
/// The join is the exact `pi_herdsman_session` UUID the source published for
/// this row. A publisher's `hello` pane is a fallback for a row that publishes
/// no UUID at all — it is never a second chance for a row whose own UUID names
/// a session no publisher is connected for, and nothing joins on a cwd, a
/// title or a human session name.
fn bus_lines(app: &App, agent: &AgentRow) -> Vec<Line<'static>> {
    let held = match agent.session_uuid.as_deref() {
        Some(uuid) => app.bus_session(uuid),
        None => app.bus_session_on_pane(&agent.pane_id),
    };
    let Some(held) = held else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    if held.tasks.is_empty() {
        // A published empty list and no publisher are different facts, and the
        // panel says which of the two it is.
        lines.push(field("bus", "connected — no unresolved tasks".into()));
    } else {
        let now = now_unix_ms();
        for task in &held.tasks {
            lines.push(bus_task_line(task, now));
            let id = sanitize(&task.id);
            if let Some(command) = task.command.as_deref() {
                lines.push(field(&format!("command ({id})"), bound_text(command)));
            }
            if let Some(cwd) = task.cwd.as_deref() {
                lines.push(field(&format!("cwd ({id})"), bound_text(cwd)));
            }
        }
    }
    // The list outranks the count, so the count is never drawn in its place;
    // where the two disagree the panel states it instead of resolving it. The
    // token counts running tasks, so only running tasks are comparable with it.
    if let Some(reported) = agent.facts.background_running
        && reported as usize != held.running()
    {
        lines.push(field(
            "tokens",
            format!(
                "report {reported} running; the bus lists {}",
                held.running()
            ),
        ));
    }
    lines
}

/// One task as the details draw it: its id, its state word as published, how
/// long it has run, how long ago it last produced output, how much it has
/// produced, and its exit code once there is one. A field the publisher did not
/// send draws nothing — absent is not a zero.
fn bus_task_line(task: &Task, now_unix_ms: u64) -> Line<'static> {
    let mut parts = vec![sanitize(&task.id), sanitize(&task.state.to_string())];
    if let Some(started) = task.started_at {
        parts.push(duration(elapsed(started, now_unix_ms)));
    }
    if let Some(last_output) = task.last_output_at {
        parts.push(format!(
            "output {} ago",
            duration(elapsed(last_output, now_unix_ms))
        ));
    }
    if let Some(bytes) = task.output_bytes {
        parts.push(format!("{bytes} B"));
    }
    if let Some(code) = task.exit_code {
        parts.push(format!("exit {code}"));
    }
    field("task", parts.join(" · "))
}

/// How long ago a published Unix-millisecond timestamp was.
fn elapsed(then_unix_ms: u64, now_unix_ms: u64) -> Duration {
    Duration::from_millis(now_unix_ms.saturating_sub(then_unix_ms))
}

/// Unix milliseconds now, for the age of a published timestamp. Read where the
/// panel is drawn: an age is true of the frame that shows it, as every other
/// fact on screen is.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// Most of a published `command` or `cwd` the details draw. The contract bounds
/// both to 256 characters at the publisher; Radar bounds them again here, where
/// an arbitrary string reaches the screen.
const BUS_TEXT_LIMIT: usize = 256;

/// External text as the details draw it: control characters stripped, and the
/// value bounded so that one published field cannot fill the panel.
fn bound_text(text: &str) -> String {
    let cleaned = sanitize(text);
    if cleaned.chars().count() <= BUS_TEXT_LIMIT {
        return cleaned;
    }
    let kept: String = cleaned.chars().take(BUS_TEXT_LIMIT).collect();
    format!("{kept}…")
}

/// The bus's own diagnostic, when the listener is not running. The bus is a
/// subsystem beside the source rather than part of it, so its failure is
/// written on a line of its own.
fn bus_diagnostic(app: &App) -> Option<Line<'static>> {
    let diagnostic = app.bus_diagnostic()?;
    Some(field(
        "bus",
        format!("unavailable — {}", sanitize(diagnostic)),
    ))
}

fn location_text(label: Option<&str>, id: &str) -> String {
    match label {
        Some(label) => format!("{} ({})", sanitize(label), sanitize(id)),
        None => sanitize(id),
    }
}

/// One line of runtime location: workspace, tab and pane, innermost last.
fn location_line(
    workspace: (Option<&str>, &str),
    tab: (Option<&str>, &str),
    pane_id: &str,
) -> String {
    format!(
        "{} · {} · {}",
        location_text(workspace.0, workspace.1),
        location_text(tab.0, tab.1),
        sanitize(pane_id)
    )
}

/// Reported identity text: an explicit UUID, or the source-qualified reported
/// reference, exactly as reported.
fn session_text(session: Option<&SessionIdentity>) -> Option<String> {
    match session? {
        SessionIdentity::Uuid(uuid) => Some(format!("uuid {}", sanitize(uuid.as_str()))),
        SessionIdentity::Reported { source, value } => Some(match source {
            Some(source) => format!("{} {}", sanitize(source), sanitize(value)),
            None => sanitize(value),
        }),
    }
}

/// Strips terminal-control characters from runtime-provided text.
///
/// Whitespace-like controls become a space so words stay separated; every
/// other control character (notably ESC, so ANSI sequences are never emitted)
/// is removed.
fn sanitize(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' | '\n' | '\r' => cleaned.push(' '),
            _ if ch.is_control() => {}
            _ => cleaned.push(ch),
        }
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use crate::app::Action;
    use crate::focus::Target;
    use crate::herdr::decode_snapshot;
    use crate::model::{
        AgentObservation, FleetObservation, ForegroundEvidence, HerdsmanFacts, Location, Pane,
        RuntimeStatus, SemanticState, Tab, Workspace,
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Style};

    const REAL_SHAPED: &str = include_str!("../tests/fixtures/snapshot_real_shaped.json");

    fn fixture_state() -> ObservationState {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        state
    }

    fn render_with(
        state: &ObservationState,
        app: &App,
        width: u16,
        height: u16,
        list: &mut ListState,
        tick: usize,
    ) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("test backend is infallible");
        terminal
            .draw(|frame| {
                render(frame, state, app, list, tick);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| row_text(buffer, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_text(state: &ObservationState, app: &App, width: u16, height: u16) -> String {
        render_with(state, app, width, height, &mut ListState::default(), 0)
    }

    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn styles(buffer: &Buffer) -> Vec<Style> {
        buffer.content().iter().map(|cell| cell.style()).collect()
    }

    fn app_for(state: &ObservationState) -> App {
        let mut app = App::new();
        app.refresh(state);
        app
    }

    /// A one-workspace fleet whose only agent publishes `facts`, so a row and a
    /// details panel can be read against exactly what Herdsman reported.
    fn state_with_facts(facts: HerdsmanFacts, status: RuntimeStatus) -> ObservationState {
        let location = Location {
            workspace_id: "wH".into(),
            tab_id: "wH:t1".into(),
            pane_id: "wH:p1".into(),
        };
        let mut state = ObservationState::new();
        state.apply_success(FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wH".into(),
                label: Some("herdsman".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wH:t1".into(),
                workspace_id: "wH".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: location.clone(),
                label: None,
                title: None,
            }],
            agents: vec![AgentObservation {
                location,
                name: Some("pi".into()),
                label: Some("worker task".into()),
                status: Some(status),
                session: None,
                lineage: None,
                facts,
            }],
        });
        state
    }

    /// A rendered row's own text, without the panel frame around it.
    fn row_cell(line: &str) -> &str {
        line.trim_matches(|ch: char| ch.is_whitespace() || ch == '│')
    }

    /// Cycles to the view that lists every pane, for tests about pane rows.
    fn show_all_panes(app: &mut App) {
        while app.pane_view() != crate::app::PaneView::All {
            app.cycle_panes();
        }
    }

    fn select_row(app: &mut App, id: crate::tree::RowId) {
        let rows = app.visible_rows();
        let index = rows
            .iter()
            .position(|row| row.id == &id)
            .expect("row is visible");
        app.move_selection(index as i32 - app.selected_index().unwrap_or(0) as i32);
        assert_eq!(app.selected_index(), Some(index));
    }

    fn select_agent(app: &mut App, pane_id: &str) {
        select_row(app, crate::tree::RowId::Agent(pane_id.into()));
    }

    #[test]
    fn tree_and_detail_show_normalized_facts_without_tokens() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let screen = render_text(&state, &app, 180, 30);

        // Workspace grouping, nested worker under its owner, lifecycle, and no
        // ordinary panes (hidden by default).
        assert!(screen.contains("main"), "{screen}");
        assert!(
            screen.contains("worker task") && !screen.contains("worker task · working"),
            "{screen}"
        );
        assert!(
            !screen.contains("wA:p3"),
            "panes hidden by default: {screen}"
        );
        assert!(screen.contains("source: current"), "{screen}");

        // Detail panel: location, reported identity, lifecycle, freshness, and
        // unavailable metadata rather than derived values.
        assert!(
            screen.contains("location: main (wA) · agent tab (wA:t2) · wA:p2"),
            "{screen}"
        );
        assert!(screen.contains("agent: pi"), "{screen}");
        // The reported identity is a source-qualified reference, shown as
        // reported (long values wrap).
        assert!(screen.contains("identity: herdr:pi"), "{screen}");
        assert!(screen.contains("--home-dev-projects-beta--"), "{screen}");
        assert!(screen.contains("status: working"), "{screen}");
        // The row draws a state and the panel says which opinion it is.
        assert!(
            screen.contains("state: working (derived from the pane)"),
            "{screen}"
        );
        // The one Herdsman fact this worker publishes is its model; a role, an
        // assignment or anything outstanding it never reported draws no line
        // at all rather than an empty one.
        assert!(screen.contains("model: sample-model-1"), "{screen}");
        for absent in ["role:", "assignment:", "awaiting:", "ask:"] {
            assert!(!screen.contains(absent), "`{absent}`: {screen}");
        }
        assert!(
            screen.contains("observation: current observation"),
            "{screen}"
        );
        // No source token names leak into presentation.
        assert!(!screen.contains("pi_herdsman"), "{screen}");

        // Enabling ordinary panes adds their rows exactly once, named by what
        // the pane is rather than by its id.
        show_all_panes(&mut app);
        let screen = render_text(&state, &app, 90, 30);
        assert!(screen.contains("build log tail"), "{screen}");
        assert_eq!(screen.matches("build log tail").count(), 1, "{screen}");
        assert!(
            !screen.contains("wA:p3"),
            "ids stay in the details: {screen}"
        );
    }

    #[test]
    fn collapsing_a_branch_hides_its_rows_on_screen() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p1");
        assert!(render_text(&state, &app, 90, 30).contains("worker task"));

        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        let screen = render_text(&state, &app, 90, 30);
        assert!(!screen.contains("worker task"), "{screen}");
        assert!(screen.contains("fleet owner task"), "{screen}");
    }

    #[test]
    fn retained_observation_renders_as_last_observed_and_never_duplicates_its_pane() {
        let mut state = fixture_state();
        let mut without_owner = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        without_owner
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p1");
        state.apply_success(without_owner);
        state.apply_evidence("wA:p1", ForegroundEvidence::Shell);

        let mut app = app_for(&state);
        show_all_panes(&mut app);
        select_agent(&mut app, "wA:p1");
        let screen = render_text(&state, &app, 180, 30);

        assert!(screen.contains("retained (shell foreground)"), "{screen}");
        assert!(
            screen.contains("agent (retained — shell foreground)"),
            "{screen}"
        );
        assert!(screen.contains("status (last observed): idle"), "{screen}");
        assert!(screen.contains("not currently observed"), "{screen}");
        // Exactly one row for the pane: the tree draws the name once, and the
        // details name it twice (its heading and the session's human name).
        assert_eq!(screen.matches("fleet owner task").count(), 3, "{screen}");
        assert!(!screen.contains("pane wA:p1"), "{screen}");
        // The retained marker is visually distinct, not just textual.
        let mut terminal =
            Terminal::new(TestBackend::new(180, 30)).expect("infallible test backend");
        terminal
            .draw(|frame| {
                render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        assert!(
            styles(terminal.backend().buffer())
                .iter()
                .any(|style| style.fg == Some(theme::palette().retained)),
            "retained rows/markers carry a distinct style"
        );
    }

    #[test]
    fn stale_source_is_marked_with_its_diagnostic_on_every_row() {
        let mut state = fixture_state();
        state.apply_failure("herdr exited with status 1");
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p1");
        let screen = render_text(&state, &app, 180, 30);

        // The heading says the facts are not current, and the details say why.
        assert!(screen.contains("Fleet · STALE"), "{screen}");
        // The details panel is where the failure is written out, wrapped to its
        // width, so the state and the tail of the diagnostic are asserted
        // separately.
        assert!(screen.contains("source: stale"), "{screen}");
        assert!(screen.contains("exited with status 1"), "{screen}");
        assert!(
            screen.contains("observation: current observation; source: stale"),
            "{screen}"
        );
        // The last-good inventory is still shown, marked stale.
        assert!(screen.contains("fleet owner task"), "{screen}");
    }

    #[test]
    fn initial_failure_reads_as_unavailable_not_an_empty_fleet() {
        let mut state = ObservationState::new();
        state.apply_failure("herdr not found");
        let app = App::new();
        let screen = render_text(&state, &app, 80, 30);

        assert!(screen.contains("Fleet · UNAVAILABLE"), "{screen}");
        assert!(screen.contains("UNAVAILABLE"), "{screen}");
        assert!(screen.contains("herdr not found"), "{screen}");
        assert!(!screen.contains("no workspaces reported"), "{screen}");
    }

    #[test]
    fn successful_empty_inventory_reads_as_empty_not_a_failure() {
        let mut state = ObservationState::new();
        state.apply_success(
            decode_snapshot(include_str!("../tests/fixtures/snapshot_empty.json"))
                .expect("empty fixture decodes"),
        );
        let app = App::new();
        let screen = render_text(&state, &app, 80, 30);

        assert!(screen.contains("Fleet · current"), "{screen}");
        assert!(
            screen.contains("no workspaces reported by the runtime"),
            "{screen}"
        );
        assert!(!screen.contains("STALE"), "{screen}");
        assert!(!screen.contains("UNAVAILABLE"), "{screen}");
    }

    #[test]
    fn pending_source_explains_itself() {
        let state = ObservationState::new();
        let app = App::new();
        let screen = render_text(&state, &app, 80, 30);
        assert!(
            screen.contains("waiting for the first collection"),
            "{screen}"
        );
        assert!(!screen.contains("UNAVAILABLE"), "{screen}");
    }

    #[test]
    fn control_sequences_in_runtime_text_never_reach_the_screen() {
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        let owner = observation
            .agents
            .iter_mut()
            .find(|agent| agent.location.pane_id == "wA:p1")
            .expect("owner");
        // A label that would set colour, move the cursor and set a title.
        owner.label = Some("evil\u{1b}[31mRED\u{7}\u{1b}]0;pwn\u{7} label".into());
        owner.facts.role = Some("\u{1b}[2Jrole".into());
        let mut state = ObservationState::new();
        state.apply_success(observation);
        state.apply_failure("boom\u{1b}[31m");
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p1");

        let mut terminal = Terminal::new(TestBackend::new(90, 30)).expect("infallible");
        terminal
            .draw(|frame| {
                render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let screen: String = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol().to_string())
            .collect();

        assert!(
            !screen.chars().any(char::is_control),
            "no control character may reach the backend"
        );
        assert!(
            !screen.contains('\u{1b}'),
            "no escape may reach the backend"
        );
        // The sequence text is inert but still legible.
        assert!(screen.contains("evil[31mRED]0;pwn"), "{screen}");
        assert!(screen.contains("role: [2Jrole"), "{screen}");
        assert!(screen.contains("boom[31m"), "{screen}");
    }

    #[test]
    fn long_trees_scroll_to_keep_the_selection_visible() {
        let agents: Vec<AgentObservation> = (0..60)
            .map(|i| AgentObservation {
                location: Location {
                    workspace_id: "wL".into(),
                    tab_id: "wL:t1".into(),
                    pane_id: format!("wL:p{i:02}"),
                },
                name: Some("pi".into()),
                label: Some(format!("agent number {i:02}")),
                status: Some(RuntimeStatus::Idle),
                session: None,
                lineage: None,
                facts: crate::model::HerdsmanFacts::default(),
            })
            .collect();
        let mut observation = FleetObservation {
            workspaces: vec![crate::model::Workspace {
                workspace_id: "wL".into(),
                label: Some("long".into()),
                number: None,
            }],
            tabs: vec![crate::model::Tab {
                tab_id: "wL:t1".into(),
                workspace_id: "wL".into(),
                label: None,
                number: None,
            }],
            panes: agents
                .iter()
                .map(|agent| Pane {
                    location: agent.location.clone(),
                    label: None,
                    title: None,
                })
                .collect(),
            agents,
        };
        observation.agents.truncate(60);
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = app_for(&state);

        // Walk to the last row through the real navigation path.
        let last = app.visible_rows().len() - 1;
        for _ in 0..last {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        assert_eq!(app.selected_index(), Some(last));

        let mut list = ListState::default();
        let screen = render_with(&state, &app, 60, 12, &mut list, 0);
        assert!(screen.contains("agent number 59"), "{screen}");
        assert!(
            !screen.contains("agent number 00"),
            "the view scrolled to the selection: {screen}"
        );
        // Scrolling is carried in the caller-owned state, so the next frame
        // does not jump back to the top.
        assert!(list.offset() > 0, "list offset advanced");
        let screen = render_with(&state, &app, 60, 12, &mut list, 0);
        assert!(screen.contains("agent number 59"), "{screen}");

        // Walking back up scrolls back to the top.
        for _ in 0..last {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('k'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        let screen = render_with(&state, &app, 60, 12, &mut list, 0);
        assert!(screen.contains("agent number 00"), "{screen}");
    }

    #[test]
    fn small_terminals_render_without_panicking() {
        let state = fixture_state();
        let mut app = app_for(&state);
        for (width, height) in [(1, 1), (5, 3), (20, 6), (20, 10), (40, 8), (80, 24)] {
            let screen = render_text(&state, &app, width, height);
            assert_eq!(
                screen.lines().count(),
                height as usize,
                "every row of a {width}x{height} terminal is drawn"
            );
        }

        // A narrow terminal still shows the selected row's identity in details
        // where there is room, and never panics while navigating.
        show_all_panes(&mut app);
        for _ in 0..10 {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        let screen = render_text(&state, &app, 30, 14);
        assert!(screen.contains("Details"), "{screen}");
    }

    #[test]
    fn agent_rows_carry_a_state_mark_and_the_vendor_logo() {
        let state = fixture_state();
        let app = app_for(&state);
        let screen = render_text(&state, &app, 90, 30);

        // The pi worker is working, so its mark is a frame of the configured
        // animation and its logo is whatever mark the installed font can draw.
        let spinner = theme::frames(theme::config().appearance.working)
            .expect("the configured animation exists")
            .iter()
            .any(|frame| screen.contains(&frame.to_string()));
        assert!(spinner, "working rows animate: {screen}");
        let logo = theme::logo(Some("pi")).expect("pi has a mark");
        assert!(screen.contains(&format!("{logo} worker task")), "{screen}");
        // An idle agent keeps a distinct shape: a parked mark, not a spinner.
        assert!(
            screen.contains(&format!("· {logo} fleet owner task")),
            "{screen}"
        );
    }

    #[test]
    fn provider_prefixes_are_stripped_from_projected_titles() {
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        // A live agent title, and a pane keeping a finished session's label.
        for agent in &mut observation.agents {
            if agent.location.pane_id == "wA:p2" {
                agent.label = Some("π - Inspect Bifrost - nix-homelab".into());
            }
        }
        for pane in &mut observation.panes {
            if pane.location.pane_id == "wA:p4" {
                pane.label = Some("π 01a0edc4".into());
            }
        }

        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = app_for(&state);
        show_all_panes(&mut app);
        let screen = render_text(&state, &app, 90, 30);

        // The workspace the group header already names is not repeated after
        // the title.
        assert!(screen.contains("Inspect Bifrost"), "{screen}");
        assert!(
            !screen.contains("Inspect Bifrost - nix-homelab"),
            "{screen}"
        );
        assert!(screen.contains("01a0edc4"), "{screen}");
        // The raw prefixes are gone; a bare mark beside a rendered logo would
        // be the same thing said twice.
        assert!(!screen.contains("π - Inspect"), "{screen}");
        assert!(!screen.contains("π 01a0edc4"), "{screen}");
    }

    fn spans_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn an_overlong_row_gives_up_its_name_before_its_state() {
        let row = || {
            vec![
                Span::raw("▾ "),
                Span::raw("a very long agent name indeed"),
                Span::raw(" · idle · 5m · gpt:high"),
            ]
        };
        let fitted = fit(row(), Some(1), 36);
        let text = spans_text(&fitted);
        assert_eq!(text.chars().count(), 36, "{text}");
        assert!(text.ends_with(" · idle · 5m · gpt:high"), "{text}");
        assert!(text.contains("…"), "{text}");
        // A row that fits is untouched.
        assert_eq!(spans_text(&fit(row(), Some(1), 80)), spans_text(&row()));
        // When even the rest does not fit, the edge cuts it, marked.
        let cut = spans_text(&fit(row(), Some(1), 12));
        assert_eq!(cut.chars().count(), 12, "{cut}");
        assert!(cut.ends_with('…'), "{cut}");
        // No width at all must not panic.
        let _ = fit(row(), Some(1), 0);
    }

    #[test]
    fn hint_footer_tracks_filter_entry_and_a_wide_terminal_keeps_its_gutter() {
        let state = fixture_state();
        let mut app = app_for(&state);
        let screen = render_text(&state, &app, 100, 24);
        assert!(screen.contains("j/k"), "{screen}");
        assert!(screen.contains("q quit"), "{screen}");

        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('/'),
            crossterm::event::KeyModifiers::NONE,
        ));
        let editing = render_text(&state, &app, 100, 24);
        assert!(editing.contains("Esc clear"), "{editing}");
        assert!(!editing.contains("q quit"), "{editing}");

        // A wide terminal gives the panels width rather than margins: a narrow
        // gutter, and no wider one however wide the terminal gets.
        let wide = render_text(&state, &app, 200, 24);
        assert!(
            wide.lines().any(|line| line.starts_with("  ┌Fleet")),
            "a small gutter on a wide terminal: {wide}"
        );
        assert!(
            wide.lines().all(|line| !line.starts_with("     ")),
            "and no larger one: {wide}"
        );
    }

    #[test]
    fn the_footer_names_focus_and_a_focus_message_takes_its_place() {
        let state = fixture_state();
        let mut app = app_for(&state);
        let screen = render_text(&state, &app, 100, 24);
        assert!(screen.contains("Enter focus"), "{screen}");
        assert!(screen.contains("q quit"), "{screen}");

        app.set_focus_message(Some("herdr refused to focus: pane wA:p9 not found".into()));
        let failed = render_text(&state, &app, 100, 24);
        assert!(
            failed.contains("herdr refused to focus: pane wA:p9 not found"),
            "{failed}"
        );
        assert!(!failed.contains("j/k"), "{failed}");
        // What the message must not replace: the source's own diagnostic.
        assert!(failed.contains("Fleet · current"), "{failed}");
    }

    #[test]
    fn durations_read_as_a_glance_wants_them() {
        use std::time::Duration;
        assert_eq!(duration(Duration::from_secs(0)), "0s");
        assert_eq!(duration(Duration::from_secs(59)), "59s");
        assert_eq!(duration(Duration::from_secs(60)), "1m00s");
        assert_eq!(duration(Duration::from_secs(158)), "2m38s");
        assert_eq!(duration(Duration::from_secs(60 * 60 - 1)), "59m59s");
        assert_eq!(duration(Duration::from_secs(3600)), "1h00m");
        assert_eq!(duration(Duration::from_secs(3600 + 14 * 60)), "1h14m");
        assert_eq!(duration(Duration::from_secs(26 * 3600)), "1d2h");
    }

    #[test]
    fn routine_states_leave_the_row_but_remain_in_details() {
        // The row's word comes from the pane's own facts, so each case is a
        // pane state rather than a projection.
        let cases: Vec<(HerdsmanFacts, RuntimeStatus, &str, bool)> = vec![
            (facts_with_model(), RuntimeStatus::Working, "working", true),
            (
                HerdsmanFacts {
                    awaited: vec!["agent:one".into()],
                    model: Some("model".into()),
                    ..HerdsmanFacts::default()
                },
                RuntimeStatus::Idle,
                "waiting",
                true,
            ),
            (facts_with_model(), RuntimeStatus::Idle, "idle", true),
            (facts_with_model(), RuntimeStatus::Unknown, "unknown", false),
            (
                facts_with_model(),
                RuntimeStatus::Other("blocked".into()),
                "blocked",
                false,
            ),
            (
                HerdsmanFacts {
                    state: Some(SemanticState::Lost),
                    model: Some("model".into()),
                    ..HerdsmanFacts::default()
                },
                RuntimeStatus::Idle,
                "lost",
                false,
            ),
        ];
        for (facts, status, word, routine) in cases {
            let projected = facts.state.is_some();
            let state = state_with_facts(facts, status);
            let mut app = app_for(&state);
            select_agent(&mut app, "wH:p1");
            let screen = render_text(&state, &app, 180, 34);
            assert!(
                screen.contains(&format!("state: {word} (derived from the pane)")),
                "{screen}"
            );
            assert_eq!(
                screen.contains("assignment: lost (owner projection)"),
                projected,
                "{screen}"
            );
            if routine {
                assert!(screen.contains("worker task · model"), "{screen}");
                assert!(
                    !screen.contains(&format!("worker task · {word}")),
                    "{screen}"
                );
            } else {
                assert!(
                    screen.contains(&format!("worker task · {word} · model")),
                    "{screen}"
                );
            }
        }
    }

    /// Facts carrying only a model, so a row has something to draw either side
    /// of the state word.
    fn facts_with_model() -> HerdsmanFacts {
        HerdsmanFacts {
            model: Some("model".into()),
            ..HerdsmanFacts::default()
        }
    }

    #[test]
    fn a_worker_row_reads_its_published_name_state_age_and_model() {
        let state = state_with_facts(
            HerdsmanFacts {
                label: Some("bash-pane-facts".into()),
                role: Some("worker".into()),
                definition: Some("pi-herdsman-worker".into()),
                assignment: Some("Run the terminal smoke suite".into()),
                assigned_for: Some(Duration::from_secs(158)),
                awaited: vec!["agent:one".into()],
                background_running: Some(1),
                background_tasks: vec!["bg-1".into()],
                background_started: Some("2026-01-01T00:00:00.000Z".into()),
                model: Some("example/model".into()),
                provider: Some("example".into()),
                thinking: Some("high".into()),
                context_usage: Some("43%".into()),
                session_name: Some("worker session".into()),
                run: Some("run-1".into()),
                request: Some("req-1".into()),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Working,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        // The row is the runtime label Herdsman publishes — neither Herdr's
        // truncated agent name nor its task-bearing title — with how long the
        // assignment has run and the model without its provider prefix.
        assert!(
            screen.contains("bash-pane-facts · 2m38s · model:high"),
            "{screen}"
        );
        // The badge counts unresolved tasks, and moves while one is running.
        assert!(screen.contains("1 bg"), "{screen}");
        assert!(!screen.contains("worker task"), "{screen}");

        // Everything else is a details line, each with its own label.
        for fact in [
            "role: worker",
            "definition: pi-herdsman-worker",
            "assignment: Run the terminal smoke suite",
            "assigned for: 2m38s",
            "awaiting: agent:one, 1 background task",
            "background: bg-1",
            "background started: 2026-01-01T00:00:00.000Z",
            "model: example/model",
            "provider: example",
            "thinking: high",
            "context usage: 43%",
            "session name: worker session",
            "run: run-1",
            "request: req-1",
        ] {
            assert!(screen.contains(fact), "missing `{fact}`: {screen}");
        }
    }

    #[test]
    fn the_row_badge_counts_outstanding_tasks_not_the_running_count() {
        // The live shape: the count reports nothing running while five tasks
        // have exited into review.
        let state = state_with_facts(
            HerdsmanFacts {
                background_running: Some(0),
                background_tasks: [
                    "bg-2286:review",
                    "bg-2284:review",
                    "bg-2283:review",
                    "bg-2282:review",
                    "bg-2281:review",
                ]
                .map(String::from)
                .to_vec(),
                background_started: Some("2026-01-01T00:00:00.000Z".into()),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Idle,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        // The row counts what is unresolved, and the details keep the
        // published running count and the oldest outstanding start beside it.
        assert!(screen.contains("worker task"), "{screen}");
        assert!(screen.contains("5 bg"), "{screen}");
        assert!(
            screen.contains("state: waiting (derived from the pane)"),
            "{screen}"
        );
        assert!(screen.contains("awaiting: 5 background tasks"), "{screen}");
        assert!(screen.contains("background running: 0"), "{screen}");
        assert!(
            screen.contains("background started: 2026-01-01T00:00:00.000Z"),
            "{screen}"
        );
        assert!(screen.contains("background: bg-2286:review"), "{screen}");
    }

    #[test]
    fn a_working_pane_still_shows_what_it_awaits() {
        // A pane that spawned two tasks and is still working, one of them
        // running and one exited into review.
        let state = state_with_facts(
            HerdsmanFacts {
                awaited: vec!["agent:researcher-1".into()],
                background_running: Some(1),
                background_tasks: vec!["bg-2:running".into(), "bg-1:review".into()],
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Working,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        // Work in flight stays in flight; the whole outstanding set is still
        // named, and its two halves stay distinguishable.
        assert!(
            screen.contains("state: working (derived from the pane)"),
            "{screen}"
        );
        assert!(
            screen.contains("awaiting: agent:researcher-1, 2 background tasks"),
            "{screen}"
        );
        assert!(screen.contains("worker task"), "{screen}");
        assert!(screen.contains("2 bg"), "{screen}");
        assert!(screen.contains("background running: 1"), "{screen}");
        assert!(!screen.contains("assignment:"), "{screen}");
    }

    #[test]
    fn a_phase_word_radar_does_not_know_is_shown_as_published() {
        let state = state_with_facts(
            HerdsmanFacts {
                background_running: Some(0),
                // An id with a colon of its own, a phase this version does not
                // know, and an entry published with no phase at all.
                background_tasks: vec!["wA:bg-9:quiescing".into(), "bg-7".into()],
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Idle,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        assert!(screen.contains("worker task · 2 bg"), "{screen}");
        assert!(screen.contains("awaiting: 2 background tasks"), "{screen}");
        assert!(
            screen.contains("background: wA:bg-9:quiescing, bg-7"),
            "{screen}"
        );
        assert!(
            screen.contains("state: waiting (derived from the pane)"),
            "{screen}"
        );
    }

    #[test]
    fn a_row_shows_no_age_it_was_not_given_and_still_shows_its_model() {
        let state = state_with_facts(
            HerdsmanFacts {
                model: Some("example/model".into()),
                provider: Some("example".into()),
                thinking: Some("high".into()),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Working,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");

        // With the details hidden the row is the whole line, so it can be
        // asserted where it ends: no age, and no separator left dangling.
        press(&mut app, 'd');
        let tree = render_text(&state, &app, 180, 34);
        let row = tree
            .lines()
            .find(|line| line.contains("worker task"))
            .expect("the agent row");
        assert!(row_cell(row).ends_with("worker task · model:high"), "{row}");

        press(&mut app, 'd');
        let details = render_text(&state, &app, 180, 34);
        assert!(details.contains("model: example/model"), "{details}");
        assert!(!details.contains("assigned for:"), "{details}");
    }

    #[test]
    fn the_row_draws_the_panes_activity_and_the_details_name_the_owners_projection() {
        // The owner says blocked; the pane's own facts — an idle agent with a
        // child still outstanding — derive waiting, and the row follows the
        // pane.
        let state = state_with_facts(
            HerdsmanFacts {
                state: Some(SemanticState::Blocked),
                awaited: vec!["agent:one".into()],
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Idle,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        // Waiting is a routine state, so the row carries it as mark and colour
        // alone; the panel names it and names the projection beside it.
        assert!(screen.contains("worker task"), "{screen}");
        assert!(!screen.contains("worker task · waiting"), "{screen}");
        assert!(
            screen.contains("state: waiting (derived from the pane)"),
            "{screen}"
        );
        assert!(
            screen.contains("assignment: blocked (owner projection)"),
            "{screen}"
        );
        assert!(screen.contains("status: idle"), "{screen}");
    }

    #[test]
    fn an_agent_without_herdsman_facts_draws_no_herdsman_field() {
        let state = state_with_facts(HerdsmanFacts::default(), RuntimeStatus::Idle);
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        for absent in [
            "role:",
            "assignment:",
            "definition:",
            "assignment:",
            "assigned for:",
            "awaiting:",
            "background:",
            "model:",
            "provider:",
            "thinking:",
            "context usage:",
            "session name:",
            "run:",
            "request:",
            "ask:",
        ] {
            assert!(
                !screen.contains(absent),
                "`{absent}` with no fact: {screen}"
            );
        }
        assert!(
            screen.contains("state: idle (derived from the pane)"),
            "{screen}"
        );
    }

    #[test]
    fn a_lead_is_named_by_its_published_name_and_shows_its_pending_ask() {
        let state = state_with_facts(
            HerdsmanFacts {
                name: Some("fleet lead".into()),
                role: Some("lead".into()),
                definition: Some("lead".into()),
                ask: Some("ask-1".into()),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Idle,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_text(&state, &app, 180, 34);

        assert!(screen.contains("fleet lead"), "{screen}");
        assert!(!screen.contains("fleet lead · idle"), "{screen}");
        assert!(!screen.contains("worker task"), "{screen}");
        assert!(screen.contains("role: lead"), "{screen}");
        assert!(screen.contains("ask: ask-1"), "{screen}");
        // The definition says nothing the role has not already said.
        assert!(!screen.contains("definition:"), "{screen}");
    }

    /// Where `needle` begins in the rendered buffer, matched cell by cell so a
    /// wide glyph elsewhere in the row cannot shift it.
    fn find_text(buffer: &ratatui::buffer::Buffer, needle: &str) -> (u16, u16) {
        let columns: Vec<String> = needle.chars().map(|ch| ch.to_string()).collect();
        for y in 0..buffer.area.height {
            'row: for x in 0..buffer
                .area
                .width
                .saturating_sub(needle.chars().count() as u16)
            {
                for (index, symbol) in columns.iter().enumerate() {
                    if buffer[(x + index as u16, y)].symbol() != symbol {
                        continue 'row;
                    }
                }
                return (x, y);
            }
        }
        panic!("`{needle}` is not on screen");
    }

    #[test]
    fn a_workspace_heading_is_drawn_as_a_heading() {
        let state = fixture_state();
        let app = app_for(&state);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("infallible");
        terminal
            .draw(|frame| {
                render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();

        // Asserted on an unselected row: the selection fill is a style of its
        // own, and would make any cell in its row read as bold.
        let (x, y) = find_text(buffer, "docs");
        let label = buffer[(x, y)].style();
        assert_eq!(label.fg, Some(theme::palette().heading), "{label:?}");
        assert!(
            label.add_modifier.contains(Modifier::BOLD),
            "a heading carries weight: {label:?}"
        );
        let (x, y) = find_text(buffer, "· 2");
        let number = buffer[(x, y)].style();
        assert_eq!(number.fg, Some(theme::palette().subtle), "{number:?}");
        assert!(!number.add_modifier.contains(Modifier::BOLD), "{number:?}");
    }

    #[test]
    fn panel_frames_take_the_border_role() {
        let state = fixture_state();
        let app = app_for(&state);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("infallible");
        terminal
            .draw(|frame| {
                render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();

        // The frame and its title both carry the border ink, so a theme can
        // dim the chrome without touching the content it surrounds.
        for needle in ["┌Fleet", "Fleet", "Details"] {
            let (x, y) = find_text(buffer, needle);
            assert_eq!(
                buffer[(x, y)].style().fg,
                Some(theme::palette().border),
                "{needle}"
            );
        }
    }

    /// A snapshot of one agent per pane, in as many workspaces as the test
    /// names: `(pane, workspace, agent_status, title)`.
    ///
    /// The real-shaped fixture is documentary; a test that needs a particular
    /// state or label places it here instead.
    fn state_of(agents: &[(&str, &str, &str, &str, &str)]) -> ObservationState {
        let mut workspaces: Vec<&str> = Vec::new();
        for (_, workspace, _, _, _) in agents {
            if !workspaces.contains(workspace) {
                workspaces.push(workspace);
            }
        }
        let workspace_records: Vec<String> = workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| {
                format!(
                    r#"{{"workspace_id":"{workspace}","label":"{workspace}","number":{},"agent_status":"idle","tokens":{{}}}}"#,
                    index + 1
                )
            })
            .collect();
        let tab_records: Vec<String> = workspaces
            .iter()
            .map(|workspace| {
                format!(
                    r#"{{"tab_id":"{workspace}:t1","workspace_id":"{workspace}","label":"tab","number":1,"agent_status":"idle"}}"#
                )
            })
            .collect();
        let pane_records: Vec<String> = agents
            .iter()
            .map(|(pane, workspace, status, title, tokens)| {
                format!(
                    r#"{{"pane_id":"{pane}","tab_id":"{workspace}:t1","workspace_id":"{workspace}","label":"{title}","agent":"pi","agent_status":"{status}","terminal_title":"{title}","tokens":{{{tokens}}}}}"#
                )
            })
            .collect();
        let agent_records: Vec<String> = agents
            .iter()
            .map(|(pane, workspace, status, title, tokens)| {
                format!(
                    r#"{{"pane_id":"{pane}","tab_id":"{workspace}:t1","workspace_id":"{workspace}","agent":"pi","agent_status":"{status}","terminal_title":"{title}","tokens":{{{tokens}}}}}"#
                )
            })
            .collect();
        let json = format!(
            r#"{{"result":{{"snapshot":{{"workspaces":[{}],"tabs":[{}],"panes":[{}],"agents":[{}]}}}}}}"#,
            workspace_records.join(","),
            tab_records.join(","),
            pane_records.join(","),
            agent_records.join(","),
        );
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(&json).expect("the built snapshot decodes"));
        state
    }

    /// The rows on screen by title, in the order they are drawn.
    fn drawn_titles(app: &App) -> Vec<String> {
        app.visible_rows()
            .iter()
            .map(|row| row.node.row.title().to_string())
            .collect()
    }

    /// Where a draw would have put things, for the pointer tests: the panels
    /// this view draws, with room above the details so their scroll has
    /// something to move over.
    fn geometry_of(app: &App) -> Geometry {
        Geometry {
            tree_panel: Rect::new(0, 0, 40, 20),
            tree_content: Rect::new(1, 1, 38, 18),
            details: app.shows_details().then(|| Rect::new(40, 0, 30, 20)),
            details_lines: 100,
            offset: 0,
        }
    }

    fn mouse_at(at: (u16, u16), kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: at.0,
            row: at.1,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click_at(app: &mut App, at: (u16, u16)) -> Option<Action> {
        app.handle_mouse(mouse_at(at, MouseEventKind::Down(MouseButton::Left)))
    }

    #[test]
    fn a_click_selects_the_row_it_lands_on_and_a_second_click_focuses_it() {
        let state = state_of(&[
            ("wA:p1", "wA", "idle", "first", ""),
            ("wA:p2", "wA", "idle", "second", ""),
        ]);
        let mut app = app_for(&state);
        let geometry = geometry_of(&app);
        app.note_layout(geometry);

        // Row 0 is the workspace heading; row 1 its first child.
        let (x, first_child) = (geometry.tree_content.x + 2, geometry.tree_content.y + 1);
        assert!(click_at(&mut app, (x, first_child)).is_none());
        assert_eq!(
            app.selected_row()
                .map(|row| row.node.row.title().to_string()),
            Some("first".to_string())
        );

        // The row is selected now, so the next click is the one that acts.
        assert!(
            matches!(click_at(&mut app, (x, first_child)), Some(Action::Focus(_))),
            "a click on the already-selected row focuses its pane"
        );
    }

    #[test]
    fn a_click_on_a_heading_folds_it_and_a_click_beside_a_row_does_nothing() {
        let state = state_of(&[("wA:p1", "wA", "idle", "first", "")]);
        let mut app = app_for(&state);
        let geometry = geometry_of(&app);
        app.note_layout(geometry);
        let before = app.visible_rows().len();

        let (x, heading) = (geometry.tree_content.x + 2, geometry.tree_content.y);
        assert!(click_at(&mut app, (x, heading)).is_none());
        assert!(
            app.visible_rows().len() < before,
            "a click on a heading folds it"
        );
        click_at(&mut app, (x, heading));
        assert_eq!(
            app.visible_rows().len(),
            before,
            "the next click unfolds it again"
        );

        let selected = app
            .selected_row()
            .map(|row| row.node.row.title().to_string());
        // Below the last row, and over the details panel: neither holds a row.
        let empty = geometry.tree_content.bottom() + 2;
        assert!(click_at(&mut app, (x, empty)).is_none());
        click_at(
            &mut app,
            (geometry.details.expect("details are drawn").x + 2, 3),
        );
        assert_eq!(
            app.selected_row()
                .map(|row| row.node.row.title().to_string()),
            selected,
            "a click with no row under it leaves the selection alone"
        );
    }

    #[test]
    fn the_wheel_scrolls_the_panel_under_the_pointer() {
        let state = state_of(&[
            ("wA:p1", "wA", "idle", "one", ""),
            ("wA:p2", "wA", "idle", "two", ""),
            ("wA:p3", "wA", "idle", "three", ""),
            ("wA:p4", "wA", "idle", "four", ""),
        ]);
        let mut app = app_for(&state);
        // A viewport shorter than the list, so there is somewhere to scroll to.
        let geometry = Geometry {
            tree_content: Rect::new(1, 1, 38, 3),
            ..geometry_of(&app)
        };
        app.note_layout(geometry);

        let tree = (geometry.tree_content.x + 2, geometry.tree_content.y + 1);
        app.handle_mouse(mouse_at(tree, MouseEventKind::ScrollDown));
        assert_eq!(app.take_scroll(), Some(2), "the tree scrolls down");
        assert_eq!(app.take_scroll(), None, "and the request is taken once");
        // The main loop applies the request, and the next draw reports it.
        app.note_layout(Geometry {
            offset: 2,
            ..geometry
        });
        app.handle_mouse(mouse_at(tree, MouseEventKind::ScrollUp));
        assert_eq!(app.take_scroll(), Some(0), "and back up");

        let details = (geometry.details.expect("drawn").x + 2, 4);
        app.handle_mouse(mouse_at(details, MouseEventKind::ScrollDown));
        assert_eq!(app.details_scroll(), 3);
        assert_eq!(
            app.take_scroll(),
            None,
            "a wheel over the details does not scroll the tree"
        );

        // Nowhere in particular: nothing moves.
        app.handle_mouse(mouse_at((200, 200), MouseEventKind::ScrollDown));
        assert_eq!(app.take_scroll(), None);
        assert_eq!(app.details_scroll(), 3);
    }

    /// The row at a drawn index, clicked once.
    fn click_row(app: &mut App, geometry: Geometry, index: usize) -> Option<Action> {
        click_at(
            app,
            (
                geometry.tree_content.x + 2,
                geometry.tree_content.y + index as u16,
            ),
        )
    }

    fn selected_id(app: &App) -> Option<crate::tree::RowId> {
        app.selected_row().map(|row| row.id.clone())
    }

    #[test]
    fn a_fold_and_the_selected_row_survive_a_re_sort() {
        let state = state_of(&[
            ("wA:p1", "wA", "idle", "alpha", ""),
            ("wA:p2", "wA", "working", "beta", ""),
            ("wA:p3", "wA", "done", "gamma", ""),
        ]);
        let mut app = app_for(&state);
        let beta = crate::tree::RowId::Agent("wA:p2".to_string());
        let workspace = crate::tree::RowId::Workspace("wA".to_string());

        select_row(&mut app, beta.clone());
        press(&mut app, 's');
        assert_eq!(
            drawn_titles(&app),
            ["wA", "beta", "alpha", "gamma"],
            "state order moves the working row to the front of its level"
        );
        assert_eq!(selected_id(&app), Some(beta), "selection follows the row");

        select_row(&mut app, workspace.clone());
        press(&mut app, ' ');
        assert!(app.is_collapsed(&workspace));
        press(&mut app, 's');
        press(&mut app, 's');
        assert!(
            app.is_collapsed(&workspace),
            "a fold is keyed by row, so re-sorting leaves it folded"
        );
        assert_eq!(selected_id(&app), Some(workspace));
    }

    #[test]
    fn a_retained_row_ranks_after_every_observed_one() {
        let state = retained_working_state();
        let mut app = app_for(&state);
        press(&mut app, 's');
        let order: Vec<crate::tree::RowId> = app
            .visible_rows()
            .iter()
            .map(|row| row.id.clone())
            .collect();
        let retained = order
            .iter()
            .position(|id| id == &crate::tree::RowId::Agent("wA:p2".to_string()))
            .expect("the retained row is drawn");
        let observed = order
            .iter()
            .position(|id| matches!(id, crate::tree::RowId::Agent(pane) if pane != "wA:p2"))
            .expect("an observed agent is drawn");
        assert!(
            retained > observed,
            "a retained row keeps the last facts anyone saw, so it cannot outrank \
             a row Radar is observing now"
        );
    }

    #[test]
    fn a_jump_never_selects_a_row_the_view_hides() {
        let state = state_of(&[(
            "wA:p1",
            "wA",
            "idle",
            "stuck",
            r#""pi_herdsman_state":"lost""#,
        )]);
        let mut app = app_for(&state);
        let workspace = crate::tree::RowId::Workspace("wA".to_string());
        select_row(&mut app, workspace.clone());
        press(&mut app, ' ');
        assert_eq!(app.visible_rows().len(), 1, "the lost row is folded away");

        press(&mut app, 'n');
        assert_eq!(
            selected_id(&app),
            Some(workspace.clone()),
            "a jump cannot reach a row the fold hides"
        );

        press(&mut app, ' ');
        press(&mut app, '/');
        for character in "nothing matches this".chars() {
            press(&mut app, character);
        }
        press(&mut app, '\n');
        assert!(
            app.visible_rows().is_empty(),
            "the filter excludes every row, the lost one included"
        );
        press(&mut app, 'n');
        assert_ne!(
            selected_id(&app),
            Some(crate::tree::RowId::Agent("wA:p1".to_string())),
            "and a jump cannot select one the filter excludes"
        );
    }

    #[test]
    fn an_empty_view_survives_a_jump_and_an_order_cycle() {
        let mut app = App::new();
        for key in ['n', 'N', 'w', 'W', 's'] {
            press(&mut app, key);
        }
        assert_eq!(app.order().label(), "state");
        assert_eq!(app.visible_rows().len(), 0);
    }

    #[test]
    fn a_click_maps_to_the_row_drawn_there_after_a_fold() {
        let state = state_of(&[
            ("wA:p1", "wA", "idle", "alpha", ""),
            ("wB:p1", "wB", "idle", "gamma", ""),
        ]);
        let mut app = app_for(&state);
        let geometry = geometry_of(&app);
        app.note_layout(geometry);
        assert_eq!(drawn_titles(&app), ["wA", "alpha", "wB", "gamma"]);

        // Fold the first workspace by clicking its heading, then the row drawn
        // below it is the next workspace rather than the child it replaced.
        click_row(&mut app, geometry, 0);
        assert_eq!(drawn_titles(&app), ["wA", "wB", "gamma"]);
        click_row(&mut app, geometry, 1);
        assert_eq!(
            app.selected_row()
                .map(|row| row.node.row.title().to_string()),
            Some("wB".to_string())
        );
    }

    #[test]
    fn a_retained_row_focuses_the_pane_it_was_last_seen_on() {
        let state = retained_working_state();
        let mut app = app_for(&state);
        let geometry = geometry_of(&app);
        app.note_layout(geometry);
        let index = app
            .visible_rows()
            .iter()
            .position(|row| row.id == &crate::tree::RowId::Agent("wA:p2".to_string()))
            .expect("the retained row is drawn");

        click_row(&mut app, geometry, index);
        assert!(
            matches!(
                click_row(&mut app, geometry, index),
                Some(Action::Focus(Target::Pane(pane))) if pane == "wA:p2"
            ),
            "a retained row's pane is the useful thing to focus"
        );
    }

    #[test]
    fn a_click_with_a_stale_inventory_answers_with_a_message_not_a_request() {
        let mut state = state_of(&[("wA:p1", "wA", "idle", "alpha", "")]);
        let mut app = app_for(&state);
        let geometry = geometry_of(&app);
        app.note_layout(geometry);

        // The row is selected by the first click, so the second is the one that
        // acts.
        let child = 1;
        click_row(&mut app, geometry, child);
        assert!(matches!(
            click_row(&mut app, geometry, child),
            Some(Action::Focus(_))
        ));

        state.apply_failure("herdr exited with status 1");
        app.refresh(&state);
        app.note_layout(geometry);
        assert!(
            click_row(&mut app, geometry, child).is_none(),
            "a last-good row is not a location Radar can still see"
        );
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("stale")),
            "the click says why rather than moving the terminal"
        );
    }

    #[test]
    fn the_order_cycles_and_names_itself_in_the_footer() {
        let state = state_of(&[("wA:p1", "wA", "working", "moving", "")]);
        let mut app = app_for(&state);
        assert_eq!(app.order(), crate::app::RowOrder::Source);
        for (key, label) in [("s", "state"), ("s", "name"), ("s", "source")] {
            press(&mut app, key.chars().next().unwrap());
            assert_eq!(app.order().label(), label);
            let screen = render_text(&state, &app, 180, 30);
            assert!(screen.contains(&format!("s {label}")), "{screen}");
        }
        // While the filter is being edited the key is text, not a command.
        press(&mut app, '/');
        press(&mut app, 's');
        assert_eq!(app.order(), crate::app::RowOrder::Source);
        assert_eq!(app.filter_query(), "s");
    }

    #[test]
    fn state_order_puts_what_needs_a_human_first_and_keeps_children_under_their_parent() {
        let state = state_of(&[
            ("wA:p1", "wA", "working", "moving", ""),
            ("wA:p2", "wA", "done", "finished", ""),
            ("wA:p3", "wA", "idle", "parked", ""),
            (
                "wA:p4",
                "wA",
                "idle",
                "stuck",
                r#""pi_herdsman_state":"lost""#,
            ),
            ("wB:p1", "wB", "idle", "quiet", ""),
        ]);
        let mut app = app_for(&state);
        assert_eq!(
            drawn_titles(&app),
            ["wA", "moving", "finished", "parked", "stuck", "wB", "quiet"]
        );

        press(&mut app, 's');
        assert_eq!(
            drawn_titles(&app),
            ["wA", "stuck", "moving", "parked", "finished", "wB", "quiet"],
            "a group ranks by its most urgent row, and a child never leaves its parent"
        );

        press(&mut app, 's');
        assert_eq!(
            drawn_titles(&app),
            ["wA", "finished", "moving", "parked", "stuck", "wB", "quiet"],
            "name order is alphabetical within the level"
        );
    }

    #[test]
    fn a_jump_reaches_the_next_row_of_its_kind_and_wraps() {
        let state = state_of(&[
            ("wA:p1", "wA", "working", "moving", ""),
            ("wA:p2", "wA", "blocked", "stuck one", ""),
            ("wA:p3", "wA", "done", "finished", ""),
            ("wA:p4", "wA", "blocked", "stuck two", ""),
        ]);
        let mut app = app_for(&state);
        let selected = |app: &App| {
            app.selected_row()
                .map(|row| row.node.row.title().to_string())
        };

        // Nothing is selected, so a forward jump starts at the first match.
        press(&mut app, 'n');
        assert_eq!(selected(&app).as_deref(), Some("stuck one"));
        press(&mut app, 'n');
        assert_eq!(selected(&app).as_deref(), Some("stuck two"));
        press(&mut app, 'n');
        assert_eq!(selected(&app).as_deref(), Some("stuck one"), "it wraps");

        press(&mut app, 'N');
        assert_eq!(
            selected(&app).as_deref(),
            Some("stuck two"),
            "backwards wraps the other way"
        );

        press(&mut app, 'w');
        assert_eq!(selected(&app).as_deref(), Some("moving"));
        press(&mut app, 'w');
        assert_eq!(
            selected(&app).as_deref(),
            Some("moving"),
            "a repeat with no other working row stays on it"
        );
    }

    #[test]
    fn a_jump_that_finds_nothing_moves_nothing() {
        let state = state_of(&[("wA:p1", "wA", "done", "finished", "")]);
        let mut app = app_for(&state);
        select_row(&mut app, crate::tree::RowId::Agent("wA:p1".to_string()));
        press(&mut app, 'w');
        press(&mut app, 'W');
        press(&mut app, 'n');
        assert_eq!(
            app.selected_row()
                .map(|row| row.node.row.title().to_string()),
            Some("finished".to_string())
        );
    }

    #[test]
    fn a_pane_row_is_never_blank_and_moves_while_a_command_runs() {
        // Which animation is installed is global to the test process, so this
        // asserts that the mark moves rather than which frame it shows.
        let config = crate::config::Config::parse("[appearance]\ncommand = \"orbit\"\n")
            .expect("the running mark parses");
        theme::install(config);
        assert!(theme::command_frames().is_some(), "the running marks move");

        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p6",
            ForegroundEvidence::command(8, Some("nix".into()), Some("nix build .#radar".into())),
        );
        let mut app = app_for(&state);
        show_all_panes(&mut app);

        let first = render_with(&state, &app, 180, 34, &mut ListState::default(), 0);
        let second = render_with(&state, &app, 180, 34, &mut ListState::default(), 1);

        // A pane with nothing running in it still leads with the pane's own
        // mark, so its row is never a blank column under a marked agent row.
        let pane_mark = theme::pane_mark();
        assert!(first.contains(&format!("{pane_mark} ")), "{first}");

        // A running command's mark advances with the clock.
        let lead = |screen: &str| {
            screen
                .lines()
                .find(|line| line.contains("nix build .#radar"))
                .and_then(|line| line.trim_start_matches(['\u{2502}', ' ']).chars().next())
        };
        assert!(lead(&first).is_some_and(|mark| mark != ' '), "{first}");
        assert_ne!(
            lead(&first),
            lead(&second),
            "the running mark advances with the clock"
        );
        assert!(
            app.animates(),
            "a command in a pane keeps the clock running"
        );
    }

    #[test]
    fn a_full_screen_program_and_a_command_read_differently() {
        use crate::model::{LocalFacts, TerminalMode};

        let mut state = fixture_state();
        for (pane, pid, command, running, terminal) in [
            (
                "wA:p3",
                7,
                "nvim notes.md",
                5 * 3600 + 14 * 60,
                TerminalMode::FullScreen,
            ),
            ("wA:p6", 8, "nix build .#radar", 240, TerminalMode::Line),
        ] {
            let mut evidence = ForegroundEvidence::command(
                pid,
                command.split(' ').next().map(str::to_string),
                Some(command.to_string()),
            );
            if let ForegroundEvidence::NonShell { local, .. } = &mut evidence {
                *local = LocalFacts {
                    running_for: Some(std::time::Duration::from_secs(running)),
                    terminal,
                };
            }
            state.apply_evidence(pane, evidence);
        }

        let mut app = app_for(&state);
        show_all_panes(&mut app);
        let screen = render_text(&state, &app, 180, 30);
        // A program that owns the pane, with how long it has owned it...
        // A program Radar has a mark for carries it; a Nerd Font decides
        // whether that is a drawn glyph or the mode mark standing in for it.
        let editor = theme::process_mark("nvim").unwrap_or("▣");
        assert!(
            screen.contains(&format!("{editor} nvim notes.md · 5h14m")),
            "{screen}"
        );
        // ...and a command that will hand it back.
        assert!(screen.contains("❯ nix build .#radar · 4m"), "{screen}");

        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        let details = render_text(&state, &app, 180, 30);
        assert!(details.contains("foreground: nvim notes.md"), "{details}");
        assert!(
            details.contains("terminal: full screen — the program is drawing the pane"),
            "{details}"
        );
        assert!(details.contains("running for: 5h14m"), "{details}");
    }

    #[test]
    fn details_sit_beside_the_tree_and_toggle_away() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");

        let side_by_side = render_text(&state, &app, 140, 30);
        let header = side_by_side
            .lines()
            .find(|line| line.contains("┌Fleet"))
            .expect("the tree panel");
        assert!(
            header.contains("┌Details"),
            "details share the row: {header}"
        );
        assert!(side_by_side.contains("d hide details"), "{side_by_side}");

        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('d'),
            crossterm::event::KeyModifiers::NONE,
        ));
        let without = render_text(&state, &app, 140, 30);
        assert!(!without.contains("Details"), "{without}");
        assert!(without.contains("d details"), "{without}");
        assert!(!without.contains("d hide details"), "{without}");
        // The tree keeps the whole width.
        assert!(
            without.lines().any(|line| line.contains("┌Fleet")
                && line.contains("┐")
                && line.matches("┐").count() == 1),
            "{without}"
        );
    }

    #[test]
    fn the_working_mark_follows_the_frame_counter() {
        let state = fixture_state();
        let app = app_for(&state);
        assert!(app.animates(), "the fixture has a working agent");
        let frames = theme::frames(theme::config().appearance.working)
            .expect("the configured animation exists");

        // The row the mark belongs to, as drawn at a given frame.
        let row = |tick: usize| {
            render_with(&state, &app, 90, 30, &mut ListState::default(), tick)
                .lines()
                .find(|line| line.contains("worker task"))
                .expect("the working row is on screen")
                .to_string()
        };

        // Every tick draws that frame of the animation, so the mark follows the
        // clock rather than the redraw that happened to happen.
        for tick in 0..frames.len() * 2 {
            let frame = frames[tick % frames.len()].to_string();
            assert!(
                row(tick).contains(&frame),
                "tick {tick} draws `{frame}`: {}",
                row(tick)
            );
        }
        assert_eq!(row(3), row(3), "a frame is a function of the tick");
        assert_ne!(row(3), row(4), "the next tick moves it on");
    }

    /// A fleet whose only working agent has gone: its row is retained, so the
    /// last status anyone saw is `working` with nothing observing it now.
    fn retained_working_state() -> ObservationState {
        let mut state = fixture_state();
        let mut without_worker = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        without_worker
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p2");
        state.apply_success(without_worker);
        state.apply_evidence("wA:p2", ForegroundEvidence::Shell);
        state
    }

    #[test]
    fn a_retained_row_holds_its_mark_still() {
        // A retained row carries the last status anyone saw, so it must not
        // claim activity: its frame does not depend on the tick.
        let state = retained_working_state();
        let app = app_for(&state);
        assert!(!app.animates(), "nothing observed now is working");
        let drawn =
            |tick: usize| render_with(&state, &app, 90, 30, &mut ListState::default(), tick);
        assert_eq!(drawn(0), drawn(3));
    }

    #[test]
    fn agent_text_carries_the_same_state_colour_as_its_mark() {
        use crate::model::AgentState;
        let state = fixture_state();
        let app = app_for(&state);
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).expect("infallible");
        terminal
            .draw(|frame| {
                render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();

        let expected = |state: AgentState| theme::agent_state(&state, Some("pi"), false, 0).1;
        for (title, status) in [
            ("worker task", RuntimeStatus::Working),
            ("monitor row", RuntimeStatus::Done),
            ("review task", RuntimeStatus::Idle),
        ] {
            let y = (0..buffer.area.height)
                .find(|y| row_text(buffer, *y).contains(title))
                .unwrap_or_else(|| panic!("row for {title}"));
            let x = (0..buffer.area.width)
                .find(|x| buffer[(*x, y)].symbol() == &title[..1])
                .expect("first character of the title");
            assert_eq!(
                buffer[(x, y)].style().fg,
                Some(expected((&status).into())),
                "{title} is drawn in its state's colour"
            );
        }
    }

    #[test]
    fn a_running_pane_leads_with_its_command_and_reports_it_in_the_details() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p3",
            ForegroundEvidence::command(
                7,
                Some("nix".into()),
                Some("/nix/store/abc-nix/bin/nix build .#radar".into()),
            ),
        );
        let mut app = app_for(&state);
        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        let screen = render_text(&state, &app, 180, 32);

        assert!(screen.contains("nix build .#radar"), "{screen}");
        assert!(screen.contains("foreground: nix build .#radar"), "{screen}");
        // The executable's own path never reaches the screen.
        assert!(!screen.contains("/nix/store/abc-nix"), "{screen}");
    }

    /// A pane the fixture reports with no agent, but whose label is a finished
    /// Pi session's title.
    fn latched_session_state() -> ObservationState {
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        for pane in &mut observation.panes {
            if pane.location.pane_id == "wA:p1" {
                pane.label = Some("π 01a0edc4".into());
                pane.title = Some("~/P/d/nix-homelab".into());
            }
        }
        observation
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p1");
        let mut state = ObservationState::new();
        state.apply_success(observation);
        state
    }

    fn press(app: &mut App, key: char) {
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(key),
            crossterm::event::KeyModifiers::NONE,
        ));
    }

    #[test]
    fn a_finished_session_keeps_its_name_and_says_it_has_exited() {
        let state = latched_session_state();
        let mut app = app_for(&state);
        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p1".into()));
        let screen = render_text(&state, &app, 180, 32);

        // The row is the session it was — its vendor's mark and its own name —
        // and states what it is now.
        let logo = theme::logo(Some("pi")).expect("pi has a mark");
        assert!(
            screen.contains(&format!("{EXITED} {logo} 01a0edc4 · exited")),
            "{screen}"
        );
        // The details say what the pane is instead of it, rather than offering
        // the dead title as the pane's current name.
        assert!(screen.contains("kind: pane (finished session)"), "{screen}");
        assert!(screen.contains("session: 01a0edc4"), "{screen}");
        assert!(screen.contains("pane now: ~/P/d/nix-homelab"), "{screen}");
    }

    #[test]
    fn finished_sessions_stay_out_of_the_agents_view_until_asked_for() {
        let state = latched_session_state();
        let mut app = app_for(&state);

        // The default view is the live fleet, and says how to see the rest.
        let fleet = render_text(&state, &app, 180, 32);
        assert!(!fleet.contains("01a0edc4"), "{fleet}");
        assert!(fleet.contains("e finished"), "{fleet}");
        assert!(!fleet.contains("e hide finished"), "{fleet}");

        press(&mut app, 'e');
        let with_history = render_text(&state, &app, 180, 32);
        assert!(with_history.contains("01a0edc4 · exited"), "{with_history}");
        assert!(with_history.contains("e hide finished"), "{with_history}");

        press(&mut app, 'e');
        let fleet_again = render_text(&state, &app, 180, 32);
        assert!(!fleet_again.contains("01a0edc4"), "{fleet_again}");
    }

    #[test]
    fn a_pane_in_use_again_is_not_history() {
        let mut state = latched_session_state();
        state.apply_evidence(
            "wA:p1",
            ForegroundEvidence::command(9, Some("nvim".into()), Some("nvim notes.md".into())),
        );
        let mut app = app_for(&state);
        press(&mut app, 'e');

        // The pane is working again, so it is not a finished session: the agent
        // view does not list it, and the pane view leads with the command.
        let fleet = render_text(&state, &app, 180, 32);
        assert!(!fleet.contains("01a0edc4"), "{fleet}");
        press(&mut app, 'p');
        let panes = render_text(&state, &app, 180, 32);
        assert!(panes.contains("nvim notes.md"), "{panes}");
        assert!(!panes.contains("· exited"), "{panes}");
    }
}
