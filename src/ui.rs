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
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{
    App, DetailPage, Disclosure, Geometry, Operation, ProcessRow, ProcessTable, VisibleRow,
};
use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bus::Task;
use crate::model::{
    AgentObservation, AgentState, BinaryFreshness, BinaryIdentity, BinaryUnknown, CpuPercent,
    DescendantSample, ForegroundEvidence, HerdsmanFacts, ProcessIdentity, ProcessResources,
    ProcessState, SessionIdentity, SessionUuid, TerminalMode, Total,
};
use crate::observation::{ObservationState, RetentionBasis, SourceFreshness};
use crate::theme;
use crate::tree::{AgentRow, RowKind, TaskRow, TaskSource};

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

    let details = app.shows_details().then(|| {
        detail_page_lines(
            state,
            app,
            app.detail_page(),
            detail_content_width(body.width),
            app.process_selection(),
        )
    });
    let (tree_area, detail_area) = body_areas(
        body,
        details.as_ref().map(|page| page.lines.as_slice()),
        app,
    );

    let rows = app.visible_rows();
    let block = Block::bordered()
        .border_style(Style::new().fg(panel_border(!app.details_focused())))
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
            .map(|row| {
                row_item(
                    row,
                    tick,
                    tree_area.width.saturating_sub(2) as usize,
                    matches!(state.source_freshness(), SourceFreshness::Current),
                )
            })
            .collect();
        list_state.select(app.selected_index());
        let list = List::new(items).block(block).highlight_style(
            Style::new()
                .bg(theme::palette().selection)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, tree_area, list_state);
    }

    let mut detail_tabs = [None; DetailPage::COUNT];
    let mut disclosure_markers: Vec<(Disclosure, Rect)> = Vec::new();
    let mut process_rows: Vec<(ProcessIdentity, usize, Option<Rect>)> = Vec::new();
    let mut process_folds: Vec<(ProcessIdentity, Rect)> = Vec::new();
    let mut details_content = Rect::default();
    let mut details_viewport = 0u16;
    let mut details_rows = 0usize;
    if let (Some(detail_area), Some(page_lines)) = (detail_area, details.as_ref()) {
        let block = Block::bordered()
            .border_style(Style::new().fg(panel_border(app.details_focused())))
            .title("Details");
        let inner = block.inner(detail_area);
        frame.render_widget(block, detail_area);
        let (tabs, tab_rows) = draw_page_tabs(frame, inner, app.detail_page());
        detail_tabs = tabs;
        let content = Rect {
            y: inner.y.saturating_add(tab_rows),
            height: inner.height.saturating_sub(tab_rows),
            ..inner
        };
        details_content = content;
        details_viewport = content.height;
        let (rows, line_rows) = page_rows(&page_lines.lines, content.width);
        details_rows = rows;
        // The page scrolls in the rows it is drawn over, not in its lines: a line
        // long enough to wrap takes several rows, and a clamp counted in lines
        // stops with the last of them past the end of the panel. A page that has
        // shrunk since its last draw can also hold a scroll past its end, so the
        // offset is clamped here as well as where the keys move it.
        let offset = rows.saturating_sub(content.height as usize);
        let offset = (app.details_scroll() as usize).min(offset);
        // A page is bounded by the facts it lists, so its rows are within a
        // terminal's own coordinate space; one that somehow wrapped further
        // scrolls to the last addressable row rather than anywhere else.
        let offset = u16::try_from(offset).unwrap_or(u16::MAX);
        frame.render_widget(
            Paragraph::new(page_lines.lines.clone())
                .wrap(PAGE_WRAP)
                .scroll((offset, 0)),
            content,
        );
        // Where each block's marker is drawn: the row its line starts on, less
        // the rows the scroll has taken off the top. A marker the scroll or the
        // end of the panel has taken off the screen is drawn nowhere, and is not
        // a click target, so nothing answers to a glyph that is not there.
        for (key, index) in &page_lines.markers {
            let Some(row) = line_rows
                .get(*index)
                .and_then(|row| row.checked_sub(offset as usize))
            else {
                continue;
            };
            if row >= content.height as usize {
                continue;
            }
            disclosure_markers.push((
                key.clone(),
                Rect {
                    x: content.x,
                    y: content.y + row as u16,
                    // The marker is drawn as a two-cell affordance, so the whole
                    // of it is the pointer's target.
                    width: 2.min(content.width),
                    height: 1,
                },
            ));
        }
        // Where each process row is drawn, and where the cell that folds it is.
        // The row's place in the page is recorded whether or not the scroll has
        // left it on screen: the keyboard's cursor is scrolled to a row the panel
        // is not showing, and only a row it is showing is a pointer's target.
        for (identity, index, foldable) in &page_lines.process_rows {
            let page_row = line_rows.get(*index).copied();
            let drawn = page_row
                .and_then(|row| row.checked_sub(offset as usize))
                .filter(|row| *row < content.height as usize)
                .map(|row| Rect {
                    x: content.x,
                    y: content.y + row as u16,
                    width: content.width,
                    height: 1,
                });
            process_rows.push((identity.clone(), page_row.unwrap_or(0), drawn));
            if *foldable && let Some(rect) = drawn {
                // The fold marker is the row's own first cell, and the whole of
                // it is the target.
                process_folds.push((
                    identity.clone(),
                    Rect {
                        width: TABLE_MARKER.min(content.width as usize) as u16,
                        ..rect
                    },
                ));
            }
        }
    }

    let (confirm_cancel, confirm_confirm) = draw_confirmation(frame, app);

    Geometry {
        tree_panel: tree_area,
        tree_content,
        details: detail_area,
        details_content,
        details_rows,
        details_viewport,
        detail_tabs,
        disclosure_markers,
        process_rows,
        process_folds,
        offset: list_state.offset(),
        confirm_cancel,
        confirm_confirm,
    }
}

/// How the details panel wraps a page's lines. One place for it, so the rows a
/// page is scrolled by are measured with the same wrapping it is drawn with.
const PAGE_WRAP: Wrap = Wrap { trim: false };

/// Where a page's lines sit once the panel has wrapped them at `width`: the rows
/// the whole page occupies, and the row each line starts on.
///
/// The paragraph widget is asked for the count, on the same inner width and with
/// the same wrapping the page is drawn with, rather than the rows being counted
/// here: the panel scrolls in rows a reader can see, and a count that disagreed
/// with the widget would leave the last of them past the end of the page. The
/// widget wraps each line on its own, so a line starts at the rows of the lines
/// above it and its own are the ones it takes.
fn page_rows(lines: &[Line<'static>], width: u16) -> (usize, Vec<usize>) {
    let mut offsets = Vec::with_capacity(lines.len());
    let mut row = 0;
    for line in lines {
        offsets.push(row);
        row += Paragraph::new(vec![line.clone()])
            .wrap(PAGE_WRAP)
            .line_count(width);
    }
    (row, offsets)
}

/// The ink a panel's frame is drawn in. The focused panel takes the heading
/// role, so which panel answers the keyboard is visible before reading a word
/// of either.
fn panel_border(focused: bool) -> ratatui::style::Color {
    if focused {
        theme::palette().heading
    } else {
        theme::palette().border
    }
}

/// Draws the page tabs across the top of the details, and says which row of the
/// panel the page's own content starts on.
///
/// The tabs are drawn apart from the content so that scrolling a page does not
/// scroll its navigation away, and over as many rows as the panel is narrow, so
/// a stacked panel keeps every page selectable. Each drawn tab's rectangle goes
/// back to the caller, so a click is answered by the tab actually drawn.
fn draw_page_tabs(
    frame: &mut Frame,
    area: Rect,
    active: DetailPage,
) -> ([Option<Rect>; DetailPage::COUNT], u16) {
    let palette = theme::palette();
    let mut tabs = [None; DetailPage::COUNT];
    let mut x = area.x;
    let mut row = 0u16;
    for page in DetailPage::ALL {
        let width = tab_width(page);
        if x > area.x && x + width > area.right() {
            row += 1;
            x = area.x;
        }
        if area.y + row >= area.bottom() {
            // No room left in the panel: the page keeps its place in the cycle
            // (`←`/`→`) rather than being drawn outside the frame.
            break;
        }
        let rect = Rect {
            x,
            y: area.y + row,
            width,
            height: 1,
        };
        let style = if page == active {
            Style::new()
                .bg(palette.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(palette.subtle)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} ", page.label()),
                style,
            ))),
            rect,
        );
        tabs[page.index()] = Some(rect);
        x += width;
    }
    (tabs, row + 1)
}

/// How wide one page's tab is drawn: its label and one space either side, so
/// the whole tab is a pointer target.
fn tab_width(page: DetailPage) -> u16 {
    page.label().chars().count() as u16 + 2
}

/// How many rows the page tabs need in a panel whose content is `width` cells
/// wide. The layout is the one [`draw_page_tabs`] performs, told in advance: the
/// stacked panel has to reserve these rows before it knows it needs them.
fn tab_rows(width: u16) -> u16 {
    let width = width.max(1);
    let mut rows = 1u16;
    let mut x = 0u16;
    for page in DetailPage::ALL {
        let tab = tab_width(page);
        if x > 0 && x + tab > width {
            rows += 1;
            x = 0;
        }
        x += tab;
    }
    rows
}

/// The lifecycle confirmation, drawn over everything else. Returns the two
/// button rectangles so a pointer can be mapped back to the buttons actually
/// drawn, and nothing else on screen is clickable while it is up.
fn draw_confirmation(frame: &mut Frame, app: &App) -> (Option<Rect>, Option<Rect>) {
    let Some(confirmation) = app.confirmation() else {
        return (None, None);
    };
    let palette = theme::palette();
    let screen = frame.area();
    let title = match confirmation.operation {
        Operation::ClosePane => "Close pane",
        Operation::CloseTab => "Close tab",
        Operation::Restart => "Restart worker",
    };
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled(
                title.to_string(),
                Style::new()
                    .fg(palette.heading)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                sanitize(&confirmation.target.description()),
                Style::new().fg(palette.subtle),
            ),
        ]),
        Line::from(Span::styled(
            "This may lose:",
            Style::new().fg(palette.subtle),
        )),
    ];
    for loss in &confirmation.losses {
        lines.push(Line::from(Span::styled(
            format!("  {}", sanitize(loss)),
            Style::new().fg(palette.muted),
        )));
    }
    lines.push(Line::raw(""));
    let buttons_row = lines.len();
    // Cancel first, so the destructive button is not under the pointer each
    // time the dialog opens, and the default selection is Cancel.
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("[ Cancel ]", button_style(!confirmation.confirm_selected)),
        Span::raw("   "),
        Span::styled("[ Confirm ]", button_style(confirmation.confirm_selected)),
    ]));

    let inner_width = lines.iter().map(Line::width).max().unwrap_or(16) as u16;
    let popup = centered(
        screen,
        (inner_width + 4).min(screen.width),
        (lines.len() as u16 + 2).min(screen.height),
    );
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::new().fg(palette.border))
        .title("Confirm");
    let inner = block.inner(popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);

    // The buttons line is `  [ Cancel ]   [ Confirm ]`; the offsets are those
    // literal spans, so the hit test and the draw read one layout.
    let y = inner.y + buttons_row as u16;
    let cancel = Rect {
        x: inner.x + 2,
        y,
        width: 10,
        height: 1,
    };
    let confirm = Rect {
        x: inner.x + 2 + 10 + 3,
        y,
        width: 11,
        height: 1,
    };
    (Some(cancel), Some(confirm))
}

/// A drawn button's ink: the selected one is reversed so it reads as pressed,
/// the other recedes to second-rank ink.
fn button_style(selected: bool) -> Style {
    if selected {
        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::new().fg(theme::palette().subtle)
    }
}

/// A rectangle of at most `width` by `height`, centred in `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
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
        let width = detail_panel_width(body.width);
        let [tree, detail_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(width)]).areas(body);
        return (tree, Some(detail_area));
    }

    // Stacked: as many rows as the content needs, counting the rows a long
    // identity will actually wrap onto and the rows the page tabs take, and
    // never more than half the body.
    let columns = detail_content_width(body.width) as u32;
    let rows: u16 = detail
        .iter()
        .map(|line| (line.width() as u32).div_ceil(columns).max(1) as u16)
        .sum();
    let rows = rows.saturating_add(tab_rows(body.width.saturating_sub(2)));
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

/// The panel width the details are drawn in, for a body of this width: beside
/// the tree it is a clamped third of the body, and below it the whole body.
fn detail_panel_width(body_width: u16) -> u16 {
    if body_width >= SIDE_BY_SIDE_MIN_WIDTH {
        (body_width / 3).clamp(DETAIL_MIN_WIDTH, DETAIL_MAX_WIDTH)
    } else {
        body_width
    }
}

/// The cells the details' own content is drawn in: the panel, less its borders.
/// The page is wrapped to this before it is laid out, so a fact the panel cannot
/// hold on one line is continued under its own column rather than re-wrapped by
/// the drawing under the label.
fn detail_content_width(body_width: u16) -> u16 {
    detail_panel_width(body_width).saturating_sub(2).max(1)
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
    if app.confirmation().is_some() {
        let pairs = vec![
            ("tab", "select".to_string()),
            ("Enter", "confirm".to_string()),
            ("Esc", "cancel".to_string()),
        ];
        return wrap_hints(&pairs, width);
    }
    // Lifecycle outcomes are kept until dismissed and shown here, apart from the
    // source freshness drawn with the tree: an owner's answer says nothing about
    // whether the fleet is being observed.
    let mut action_lines: Vec<Line<'static>> = Vec::new();
    if let Some(message) = app.focus_message() {
        action_lines.push(Line::from(Span::styled(
            sanitize(message),
            Style::new().fg(palette.failed),
        )));
    }
    let notices = app.lifecycle_notices();
    let shown = notices
        .iter()
        .rev()
        .take(2usize.saturating_sub(action_lines.len()));
    let count = 2usize.saturating_sub(action_lines.len()).min(notices.len());
    for (index, notice) in shown.enumerate() {
        let style = if notice.failed {
            Style::new().fg(palette.failed)
        } else {
            Style::new().fg(palette.done)
        };
        let mut spans = vec![Span::styled(sanitize(&notice.text), style)];
        if index + 1 == count {
            spans.push(Span::styled(
                "  ·  c dismiss",
                Style::new().fg(palette.muted),
            ));
        }
        action_lines.push(Line::from(spans));
    }
    if !action_lines.is_empty() {
        return action_lines;
    }
    if app.is_filter_editing() {
        let pairs = vec![
            ("type", "filter".to_string()),
            ("Enter", "keep".to_string()),
            ("Esc", "clear".to_string()),
        ];
        return wrap_hints(&pairs, width);
    }
    // The panel that has the keyboard says so by naming its own keys: the
    // details answer to a different set from the tree, and a reader who has just
    // tabbed into them needs that set rather than the tree's.
    if app.details_focused() {
        let pairs = vec![
            ("j/k", "scroll".to_string()),
            ("spc/Enter", "open".to_string()),
            ("←/→", "page".to_string()),
            ("tab", "tree".to_string()),
            ("Esc", "tree".to_string()),
            ("d", "hide".to_string()),
        ];
        return wrap_hints(&pairs, width);
    }

    // The keys in the order a reader needs them. Full wording first; a terminal
    // that cannot hold it in two lines gets the terse wording instead, so the
    // state words are the first thing to go rather than the last keys.
    let tasks = task_rows_label(app.shows_tasks());
    let full = vec![
        ("j/k", String::new()),
        ("spc", "fold".to_string()),
        ("Enter", "focus".to_string()),
        ("/", "filter".to_string()),
        ("n/N", "next".to_string()),
        ("w/W", "work".to_string()),
        ("x/X", "close pane/tab".to_string()),
        ("s", app.order().label().to_string()),
        ("p", app.pane_view().label().to_string()),
        ("d", toggle_label(app.shows_details(), "details")),
        ("e", toggle_label(app.shows_finished(), "finished")),
        ("b", tasks.clone()),
        ("q", "quit".to_string()),
    ];
    let terse = vec![
        ("j/k", String::new()),
        ("spc", "fold".to_string()),
        ("Enter", "focus".to_string()),
        ("/", "filter".to_string()),
        ("n", "next".to_string()),
        ("w", "work".to_string()),
        ("x/X", "close".to_string()),
        ("s", app.order().label().to_string()),
        ("p", app.pane_view().label().to_string()),
        ("d", "details".to_string()),
        ("e", "finished".to_string()),
        ("b", tasks),
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

/// The `b` hint names the action in both states rather than repeating the noun
/// when the rows are already hidden: unlike `e`, whether background tasks are
/// listed is a property of the view the footer is describing.
fn task_rows_label(shown: bool) -> String {
    if shown {
        "hide tasks".to_string()
    } else {
        "show tasks".to_string()
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

fn row_item(
    row: &VisibleRow<'_>,
    tick: usize,
    width: usize,
    source_current: bool,
) -> ListItem<'static> {
    let palette = theme::palette();
    let mut spans: Vec<Span<'static>> = Vec::new();
    // The span that gives way when the row is wider than the panel: the name,
    // so the state, age and model on its right stay readable.
    let mut flex: Option<usize> = None;
    let connector = Style::new().fg(palette.subtle);
    // The branch prefix: one two-cell column per ancestor whose line continues,
    // then this row's own connector where it is not a workspace root. The
    // markers below stay in one column whatever the prefix says.
    for continues in &row.connectors.continuation {
        spans.push(Span::styled(
            if *continues { "│ " } else { "  " },
            connector,
        ));
    }
    if row.depth > 0 {
        spans.push(Span::styled(
            if row.connectors.has_following_sibling {
                "├─"
            } else {
                "└─"
            },
            connector,
        ));
    }
    spans.push(Span::styled(fold_marker(row), connector));
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
            // A stale mark only where the facts are current and a live process
            // was compared: a retained row has no process, and a stale source
            // must not present its last-good facts as freshly read.
            if source_current
                && !retained
                && agent
                    .binary()
                    .is_some_and(|binary| binary.freshness == BinaryFreshness::Stale)
            {
                spans.push(Span::styled(
                    format!("{} ", theme::stale_mark()),
                    Style::new().fg(palette.stale),
                ));
            }
            // The row's own text carries the state too, with weight spent on
            // the lifecycle that is moving: colour is what a glance reads, and
            // the mark alone is one cell of it.
            let title_style = theme::agent_text(&agent.state, name, retained);
            flex = Some(spans.len());
            spans.push(Span::styled(sanitize(&agent.title), title_style));
            // Routine states are carried by their marks; exceptional states
            // keep a word. Missing facts leave no dangling separators. The
            // background badge counts the row's own projected tasks — the same
            // rows drawn beneath it — never what is merely still running, and
            // never a source the projection has already replaced.
            let unresolved = agent.tasks.tasks.len();
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
                    // Only a task its source reports alive now moves; a
                    // last-observed phase word is stated and stays still.
                    let moving = agent.tasks.tasks.iter().any(TaskRow::is_running);
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
        RowKind::Task(task) => {
            // A task leaf: the published command where there is one and its id
            // otherwise, then the phase word as published, the age its start
            // time implies and the program's own mark. A process the publisher
            // reports alive moves in the configured command frames; a fact that
            // is only last-observed is stated and stays still.
            let published = task.published.as_ref();
            let moving = task.is_running() && theme::command_frames().is_some();
            let (mark, ink) = match theme::command_frames().filter(|_| moving) {
                Some(frames) => (theme::frame(frames, tick).to_string(), palette.working),
                None => (theme::pane_mark().to_string(), palette.subtle),
            };
            spans.push(Span::styled(format!("{mark} "), Style::new().fg(ink)));
            // The program mark is the same configured table a pane row reads;
            // a task with no published command has no program to mark.
            if let Some(identity) = published
                .and_then(|published| published.command.as_deref())
                .and_then(|command| theme::process_mark(program_of(command)))
            {
                spans.push(Span::styled(
                    format!("{identity} "),
                    Style::new().fg(palette.subtle),
                ));
            }
            flex = Some(spans.len());
            spans.push(Span::styled(
                sanitize(row.node.row.title()),
                Style::new().fg(palette.muted),
            ));
            if let Some(phase) = task.phase.as_deref() {
                spans.push(Span::styled(
                    format!(" · {}", sanitize(phase)),
                    Style::new().fg(palette.subtle),
                ));
            }
            // An age only where the publisher reports a start: absent is not a
            // zero, and a `tokens` task has neither.
            if let Some(started) = published.and_then(|published| published.started_at) {
                spans.push(Span::styled(
                    format!(" · {}", duration(elapsed(started, now_unix_ms()))),
                    Style::new().fg(palette.subtle),
                ));
            }
            // Where the facts came from is part of the row: the same id can be
            // a live publisher's task or the pane's last-observed report of it.
            if task.source == TaskSource::Tokens {
                spans.push(Span::styled(
                    format!(" · {}", task.basis()),
                    Style::new().fg(palette.retained),
                ));
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

/// The verdict a freshness comparison has to give, where it has one to give.
///
/// A current comparison has none: the package line above names what is running,
/// and a stale mark's absence is what says it is the installed one. A
/// deliberate other build says what it is and stops there — naming both roots
/// on a line a reader scans is what the block is for. A comparison that could
/// not be made gives its reason in a word; the sentence is the block's.
fn verdict_line(identity: &BinaryIdentity) -> Option<Line<'static>> {
    match identity.freshness {
        BinaryFreshness::Current if identity.running == identity.installed => None,
        // One side alone cannot be compared with anything, so nothing is
        // claimed either way.
        BinaryFreshness::Current if identity.running.is_none() || identity.installed.is_none() => {
            None
        }
        BinaryFreshness::Current => Some(field("binary", "other build".to_string())),
        // A comparison nobody attempted carries no reason, and a verdict with
        // no reason to give is not a verdict.
        BinaryFreshness::Unknown => identity
            .unknown
            .map(|reason| field("binary", unknown_word(reason).to_string())),
        BinaryFreshness::Stale => {
            let palette = theme::palette();
            let mut spans = vec![Span::styled(
                "binary: ".to_string(),
                Style::new().fg(palette.subtle),
            )];
            spans.push(Span::styled(
                format!("{} (stale)", theme::stale_mark()),
                Style::new().fg(palette.stale),
            ));
            // The installed target, named as compactly as the package line
            // names the running one. A running file the kernel marked deleted
            // has no counterpart to name, and the mark stands alone.
            if let Some(installed) = identity.installed.as_deref() {
                spans.push(Span::raw(format!(
                    " installed {}",
                    package_text(installed, identity.running.as_deref())
                )));
            }
            Some(Line::from(spans))
        }
    }
}

/// Where the store keeps its packages. A root under it is a hash and a name;
/// anywhere else a path is its own identity.
const STORE: &str = "/nix/store/";

/// How much of a store hash the page shows: enough to tell two builds of one
/// version apart in a panel, far short of the 32 characters that name the same
/// root twice.
const SHORT_HASH: usize = 7;

/// A store root's hash and package name, for `/nix/store` paths only.
fn store_root(path: &str) -> Option<(&str, &str)> {
    let (hash, name) = path.strip_prefix(STORE)?.split_once('-')?;
    (!hash.is_empty() && !name.is_empty()).then_some((hash, name))
}

/// The first [`SHORT_HASH`] characters of a store hash, cut on a character
/// boundary so a path this machine did not write cannot split a character.
fn short_hash(hash: &str) -> String {
    hash.chars().take(SHORT_HASH).collect()
}

/// A package as one short phrase: its name and version, and a short hash of its
/// store root where two builds of that same version have to be told apart.
///
/// Two versions name themselves, and one installation compared with itself has
/// only one build to name; two builds of one version are the same words over
/// different bits, and the hash is the only thing that tells a reader which of
/// them is running. A path outside the store has no version to prefer and is
/// named whole — it is short, and it is what the runtime was given.
fn package_text(root: &str, other: Option<&str>) -> String {
    let Some((hash, name)) = store_root(root) else {
        return sanitize(root);
    };
    match other.and_then(store_root) {
        Some((other_hash, other_name)) if other_name == name && other_hash != hash => {
            format!("{} {}", sanitize(name), short_hash(hash))
        }
        _ => sanitize(name),
    }
}

/// A path inside a root, as it is drawn: the root is named once, by the package
/// line, and repeating it under every file says nothing.
///
/// A path only sharing a prefix with the root — another package beside it — is
/// not inside it, and a path that *is* the root has nothing relative to show.
fn relative_to<'a>(path: &'a str, root: Option<&str>) -> Option<&'a str> {
    let rest = path.strip_prefix(root?)?;
    if rest.is_empty() {
        return None;
    }
    rest.strip_prefix('/')
}

/// The same reason in a word or two, for the line a reader scans: the sentence
/// it is cut from is the block's.
fn unknown_word(reason: BinaryUnknown) -> &'static str {
    match reason {
        BinaryUnknown::NoCounterpart => "no counterpart",
        BinaryUnknown::UnsupportedLauncher => "payload unknown",
        BinaryUnknown::Unreadable => "unreadable",
        BinaryUnknown::NotCompared => "not the named program",
    }
}

/// Why nothing was compared, in the page's words. Each reason is a different
/// thing to go and look at.
fn unknown_reason(reason: BinaryUnknown) -> &'static str {
    match reason {
        BinaryUnknown::NoCounterpart => "no installed counterpart resolves",
        BinaryUnknown::UnsupportedLauncher => {
            "the installed counterpart's payload could not be identified"
        }
        BinaryUnknown::Unreadable => "the running executable could not be read",
        BinaryUnknown::NotCompared => "the running file is not the program the runtime named",
    }
}

/// What a descendant sum is, said on the page that draws one: the processes the
/// kernel reports beneath a root, which is neither who owns them nor what work
/// they serve — and a resident set that counts a page shared by two processes
/// in each of them.
const DESCENDANT_SUM: &str = "observed processes beneath this one, not a workload or assignment total \
     (a page shared with another process counts in each)";

/// The measurements of one process: what the kernel says it is doing and what it
/// is using now.
///
/// The values are whoever the page is describing — the row's own foreground, or
/// the descendant the reader selected — and nothing here is inherited from an
/// owner, a session or a background task: a task's published PID arrives without
/// a birth identity and is not sampled at all. A value this machine could not
/// measure says so with its reason rather than as a zero, and a measured idle
/// interval is drawn as the zero it is.
fn process_section(
    state: ProcessState,
    cpu: Option<CpuPercent>,
    rss_bytes: Option<u64>,
) -> Vec<Line<'static>> {
    let (cpu, cpu_reason) = measured_value(
        cpu_value(cpu),
        "no interval of this process has been measured",
    );
    let (rss, rss_reason) = measured_value(
        rss_value(rss_bytes),
        "the kernel's page count could not be converted",
    );
    metric_section(
        "process",
        &[
            vec![Metric::stated("state", state_word(state))],
            vec![
                Metric::measured("cpu", cpu, cpu_reason),
                Metric::measured("rss", rss, rss_reason),
            ],
        ],
    )
}

/// The qualified sums of what was observed beneath a root: how many processes,
/// and what they are using between them.
///
/// The root's own figures are drawn apart from these and never added to them: a
/// build under a pane is the build's CPU, not the pane's. A sum that could not
/// cover every member is drawn as the lower bound it is, with the reason it is
/// one, and a root nothing could be enumerated beneath draws that as unavailable
/// rather than as a measured zero.
fn descendant_section(resources: &ProcessResources) -> Vec<Line<'static>> {
    let descendants = &resources.descendants;
    let (count, count_reason) = measured_value(
        descendant_count(descendants.observed),
        "nothing beneath this process could be enumerated",
    );
    let (total_cpu, total_cpu_reason) = total_value(&descendants.cpu, |cpu| cpu_percent(*cpu));
    let (total_rss, total_rss_reason) = total_value(&descendants.rss_bytes, |bytes| size(*bytes));
    metric_section(
        "descendants",
        &[
            vec![Metric::measured("count", count, count_reason)],
            vec![
                Metric::measured("cpu", total_cpu, total_cpu_reason),
                Metric::measured("rss", total_rss, total_rss_reason),
            ],
        ],
    )
}

/// Cells between one fact of a metric section and the fact beside it, and
/// between a fact's label and its value.
const METRIC_FACT_GAP: usize = 2;
const METRIC_LABEL_GAP: usize = 2;

/// The narrowest panel a metric section is drawn in on the side-by-side layout.
///
/// Two facts share a line only when both fit inside that width. A row that had
/// to wrap leaves the second fact's label at the end of the line above its own
/// value, and a label read apart from its number is worse than a fact on a line
/// of its own. The panel's real width is not known here — the page is built
/// before it is laid out — so the width the layout can never go below is the
/// one a pair has to fit.
const METRIC_PAIR_WIDTH: usize = 28;

/// One fact of a compact metric section: its label, its value as the section
/// draws it, and the reason that value is not a measurement — or is only a bound
/// on one — when there is a reason to give.
struct Metric {
    label: &'static str,
    value: String,
    reason: Option<String>,
}

impl Metric {
    /// A fact drawn as it was read, with nothing qualifying it.
    fn stated(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            reason: None,
        }
    }

    /// A measured value that carries a reason: `unavailable` with why nothing
    /// was measured, or a lower bound with why it is one.
    fn measured(label: &'static str, value: String, reason: Option<String>) -> Self {
        Self {
            label,
            value,
            reason,
        }
    }
}

/// A value this machine may not have measured, with the reason it did not.
///
/// The value stays `unavailable` and the reason travels beside it, so nothing
/// measured can be silently dropped and nothing unmeasured can read as a zero.
fn measured_value(value: Option<String>, reason: &str) -> (String, Option<String>) {
    match value {
        Some(value) => (value, None),
        None => (unavailable(), Some(reason.to_string())),
    }
}

/// One metric section: a heading, then the facts given, at most two to a row.
///
/// Labels are as wide as the section's longest, so the values line up down the
/// section instead of drifting with the label above them. Two facts share a row
/// while both fit the narrowest panel a section is drawn in; a row that does not
/// gives each fact a line of its own, so no value is ever cut short and no label
/// is ever left on the line above its own number.
///
/// A value that could not be measured, or that covers only part of what it
/// names, keeps its place on the line and puts its reason on the next one,
/// aligned under the value it belongs to. A reader comparing two readings
/// compares the values, and a reason reads as a reason rather than as another
/// measurement. A fact that has a reason therefore never shares its line: the
/// reason belongs to one value, and a line drawing two of them has no way to say
/// which.
fn metric_section(heading_text: &str, rows: &[Vec<Metric>]) -> Vec<Line<'static>> {
    let palette = theme::palette();
    let mut lines = vec![heading(heading_text)];
    for row in rows {
        // A row's labels are as wide as its own longest, so a row of two facts
        // spends no more of the panel on its labels than it has to: the width a
        // pair saves is the width the panel is being spent on.
        let label_width = row
            .iter()
            .map(|metric| metric.label.chars().count())
            .max()
            .unwrap_or(0);
        let cell_width =
            |metric: &Metric| label_width + METRIC_LABEL_GAP + metric.value.chars().count();
        let paired = row.len() > 1
            && row.iter().all(|metric| metric.reason.is_none())
            && row.iter().map(cell_width).sum::<usize>() + METRIC_FACT_GAP * (row.len() - 1)
                <= METRIC_PAIR_WIDTH;
        let drawn: Vec<&[Metric]> = if paired {
            vec![row.as_slice()]
        } else {
            row.iter().map(std::slice::from_ref).collect()
        };
        let reason_column = " ".repeat(label_width + METRIC_LABEL_GAP);
        for facts in drawn {
            let mut spans = Vec::new();
            for (index, metric) in facts.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::raw(" ".repeat(METRIC_FACT_GAP)));
                }
                spans.push(Span::styled(
                    format!(
                        "{:<label_width$}{}",
                        metric.label,
                        " ".repeat(METRIC_LABEL_GAP)
                    ),
                    Style::new().fg(palette.subtle),
                ));
                spans.push(Span::raw(metric.value.clone()));
            }
            lines.push(Line::from(spans));
            for metric in facts {
                if let Some(reason) = &metric.reason {
                    lines.push(Line::from(vec![
                        Span::raw(reason_column.clone()),
                        Span::styled(reason.clone(), Style::new().fg(palette.muted)),
                    ]));
                }
            }
        }
    }
    lines
}

/// The scheduler state the kernel reported, as one short enumerable name.
///
/// It says only what the scheduler is doing with the process — a sleeping
/// process may be waiting on a socket or on nothing, and a zombie has already
/// exited — so it is the kernel's own state, never progress or a verdict on the
/// work underneath, and it carries no activity colour. The name is kept short
/// on purpose: this is the value that changes while a reader watches, and a
/// changing sentence reads as a changing claim.
fn state_word(state: ProcessState) -> String {
    match state {
        ProcessState::Running => "running".into(),
        ProcessState::Sleeping => "sleeping".into(),
        ProcessState::DiskSleep => "disk wait".into(),
        ProcessState::Stopped => "stopped".into(),
        ProcessState::TracingStop => "traced".into(),
        ProcessState::Zombie => "zombie".into(),
        ProcessState::Dead => "dead".into(),
        ProcessState::Idle => "idle".into(),
        ProcessState::Other(letter) => format!("unknown ({})", sanitize(&letter.to_string())),
    }
}

/// A measured interval as a section draws it: a percentage of one CPU, which
/// work on more than one can carry past 100.
///
/// The base is not repeated on every value: it is a property of the measure
/// rather than of one reading, and stating it beside each number is what the
/// section's width is spent on instead of the numbers.
fn cpu_percent(cpu: CpuPercent) -> String {
    format!("{:.1}%", cpu.hundredths() as f64 / 100.0)
}

/// Interval CPU, or nothing when this machine has no interval to measure.
///
/// Unavailable until two readings of the same incarnation make an interval. A
/// first reading, a counter that went backwards, no elapsed time and a failed
/// read all leave nothing to measure — which is not the same as zero, and is
/// not always the warm-up either. A measured idle interval is zero, and is
/// drawn as one.
fn cpu_value(cpu: Option<CpuPercent>) -> Option<String> {
    cpu.map(cpu_percent)
}

/// A process's resident set, or nothing when this machine cannot convert the
/// kernel's page count to a size.
fn rss_value(bytes: Option<u64>) -> Option<String> {
    bytes.map(size)
}

/// How many processes were observed beneath a root, as a section draws it: a
/// count, or nothing when none could be enumerated at all.
fn descendant_count(observed: Option<u32>) -> Option<String> {
    observed.map(|count| match count {
        1 => "1 process".to_string(),
        count => format!("{count} processes"),
    })
}

/// A total as a section draws it: its value, or why it is not one. A partial
/// total keeps what it does cover, marked as the lower bound it is, and carries
/// the reason it is incomplete; an unknown one is unavailable, with the reason
/// no total could be made.
fn total_value<T>(total: &Total<T>, show: impl Fn(&T) -> String) -> (String, Option<String>) {
    match total {
        Total::Complete(value) => (show(value), None),
        Total::Partial(value, reason) => (format!("≥{}", show(value)), Some(sanitize(reason))),
        Total::Unknown(reason) => (unavailable(), Some(sanitize(reason))),
    }
}

/// A byte count in the units a reader compares sizes in.
fn size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    match bytes {
        gib if gib >= GIB => format!("{:.1} GiB", gib as f64 / GIB as f64),
        mib if mib >= MIB => format!("{:.1} MiB", mib as f64 / MIB as f64),
        kib if kib >= KIB => format!("{:.1} KiB", kib as f64 / KIB as f64),
        bytes => format!("{bytes} B"),
    }
}

fn basis_label(basis: &RetentionBasis) -> &'static str {
    match basis {
        RetentionBasis::ShellForeground => "shell foreground",
        RetentionBasis::Unverified => "unverified",
    }
}

/// The process holding a pane's foreground, when the evidence names a
/// non-shell one: a shell, an inconclusive foreground or no evidence at all
/// leaves nothing running to name.
fn foreground_pid(foreground: Option<&ForegroundEvidence>) -> Option<i32> {
    match foreground? {
        ForegroundEvidence::NonShell { pid, .. } => Some(*pid),
        _ => None,
    }
}

/// Detail-panel content for the selected row's page.
///
/// Every page begins with the row's own title, so a page always says which row
/// it is describing; what follows is the facts that page is about. Splitting
/// them keeps identity, the live PID and the agent's activity in front of the
/// long published text, and leaves every fact the panel used to draw reachable
/// on some page.
/// The lines of one page, and where among them each openable block's summary
/// was drawn: what the render needs to report a click back to the block it
/// landed on.
#[derive(Default)]
struct PageLines {
    lines: Vec<Line<'static>>,
    markers: Vec<(Disclosure, usize)>,
    /// The process rows this page drew, in the order they were drawn: each row's
    /// birth identity, the line it was drawn on, and whether the table can fold
    /// its branch. The render turns these into the rectangles the pointer and the
    /// cursor work from; a row is its identity, never the line it landed on.
    process_rows: Vec<(ProcessIdentity, usize, bool)>,
}

impl PageLines {
    fn push(&mut self, line: Line<'static>) {
        self.lines.push(line);
    }

    fn extend(&mut self, lines: Vec<Line<'static>>) {
        self.lines.extend(lines);
    }

    /// Adds a block's summary line, and remembers where its marker went.
    fn push_block(&mut self, key: &Disclosure, summary: Line<'static>) {
        self.markers.push((key.clone(), self.lines.len()));
        self.lines.push(summary);
    }
}

/// The openable blocks of the page being drawn: which the page offers, which of
/// them are open, and which one the keyboard is on.
struct Blocks<'a> {
    targets: Vec<Disclosure>,
    highlighted: Option<Disclosure>,
    app: &'a App,
    width: usize,
}

impl<'a> Blocks<'a> {
    fn new(app: &'a App, width: usize) -> Self {
        Self {
            targets: app.disclosures(),
            highlighted: app.disclosure_target(),
            app,
            width,
        }
    }

    /// The cells this page's content is drawn in.
    fn content_width(&self) -> usize {
        self.width
    }

    /// Whether the page offers this block, and whether it is open. `None` is
    /// content with nothing to open, which is drawn as it always was.
    fn open(&self, key: &Disclosure) -> Option<bool> {
        self.targets
            .contains(key)
            .then(|| self.app.disclosure_open(key))
    }

    /// A block's marker and label with no facts beside them: for a block whose
    /// content is only ever its body.
    fn marker_line(&self, key: &Disclosure, open: Option<bool>, label: &str) -> Line<'static> {
        let mut spans = Vec::new();
        if let Some(open) = open {
            spans.push(self.marker(key, open));
        }
        spans.push(Span::styled(
            label.to_string(),
            Style::new().fg(theme::palette().subtle),
        ));
        Line::from(spans)
    }

    /// The block's marker: closed or open, in the ink the keyboard's target
    /// takes so the block Enter would open is the one the eye finds.
    fn marker(&self, key: &Disclosure, open: bool) -> Span<'static> {
        let glyph = if open { "▾ " } else { "▸ " };
        let ink = if self.highlighted.as_ref() == Some(key) {
            Style::new()
                .fg(theme::palette().heading)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::palette().subtle)
        };
        Span::styled(glyph, ink)
    }

    /// A block's summary line: its marker, its label and the facts that stay
    /// visible whether it is open or closed. `None` is content with nothing to
    /// open, which is drawn without a marker rather than with one that answers
    /// to nothing.
    fn summary(
        &self,
        key: &Disclosure,
        open: Option<bool>,
        label: &str,
        value: String,
    ) -> Line<'static> {
        let mut spans = Vec::new();
        if let Some(open) = open {
            spans.push(self.marker(key, open));
        }
        spans.push(Span::styled(
            format!("{label}: "),
            Style::new().fg(theme::palette().subtle),
        ));
        spans.push(Span::raw(value));
        Line::from(spans)
    }
}

/// The opening of a long text: enough to recognise it, short enough to leave the
/// panel to the facts around it.
const DIGEST_LIMIT: usize = 60;

fn digest(text: &str) -> String {
    if text.chars().count() <= DIGEST_LIMIT {
        return text.to_string();
    }
    let kept: String = text.chars().take(DIGEST_LIMIT).collect();
    format!("{kept}…")
}

/// The page's lines, and the block each marker among them belongs to.
///
/// `selection` is the row of the process table the reader has selected, when one
/// is: the processes page describes that descendant, and every other page draws
/// its own row's facts. See [`process_lines`].
fn detail_page_lines(
    state: &ObservationState,
    app: &App,
    page: DetailPage,
    width: u16,
    selection: Option<&ProcessIdentity>,
) -> PageLines {
    let mut page_lines = PageLines::default();
    let Some(row) = app.selected_row() else {
        page_lines.push(Line::from("no row selected"));
        return page_lines;
    };
    page_lines.push(Line::from(Span::styled(
        sanitize(row.node.row.title()),
        Style::new().add_modifier(Modifier::BOLD),
    )));
    let blocks = Blocks::new(app, width as usize);
    match page {
        DetailPage::Overview => overview_lines(state, &row, &blocks, &mut page_lines),
        DetailPage::Processes => process_lines(state, &row, &blocks, selection, &mut page_lines),
        DetailPage::Tasks => task_page_lines(&row, &blocks, &mut page_lines),
        DetailPage::Source => page_lines.extend(source_lines(state, &row)),
    }
    // The bus is a subsystem beside the source rather than part of it, so its
    // failure is written on every page: nothing here depends on it, and a
    // reader should not have to find a page to learn it is down.
    if let Some(line) = bus_diagnostic(app) {
        page_lines.push(line);
    }
    page_lines
}

/// Overview: what this row is, in the compact form: location, the live PID and
/// the agent's activity and model, with the published assignment and what it
/// awaits kept to a summary. The command, the age and the measurements of the
/// process are the Processes page's.
fn overview_lines(
    state: &ObservationState,
    row: &VisibleRow<'_>,
    blocks: &Blocks<'_>,
    page: &mut PageLines,
) {
    match &row.node.row.kind {
        RowKind::Workspace {
            workspace_id,
            label,
            number,
        } => {
            page.push(field("kind", "workspace".to_string()));
            page.push(field(
                "workspace",
                location_text(label.as_deref(), workspace_id),
            ));
            page.push(field(
                "number",
                number.map(|n| n.to_string()).unwrap_or_else(unavailable),
            ));
        }
        RowKind::Pane(pane) => {
            page.push(field(
                "kind",
                match pane.exited {
                    Some(_) => "pane (finished session)".to_string(),
                    None => "pane".to_string(),
                },
            ));
            page.push(field(
                "state",
                match &pane.exited {
                    Some(_) => "exited — no agent reported on this pane".to_string(),
                    None => unavailable(),
                },
            ));
            page.push(field(
                "location",
                location_line(
                    (pane.workspace_label.as_deref(), &pane.workspace_id),
                    (pane.tab_label.as_deref(), &pane.tab_id),
                    &pane.pane_id,
                ),
            ));
            if let Some(session) = &pane.exited {
                page.push(field("session", sanitize(&session.title)));
                // The row is the session now, so this is what the pane is
                // instead of it.
                page.push(field("pane now", sanitize(&pane.title)));
            }
            if let Some(line) = live_pid(state, pane.foreground.as_ref()) {
                page.push(line);
            }
            page.push(field("agent", unavailable()));
        }
        RowKind::Agent(agent) => {
            page.push(field(
                "kind",
                match &agent.retained {
                    Some(basis) => format!("agent (retained — {})", basis_label(basis)),
                    None => "agent (current)".to_string(),
                },
            ));
            page.push(field(
                "location",
                location_line(
                    (agent.workspace_label.as_deref(), &agent.workspace_id),
                    (agent.tab_label.as_deref(), &agent.tab_id),
                    &agent.pane_id,
                ),
            ));
            // A retained row has no live process, so it names no PID.
            if agent.retained.is_none()
                && let Some(line) = live_pid(state, agent.foreground.as_ref())
            {
                page.push(line);
            }
            page.push(field(
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
            // The row draws the state derived from the pane; how the owner
            // projects it, and the identities it published, are Source facts.
            page.push(field(
                "state",
                format!("{} (derived from the pane)", sanitize(agent.state.word())),
            ));
            page.push(match &agent.retained {
                Some(_) => field("status (last observed)", status),
                None => field("status", status),
            });
            activity_lines(&agent.facts, blocks, page);
        }
        RowKind::Task(task) => {
            page.push(field("kind", "background task".to_string()));
            page.push(field("task", sanitize(&task.id.id)));
            // A phase the pane or publisher reported is shown as reported; a
            // token entry naming only an id says nothing about a phase, so the
            // field is absent rather than drawn "unavailable".
            if let Some(phase) = task.phase.as_deref() {
                page.push(field("phase", sanitize(phase)));
            }
            page.push(field("source", task.basis().to_string()));
            page.push(field("owner", sanitize(&task.id.owner)));
            if let Some(session) = &task.id.session {
                page.push(field("session", sanitize(session)));
            }
        }
    }
}

/// The row of the process table the reader has selected, when one is: the page
/// describes its own row's foreground unless a descendant's birth identity is
/// given here.
///
/// The rows the scan confirmed decide whether the process that was selected is
/// still there, so a process that vanished, or a pid the kernel handed to
/// another process, falls back to the root instead of drawing a stranger's facts
/// as the reader's own. Task 3.1 owns the cursor that produces one; until then
/// the page is drawn with none.
fn selected_sample<'observation>(
    resources: &'observation ProcessResources,
    selection: Option<&ProcessIdentity>,
) -> Option<&'observation DescendantSample> {
    let selection = selection?;
    resources
        .descendants
        .members
        .iter()
        .find(|member| member.identity == *selection)
}

/// Processes: the process holding this row's location. A row whose process is
/// not currently observed says so rather than drawing last-good facts as live,
/// and a container row has no process to name.
///
/// The measurements come last, after the identity and command they belong to,
/// and they belong to this row's own process: a background task's published PID
/// is named without them, because a PID alone cannot authorise a reading of a
/// process that may have been replaced.
fn process_lines(
    state: &ObservationState,
    row: &VisibleRow<'_>,
    blocks: &Blocks<'_>,
    selection: Option<&ProcessIdentity>,
    page: &mut PageLines,
) {
    let current = matches!(state.source_freshness(), SourceFreshness::Current);
    match &row.node.row.kind {
        RowKind::Workspace { .. } => page.push(field(
            "process",
            "none — a workspace holds panes, not a process".to_string(),
        )),
        RowKind::Pane(_) => {
            if !current {
                page.push(withheld("the source is not current"));
                return;
            }
            observed_process_lines(state, row, blocks, selection, page);
        }
        RowKind::Agent(agent) => {
            let unavailable_reason = match (&agent.retained, current) {
                (Some(_), _) => Some("this row is retained, not currently observed"),
                (None, false) => Some("the source is not current"),
                (None, true) => None,
            };
            if let Some(reason) = unavailable_reason {
                page.push(withheld(reason));
                return;
            }
            observed_process_lines(state, row, blocks, selection, page);
        }
        RowKind::Task(task) => {
            let published = task.published.as_ref();
            page.push(match published.and_then(|published| published.pid) {
                Some(pid) => field("pid", pid.to_string()),
                None => field("pid", unavailable()),
            });
            page.push(field("source", task.basis().to_string()));
            // A task's PID arrives without the identity of the process it
            // names, so no sample can be attributed to it: the publisher's
            // captured-at-spawn birth identity is still outstanding.
            page.push(field(
                "metrics",
                "unavailable — the publisher sends no process birth identity".to_string(),
            ));
        }
    }
}

/// The body of a Processes page whose row has a process that is current: what
/// the page describes, the runtime's own facts about the foreground process, its
/// measurements and the qualified sums of what is beneath it.
///
/// The page describes its own row's foreground unless the reader has selected a
/// descendant, and then the selection's own sample names and measures it, above
/// the root's. The binary comparison is made from the root's executable and from
/// nothing else, so a page drawing a child draws it under a heading of the
/// root's own: a descendant's sample says nothing about which build the root
/// runs.
fn observed_process_lines(
    state: &ObservationState,
    row: &VisibleRow<'_>,
    blocks: &Blocks<'_>,
    selection: Option<&ProcessIdentity>,
    page: &mut PageLines,
) {
    let foreground = match &row.node.row.kind {
        RowKind::Pane(pane) => pane.foreground.as_ref(),
        RowKind::Agent(agent) => agent.foreground.as_ref(),
        _ => None,
    };
    // The name the runtime reports for the running process, and the sample the
    // scan took of it. The table's first row carries the name the `observed`
    // line above it does, so a reader reads one process in both.
    let (root_name, resources) = match foreground {
        Some(ForegroundEvidence::NonShell {
            name,
            command,
            local,
            ..
        }) => (
            observed_name(name.as_deref(), command.as_deref()),
            local.resources.as_ref(),
        ),
        // A shell, an inconclusive foreground and no evidence at all name no
        // process, and the identity lines below say which of them it is.
        _ => (unavailable(), None),
    };
    let selected = resources.and_then(|resources| selected_sample(resources, selection));
    if let Some(sample) = selected {
        // The reader's row, from its own sample and from nothing else: the
        // kernel's name for it, its own incarnation and the figures read for it.
        // Its name is the kernel's `comm`, never the arguments it was started
        // with and never a claim about what started it.
        page.push(field("pid", sample.identity.pid.to_string()));
        page.push(field("observed", sanitize(&sample.name)));
        page.extend(process_section(sample.state, sample.cpu, sample.rss_bytes));
        page.push(heading("foreground root"));
    }
    if let Some(line) = live_pid(state, foreground) {
        page.push(line);
    }
    // A live agent's running executable and the comparison made from it:
    // machine facts about the process, not the agent.
    identity_lines(foreground, blocks, page);
    if let RowKind::Pane(pane) = &row.node.row.kind {
        page.push(field("terminal", terminal_line(pane.terminal())));
        page.push(field(
            "running for",
            pane.running_for().map(duration).unwrap_or_else(unavailable),
        ));
    }
    match resources {
        Some(resources) => {
            // A picked child's page is drawn above the root's own: with the table
            // open, the root's own figures are its first row, and are not drawn a
            // second time here. The sums are not in the table and stay.
            if selected.is_none() || blocks.open(&Disclosure::ProcessTable) != Some(true) {
                page.extend(process_section(
                    resources.state,
                    resources.cpu,
                    resources.rss_bytes,
                ));
            }
            page.extend(descendant_section(resources));
        }
        None => page.push(field(
            "metrics",
            format!(
                "{} — this refresh took no sample of this process",
                unavailable()
            ),
        )),
    }
    process_table(row, &root_name, blocks, page);
}

/// Tasks: the background work this row knows about. The list, the pane's own
/// tokens and the publisher's facts stay together, so a count can be read
/// against the list it counts. A task's long text is behind its own disclosure,
/// with the identity and state it belongs to always drawn.
fn task_page_lines(row: &VisibleRow<'_>, blocks: &Blocks<'_>, page: &mut PageLines) {
    match &row.node.row.kind {
        RowKind::Agent(agent) => {
            // The pane's raw token list is drawn only while it is still this
            // row's task source: a matched publisher's list is authoritative,
            // and printing both would contradict the rows beneath this one.
            let bus_authoritative = matches!(agent.tasks.source, Some(TaskSource::Bus));
            let before = page.lines.len();
            task_lines(agent, blocks, page);
            page.extend(task_fact_lines(&agent.facts, !bus_authoritative));
            if page.lines.len() == before {
                page.push(field("tasks", "none published".to_string()));
            }
        }
        RowKind::Task(task) => {
            let key = Disclosure::Task(task.id.clone());
            let open = blocks.open(&key);
            let summary = blocks.summary(&key, open, "task", task_row_summary(task));
            match open {
                Some(_) => page.push_block(&key, summary),
                None => page.push(summary),
            }
            match task.published.as_ref() {
                // A publisher that sent no detail for the task is a fact of its
                // own, and the page names it rather than drawing nothing.
                None => page.push(field(
                    "tasks",
                    "none — the publisher sent no detail for this task".to_string(),
                )),
                Some(published) => {
                    let now = now_unix_ms();
                    let mut detail = Vec::new();
                    // A command and a directory are the two fields that wrap
                    // over the panel, so they are what the disclosure holds; the
                    // measures below are one line each and stay drawn, because an
                    // exit code behind a marker is not one a reader can compare
                    // down the list.
                    if open != Some(false) {
                        if let Some(command) = published.command.as_deref() {
                            detail.push(field("command", bound_text(command)));
                        }
                        if let Some(cwd) = published.cwd.as_deref() {
                            detail.push(field("cwd", bound_text(cwd)));
                        }
                    }
                    if let Some(started) = published.started_at {
                        detail.push(field(
                            "started",
                            format!("{} ago", duration(elapsed(started, now))),
                        ));
                    }
                    if let Some(last_output) = published.last_output_at {
                        detail.push(field(
                            "last output",
                            format!("{} ago", duration(elapsed(last_output, now))),
                        ));
                    }
                    if let Some(bytes) = published.output_bytes {
                        detail.push(field("output", format!("{bytes} B")));
                    }
                    if let Some(code) = published.exit_code {
                        detail.push(field("exit", code.to_string()));
                    }
                    page.extend(detail);
                }
            }
        }
        RowKind::Pane(_) => page.push(field(
            "tasks",
            "none — this pane reports no agent".to_string(),
        )),
        RowKind::Workspace { .. } => page.push(field(
            "tasks",
            "none — tasks are published per pane".to_string(),
        )),
    }
}

/// A task row's own line on the Tasks page: what the block is, so the fields
/// behind its disclosure have something to hang from.
fn task_row_summary(task: &TaskRow) -> String {
    match task.phase.as_deref() {
        Some(phase) => format!("{} · {}", sanitize(&task.id.id), sanitize(phase)),
        None => sanitize(&task.id.id),
    }
}

/// Source: how current the facts are, where this row's identity comes from, and
/// what the owner published about it — its projected state, the run, request and
/// ask it names, and the session behind them.
fn source_lines(state: &ObservationState, row: &VisibleRow<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![detail_freshness(state, row)];
    if let RowKind::Agent(agent) = &row.node.row.kind {
        if let Some(projected) = &agent.facts.state {
            lines.push(field(
                "projection",
                format!(
                    "{} (owner projection)",
                    sanitize(AgentState::from(projected).word())
                ),
            ));
        }
        lines.push(field(
            "identity",
            session_text(agent.session.as_ref()).unwrap_or_else(unavailable),
        ));
        lines.extend(owner_identity_lines(&agent.facts));
        // The lineage the tree placed this row by. A retained row keeps its
        // observation, so its link stays readable after the agent goes away.
        let observation = state
            .inventory()
            .and_then(|inventory| inventory.agent_on_pane(&agent.pane_id))
            .or_else(|| {
                state
                    .retained()
                    .get(agent.pane_id.as_str())
                    .map(|retained| &retained.observation)
            });
        if let Some(observation) = observation {
            lines.extend(lineage_lines(state, observation));
        }
    }
    lines
}

/// The line a page draws when the facts it holds are not this row's to claim
/// right now, so a withheld value is never read as an absent one.
fn withheld(reason: &str) -> Line<'static> {
    field("process", format!("withheld — {reason}"))
}

/// The live PID line for a row whose process was queried, when one is known.
///
/// A stale inventory's last-good evidence is not a process known to be running
/// now, so no PID is drawn from one.
fn live_pid(
    state: &ObservationState,
    foreground: Option<&ForegroundEvidence>,
) -> Option<Line<'static>> {
    if !matches!(state.source_freshness(), SourceFreshness::Current) {
        return None;
    }
    foreground_pid(foreground).map(|pid| field("pid", pid.to_string()))
}

/// The row's foreground: the process's own command name as the runtime reports
/// it, the executable the kernel is running, and how that executable compares
/// with the program installed now.
///
/// The name is the runtime's report of a running process — for a bolt pane the
/// exec-replaced payload — so nothing here says what the pane was started with,
/// and the name is never taken for a launcher or an alias. The process's
/// arguments are not drawn at all: a harness takes its prompt and system prompt
/// as arguments, and nothing the runtime reports tells a prompt from a flag. A
/// shell or an inconclusive answer is not a process to name, and is stated
/// instead.
///
/// The page draws this identity short — the package once, the executable inside
/// it, a verdict only where there is one — and holds the long halves of the same
/// facts behind the page's block: the roots a comparison was made between, the
/// whole path, the birth identity, what a state means and what a sum is. A row
/// with no process draws none of it and is offered no block.
fn identity_lines(
    foreground: Option<&ForegroundEvidence>,
    blocks: &Blocks<'_>,
    page: &mut PageLines,
) {
    let Some(ForegroundEvidence::NonShell {
        name,
        command,
        local,
        ..
    }) = foreground
    else {
        page.push(match foreground {
            Some(ForegroundEvidence::Shell) => field(
                "foreground",
                "shell — nothing in the foreground".to_string(),
            ),
            Some(ForegroundEvidence::Inconclusive) => {
                field("foreground", "unknown — PID fields disagree".to_string())
            }
            _ => field("foreground", unavailable()),
        });
        return;
    };
    page.push(field(
        "observed",
        observed_name(name.as_deref(), command.as_deref()),
    ));
    let binary = &local.binary;
    if let Some(running) = binary.running.as_deref() {
        page.push(field(
            "package",
            package_text(running, binary.installed.as_deref()),
        ));
    }
    // The executable within the package that carries it: the store root is
    // named by the line above, and its hash twice on one page is a hash a
    // reader has to compare with itself. A file outside that root is drawn
    // whole, and one that is the root says nothing the line above did not.
    if let Some(executable) = binary.executable.as_deref() {
        let executable = sanitize(executable);
        let running = binary.running.as_deref().map(sanitize);
        match relative_to(&executable, running.as_deref()) {
            Some(relative) => page.push(field("exe", relative.to_string())),
            None if Some(&executable) == running.as_ref() => {}
            None => page.push(field("exe", executable)),
        }
    }
    if let Some(verdict) = verdict_line(binary) {
        page.push(verdict);
    }
    let key = Disclosure::Process;
    if let Some(open) = blocks.open(&key) {
        page.push_block(&key, blocks.marker_line(&key, Some(open), "details"));
        if open {
            page.extend(process_detail_lines(foreground, blocks.content_width()));
        }
    }
}

/// The verbose half of a live process's identity, behind the page's block.
///
/// The two roots are the sides the comparison was made between, drawn whole so
/// the hash on the package line above can be read against the build it names.
/// The whole executable path is the one the relative form was cut from, and the
/// birth identity is what a sample belongs to: a PID alone is a number the
/// machine reuses, and the boot id and start ticks are what tell two of them
/// apart. The state letters travel as one word each, so what the kernel meant by
/// the one on the page is here, and a descendant sum is what it is only with
/// the sentence that says so.
fn process_detail_lines(
    foreground: Option<&ForegroundEvidence>,
    width: usize,
) -> Vec<Line<'static>> {
    let Some(ForegroundEvidence::NonShell { local, .. }) = foreground else {
        return Vec::new();
    };
    let binary = &local.binary;
    let running = binary.running.as_deref().map(sanitize);
    let installed = binary.installed.as_deref().map(sanitize);
    let mut rows = Vec::new();
    // The roots, and the prefix they share stated once: two whole store paths
    // repeat a store, a name and a version on one panel, and the part that says
    // which build is which is the part past the prefix.
    let mut stated: Option<String> = None;
    match (running.as_deref(), installed.as_deref()) {
        (Some(running), Some(installed)) if running == installed => {
            rows.push(BlockRow::new("root", running.to_string()));
        }
        (Some(running), Some(installed)) => match shared_prefix(running, installed) {
            Some(prefix) => {
                rows.push(BlockRow::new("store", prefix.clone()));
                rows.push(BlockRow::new(
                    "running",
                    running[prefix.len()..].to_string(),
                ));
                rows.push(BlockRow::new(
                    "installed",
                    installed[prefix.len()..].to_string(),
                ));
                stated = Some(prefix);
            }
            None => {
                rows.push(BlockRow::new("running", running.to_string()));
                rows.push(BlockRow::new("installed", installed.to_string()));
            }
        },
        (Some(running), None) => rows.push(BlockRow::new("running", running.to_string())),
        (None, Some(installed)) => rows.push(BlockRow::new("installed", installed.to_string())),
        (None, None) => {}
    }
    if let Some(executable) = binary.executable.as_deref() {
        let executable = sanitize(executable);
        let roots = [running.as_deref(), stated.as_deref()];
        let relative = roots
            .iter()
            .flatten()
            .find_map(|root| relative_to(&executable, Some(root)));
        // A file that *is* the root says nothing the root's own row did not.
        let is_a_root = roots
            .iter()
            .flatten()
            .any(|root| *root == executable.as_str());
        match (relative, is_a_root) {
            (Some(relative), _) => rows.push(BlockRow::new("exe", relative.to_string())),
            (None, true) => {}
            (None, false) => rows.push(BlockRow::new("exe", executable)),
        }
    }
    if binary.freshness == BinaryFreshness::Unknown
        && let Some(reason) = binary.unknown
    {
        rows.push(BlockRow::new("unknown", unknown_reason(reason).to_string()));
    }
    if let Some(resources) = local.resources.as_ref() {
        let identity = &resources.identity;
        rows.push(BlockRow::new(
            "birth",
            format!(
                "pid {} · start ticks {}",
                identity.pid, identity.start_ticks,
            ),
        ));
        // The boot id gets a row of its own: a UUID is longer than any column a
        // half-width panel gives a value, and it reads better hyphen by hyphen
        // than mashing three short facts together to make room for it.
        rows.push(BlockRow::new("boot", sanitize(&identity.boot_id)));
        // The meaning of the state this row is showing, named where the row's
        // own word is drawn: the eight other kernel states are the kernel's,
        // and a list of them described is a paragraph where a fact belongs.
        rows.push(BlockRow::new(
            "state",
            format!(
                "{} — {}",
                state_word(resources.state),
                state_meaning(resources.state)
            ),
        ));
    }
    rows.push(BlockRow::new("note", DESCENDANT_SUM.to_string()));
    block_lines(&rows, width)
}

/// One labelled row of a block: the label, and the value drawn after the column
/// every row of that block shares.
struct BlockRow {
    label: &'static str,
    text: String,
}

impl BlockRow {
    fn new(label: &'static str, text: String) -> Self {
        Self { label, text }
    }
}

/// Draws a block's rows: one label column, the value beside its label, and a
/// value too long for the panel continued under that same column rather than
/// under the label it belongs to.
fn block_lines(rows: &[BlockRow], width: usize) -> Vec<Line<'static>> {
    let column = rows.iter().map(|row| row.label.len()).max().unwrap_or(0);
    let indent = column + 1;
    let value_width = width.saturating_sub(indent).max(MIN_VALUE_COLUMN);
    let mut lines = Vec::new();
    for row in rows {
        for (index, part) in wrap_value(&row.text, value_width).into_iter().enumerate() {
            let lead = if index == 0 {
                format!("{:<column$} ", row.label)
            } else {
                " ".repeat(indent)
            };
            lines.push(Line::from(vec![
                Span::styled(lead, Style::new().fg(theme::palette().subtle)),
                Span::raw(part),
            ]));
        }
    }
    lines
}

/// The label of the block the process table is held behind, where the page's
/// other block is labelled `details` and says only that it holds more.
const TABLE_LABEL: &str = "process table";

/// The label while the reader is moving through the table's rows: the mode is
/// local to this page, and the page names it where the table is named.
const TABLE_LABEL_NAVIGATING: &str = "process table · navigating";

/// The cells the table keeps at the start of every row for its fold marker: the
/// glyph that says whether a branch is open or folded, and the room beside it —
/// the same two-cell affordance the page's other markers are clicked by. A leaf's
/// marker is blank, so every row's name starts in the same cell.
const TABLE_MARKER: usize = 2;

/// Cells between two columns of the process table.
const TABLE_GAP: usize = 2;

/// Cells one level of a branch takes in the name's own column: the guide that
/// marks it, and the room beside it.
const BRANCH_SEGMENT: usize = 3;

/// The narrowest name column the table draws before it gives up a metric column:
/// a branch guide and a few characters of a name. The name and the pid are what
/// identify a row and neither is given up; the metric columns go, widest first.
const TABLE_NAME_MIN: usize = 6;

/// The cell the table draws for a value this machine did not measure. A table
/// has one cell for a reading and no room for the eleven of `unavailable`: a
/// dash is not a number and not a zero, and the table says what it means below
/// the rows it drew one in.
const TABLE_UNMEASURED: &str = "—";

/// Draws the table: the columns it can name, one single-line row per process, and
/// what a dash means where one was drawn.
///
/// Every line is built to the panel's own width, so nothing here wraps into the
/// row below it: a name is shortened, and then the metric columns are dropped —
/// the resident set before the CPU — before a row is allowed past the panel's
/// edge. The branch guides are drawn inside the name's own column, so a deeper
/// tree spends the name's width rather than pushing the pid off the panel.
fn process_table_lines(
    table: &ProcessTable,
    root_name: &str,
    width: usize,
    selected: &ProcessIdentity,
    folded: &HashSet<ProcessIdentity>,
    page: &mut PageLines,
) {
    let subtle = Style::new().fg(theme::palette().subtle);
    let cpu = |row: &ProcessRow| {
        row.cpu
            .map(cpu_percent)
            .unwrap_or_else(|| TABLE_UNMEASURED.to_string())
    };
    let rss = |row: &ProcessRow| {
        row.rss_bytes
            .map(size)
            .unwrap_or_else(|| TABLE_UNMEASURED.to_string())
    };
    // Each column is as wide as the widest thing drawn in it, its own heading
    // included: a tree whose pids are shorter than `pid` still lines its rows up
    // under the heading.
    let widest = |values: Vec<String>, heading: &str| {
        values
            .iter()
            .map(|value| display_cells(value))
            .max()
            .unwrap_or(0)
            .max(display_cells(heading))
    };
    let pid_width = widest(
        table
            .rows
            .iter()
            .map(|row| row.identity.pid.to_string())
            .collect(),
        "pid",
    );
    let cpu_width = widest(table.rows.iter().map(&cpu).collect(), "cpu");
    let rss_width = widest(table.rows.iter().map(&rss).collect(), "rss");
    // What the columns right of the name cost, given which of them the panel is
    // paying for. A panel too narrow for all of them gives up the widest fact
    // first, and keeps the name and the pid whatever happens.
    let tail = |with_cpu: bool, with_rss: bool| {
        TABLE_MARKER
            + pid_width
            + TABLE_GAP
            + if with_cpu { cpu_width + TABLE_GAP } else { 0 }
            + if with_rss { rss_width + TABLE_GAP } else { 0 }
    };
    let show_rss = width >= TABLE_NAME_MIN + tail(true, true);
    let show_cpu = width >= TABLE_NAME_MIN + tail(true, show_rss);
    let name_width = width.saturating_sub(tail(show_cpu, show_rss));
    // A value in a column of its own, as wide as the column: the spaces that
    // are left of it are counted in cells, so the value ends in the same cell in
    // every row whatever it says.
    let column = |value: &str, width: usize| {
        format!(
            "{:pad$}{value}",
            "",
            pad = width.saturating_sub(display_cells(value))
        )
    };
    // One row, from the cells it draws. The name column is filled to its own
    // width, so every column starts in the same cell whatever the name's length
    // or the width of its characters. The marker column is the caller's: it is
    // paid for out of the row, before the name.
    let cells = |branch: usize, name: &str, pid: &str, cpu: Option<&str>, rss: Option<&str>| {
        let name = shorten(name, name_width.saturating_sub(branch));
        let mut text = name.clone();
        let pad = name_width
            .saturating_sub(branch)
            .saturating_sub(display_cells(&name));
        text.push_str(&" ".repeat(pad));
        text.push_str(&column(pid, pid_width + TABLE_GAP));
        if let Some(cpu) = cpu {
            text.push_str(&column(cpu, cpu_width + TABLE_GAP));
        }
        if let Some(rss) = rss {
            text.push_str(&column(rss, rss_width + TABLE_GAP));
        }
        text
    };
    page.push(Line::from(Span::styled(
        format!(
            "{:marker$}{}",
            "",
            cells(
                0,
                "name",
                "pid",
                show_cpu.then_some("cpu"),
                show_rss.then_some("rss"),
            ),
            marker = TABLE_MARKER
        ),
        subtle,
    )));
    for row in table.visible() {
        // A branch is drawn out of the name's own cells, and the levels nearest
        // the process are the ones that say where it sits: a column too narrow
        // for the whole ancestry drops the outer guides — saying so with an
        // ellipsis — rather than drawing the row flush with the root's own
        // children. Every guide is one cell, so the levels are counted in
        // characters and the branch kept is at most the name's own width.
        let mut branch = String::new();
        for guide in &row.guides {
            branch.push_str(if *guide { "│  " } else { "   " });
        }
        if row.parent.is_some() {
            branch.push_str(if row.last { "└─ " } else { "├─ " });
        }
        let branch = if display_cells(&branch) < name_width {
            branch
        } else if name_width == 0 {
            // No cell for the guides and none for the ellipsis that says they
            // were left out: the row's own marker cell is all it has.
            String::new()
        } else {
            let levels = name_width.saturating_sub(1) / BRANCH_SEGMENT;
            let dropped = branch.chars().count() - levels * BRANCH_SEGMENT;
            let kept: String = branch.chars().skip(dropped).collect();
            format!("…{kept}")
        };
        let branch_width = display_cells(&branch);
        // The marker column: the fold of this row's branch, or a blank cell where
        // the row has no branch to fold. Both say the same thing about a process
        // beneath it, and neither names a command.
        let marker = match row.has_children {
            true if folded.contains(&row.identity) => "▸ ",
            true => "▾ ",
            false => "  ",
        };
        // The row's own process is named by the runtime, and every process under
        // it by the kernel's own read: both are external text, sanitized here
        // like everything else drawn from outside.
        let name = sanitize(row.name.as_deref().unwrap_or(root_name));
        let pid = row.identity.pid.to_string();
        // The reader's row is a band across the table: it is marked by the cells
        // it fills, which no name column has to pay for.
        let ink = |base: Style| match row.identity == *selected {
            true => Style::new()
                .bg(theme::palette().selection)
                .add_modifier(Modifier::BOLD),
            false => base,
        };
        let index = page.lines.len();
        page.process_rows
            .push((row.identity.clone(), index, row.has_children));
        page.push(Line::from(vec![
            Span::styled(format!("{marker}{branch}"), ink(subtle)),
            Span::styled(
                cells(
                    branch_width,
                    &name,
                    &pid,
                    show_cpu.then(|| cpu(row)).as_deref(),
                    show_rss.then(|| rss(row)).as_deref(),
                ),
                ink(Style::new()),
            ),
        ]));
    }
    let unmeasured = table
        .visible()
        .any(|row| (show_cpu && row.cpu.is_none()) || (show_rss && row.rss_bytes.is_none()));
    if unmeasured {
        page.push(Line::from(Span::styled(
            format!("{TABLE_UNMEASURED} not measured"),
            subtle,
        )));
    }
}

/// The cells a piece of text takes on the terminal, as Ratatui measures it for
/// layout: the characters of a name are not cells, so a name with characters
/// wider than one — or marks that take none — is measured by what will be drawn.
/// Everything this table aligns is measured here, because a column laid out in
/// characters shifts every cell after it.
fn display_cells(text: &str) -> usize {
    Span::raw(text).width()
}

/// A name shortened to the cells a column gives it, keeping the front of it: a
/// process is recognised by how its name starts, and the pid beside it names the
/// row when the name is not enough. The ellipsis is paid for out of the column:
/// the characters kept are as many as leave room for it, and a character too
/// wide for what is left is dropped rather than cut.
fn shorten(name: &str, width: usize) -> String {
    if display_cells(name) <= width {
        return name.to_string();
    }
    if width == 0 {
        // Not even the ellipsis has a cell to sit in.
        return String::new();
    }
    let mut kept = String::new();
    for ch in name.chars() {
        kept.push(ch);
        if display_cells(&kept) + 1 > width {
            kept.pop();
            break;
        }
    }
    kept.push('…');
    kept
}

/// The process table's block, where the page has a verified tree to draw: the
/// root's own row and a row for every process the same scan confirmed beneath it.
///
/// The table is offered only where the scan that sampled the root also enumerated
/// what is beneath it: an unreadable or budgeted scan is not a tree of nothing,
/// and the sums above already say it is not a measurement.
fn process_table(row: &VisibleRow<'_>, root_name: &str, blocks: &Blocks<'_>, page: &mut PageLines) {
    let Some(table) = blocks.app.process_table(row) else {
        return;
    };
    let key = Disclosure::ProcessTable;
    let Some(open) = blocks.open(&key) else {
        return;
    };
    let label = match blocks.app.process_navigating() {
        true => TABLE_LABEL_NAVIGATING,
        false => TABLE_LABEL,
    };
    page.push_block(&key, blocks.marker_line(&key, Some(open), label));
    if open {
        let selected = blocks
            .app
            .process_selection()
            .unwrap_or(&table.root)
            .clone();
        process_table_lines(
            &table,
            root_name,
            blocks.content_width(),
            &selected,
            blocks.app.process_folded(),
            page,
        );
    }
}

/// The narrowest value column a block is wrapped to: past that, a fact is one
/// character per row and the wrapping costs more than the fact is worth.
const MIN_VALUE_COLUMN: usize = 8;

/// Wraps `text` to `width` cells, at a space where one is near enough and inside
/// a word only when the word is longer than the whole line. The pieces come back
/// without indentation; the caller draws them in its value column.
fn wrap_value(text: &str, width: usize) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            pieces.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() || pieces.is_empty() {
        pieces.push(current);
    }
    let mut lines = Vec::new();
    for piece in pieces {
        let mut rest = piece.as_str();
        loop {
            let count = rest.chars().count();
            if count <= width {
                lines.push(rest.to_string());
                break;
            }
            let split = split_word(rest, width);
            lines.push(rest[..split].to_string());
            rest = &rest[split..];
        }
    }
    lines
}

/// Where to cut a word that cannot fit the column whole: at the last separator
/// it can, so a store path breaks at a store and a boot id at a hyphen, and
/// inside a run of characters only when there is no separator to break at.
fn split_word(word: &str, width: usize) -> usize {
    let boundary = word
        .char_indices()
        .filter(|(index, character)| *index < width && matches!(character, '/' | '-'))
        .map(|(index, character)| index + character.len_utf8())
        .next_back();
    boundary.unwrap_or_else(|| {
        word.char_indices()
            .nth(width)
            .map(|(index, _)| index)
            .unwrap_or(word.len())
    })
}

/// The prefix two paths share down to a component boundary, when each leaves
/// something past it: `/nix/store/aaa-pi` and `/nix/store/bbb-pi` share
/// `/nix/store/`, where two sibling files share their directory and not their
/// name.
fn shared_prefix(a: &str, b: &str) -> Option<String> {
    let mut end = 0;
    for ((index, left), right) in a.char_indices().zip(b.chars()) {
        if left != right {
            break;
        }
        if left == '/' {
            end = index + 1;
        }
    }
    if end <= 1 || end >= a.len() || end >= b.len() {
        return None;
    }
    Some(a[..end].to_string())
}

/// A section heading: the word that says what the lines under it are.
fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new()
            .fg(theme::palette().heading)
            .add_modifier(Modifier::BOLD),
    ))
}

/// What the kernel's state letter means for the process that carries it, in the
/// page's words.
///
/// The letter says only what the scheduler is doing with the process: a sleeping
/// process may be waiting on a socket or on nothing, and a zombie has already
/// exited. One word of state is not readable without it.
fn state_meaning(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Running => "executing, or waiting its turn on a CPU",
        ProcessState::Sleeping => "waiting, but wakeable",
        ProcessState::DiskSleep => "blocked in the kernel, usually on I/O",
        ProcessState::Stopped => "stopped by a signal",
        ProcessState::TracingStop => "stopped because something is tracing it",
        ProcessState::Zombie => "exited, not yet reaped by its parent",
        ProcessState::Dead => "gone, or being torn down",
        ProcessState::Idle => "idle in the kernel, below the oldest run queue",
        ProcessState::Other(_) => "a state this kernel wrote that Radar does not name",
    }
}

/// The running process's command name as the runtime reports it, sanitized like
/// every other external text. A runtime that reports no name still reports the
/// command line, and its first word names the program the kernel is running.
fn observed_name(name: Option<&str>, command: Option<&str>) -> String {
    let name = name
        .map(str::to_owned)
        .or_else(|| command.map(program_of).map(str::to_owned))
        .unwrap_or_else(unavailable);
    sanitize(&name)
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

/// What the agent is doing, for Overview: the facts Herdsman publishes about
/// it, each drawn only while the source publishes it — a fact nobody reported
/// is absent here, never a placeholder. The assignment is the one field long
/// enough to bury the rest, so it is drawn behind this page's disclosure.
///
/// Every value is text another process wrote — an assignment's display text
/// above all — so each is sanitized like any other runtime string.
fn activity_lines(facts: &HerdsmanFacts, blocks: &Blocks<'_>, page: &mut PageLines) {
    if let Some(role) = facts.role.as_deref() {
        page.push(field("role", sanitize(role)));
    }
    // Herdr's own display agent is where a worker's agent definition comes
    // from; it earns a line only when it says more than the role already does.
    if let Some(definition) = facts.definition.as_deref()
        && Some(definition) != facts.role.as_deref()
    {
        page.push(field("definition", sanitize(definition)));
    }
    if let Some(assignment) = facts.assignment.as_deref() {
        let key = Disclosure::Assignment;
        match blocks.open(&key) {
            Some(open) => {
                let text = sanitize(assignment);
                let value = if open { text } else { digest(&text) };
                let summary = blocks.summary(&key, Some(open), "assignment", value);
                page.push_block(&key, summary);
            }
            None => page.push(field("assignment", sanitize(assignment))),
        }
    }
    if let Some(assigned) = facts.assigned_for {
        page.push(field("assigned for", duration(assigned)));
    }
    if let Some(awaiting) = facts.awaiting() {
        page.push(field("awaiting", sanitize(&awaiting)));
    }
    if let Some(model) = facts.model.as_deref() {
        page.push(field("model", sanitize(model)));
    }
    if let Some(provider) = facts.provider.as_deref() {
        page.push(field("provider", sanitize(provider)));
    }
    if let Some(thinking) = facts.thinking.as_deref() {
        page.push(field("thinking", sanitize(thinking)));
    }
    if let Some(usage) = facts.context_usage.as_deref() {
        page.push(field("context usage", sanitize(usage)));
    }
}

/// The pane's own published background facts, for Tasks.
///
/// `tokens_listed` is false where a matched publisher's list is authoritative:
/// printing both would contradict the rows beneath the owner.
fn task_fact_lines(facts: &HerdsmanFacts, tokens_listed: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if tokens_listed && !facts.background_tasks.is_empty() {
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
    lines
}

/// The identities the owner published for this row, for Source: what a reader
/// cross-checks against Herdsman's own records rather than against activity.
fn owner_identity_lines(facts: &HerdsmanFacts) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
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

/// This row's lineage, for Source: the session the owner published for it and the
/// parent it named, with what became of that link.
///
/// A worker that lands at a workspace root can be there because two panes
/// publish one session UUID, which makes its parent an ambiguity rather than an
/// owner. The tree does not fabricate a nesting, so the reason is said here
/// rather than left for a reader to infer from where the row landed.
fn lineage_lines(state: &ObservationState, agent: &AgentObservation) -> Vec<Line<'static>> {
    let Some(lineage) = agent.lineage.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![field("session uuid", sanitize(lineage.session.as_str()))];
    if let Some(parent) = lineage.parent.as_ref() {
        lines.push(field("parent", parent_link(state, agent, parent)));
    }
    lines
}

/// What became of a parent link: the row it is nested under, or the reason the
/// tree left it where it is. The rule is the tree's own — one claimant in the
/// workspace, and never the row itself.
fn parent_link(state: &ObservationState, agent: &AgentObservation, parent: &SessionUuid) -> String {
    let uuid = sanitize(parent.as_str());
    let owners: Vec<&AgentObservation> = state
        .inventory()
        .map(|inventory| {
            inventory
                .agents
                .iter()
                .filter(|other| {
                    other.location.workspace_id == agent.location.workspace_id
                        && other
                            .lineage
                            .as_ref()
                            .is_some_and(|lineage| &lineage.session == parent)
                })
                .collect()
        })
        .unwrap_or_default();
    match owners.as_slice() {
        [] => format!("{uuid} — no row publishes this session, so this row is not nested"),
        [owner] if owner.location.pane_id == agent.location.pane_id => {
            format!("{uuid} — this row's own session, so it is not nested")
        }
        [owner] => format!("{uuid} — nested under {}", owner_name(owner)),
        owners => format!(
            "{uuid} — {} rows publish this session, so this row is not nested",
            owners.len()
        ),
    }
}

/// The name an owner is linked by: the label or name it published, and the pane
/// that carries it.
fn owner_name(agent: &AgentObservation) -> String {
    let name = agent
        .facts
        .label
        .as_deref()
        .or(agent.label.as_deref())
        .or(agent.facts.name.as_deref())
        .filter(|name| !name.is_empty());
    match name {
        Some(name) => format!("{} · {}", agent.location.pane_id, sanitize(name)),
        None => agent.location.pane_id.clone(),
    }
}

fn unavailable() -> String {
    "unavailable".to_string()
}

/// The bus tasks attached to this agent row, and the note a disagreement with
/// the pane tokens earns.
///
/// The task rows, the badge and this summary all read the row's one projection,
/// so they cannot disagree with each other: the pane's own ids where no
/// publisher matches the row, and the publisher's complete list — an empty one
/// included — where one does. What the publisher sent is drawn as sent.
fn task_lines(agent: &AgentRow, blocks: &Blocks<'_>, page: &mut PageLines) {
    let Some(TaskSource::Bus) = agent.tasks.source else {
        // The token fallback is the pane's own report: its ids and phases are
        // the rows beneath this one, and the `background` line above states
        // them as published. Nothing more is claimed about them.
        return;
    };

    if agent.tasks.tasks.is_empty() {
        // A published empty list and no publisher are different facts, and the
        // panel says which of the two it is.
        page.push(field("bus", "connected — no unresolved tasks".into()));
    } else {
        let now = now_unix_ms();
        for task in &agent.tasks.tasks {
            let Some(published) = task.published.as_ref() else {
                continue;
            };
            let key = Disclosure::Task(task.id.clone());
            let open = blocks.open(&key);
            let summary = blocks.summary(&key, open, "task", task_summary(published, now));
            match open {
                Some(_) => page.push_block(&key, summary),
                None => page.push(summary),
            }
            // The command and the directory are the lines that wrap over the
            // panel, so they are the ones the disclosure holds. The summary
            // above carries the identity, the state and the measures open or
            // closed, so nothing a reader compares across tasks moves.
            if open != Some(false) {
                if let Some(command) = published.command.as_deref() {
                    page.push(field("command", bound_text(command)));
                }
                if let Some(cwd) = published.cwd.as_deref() {
                    page.push(field("cwd", bound_text(cwd)));
                }
            }
        }
    }
    // The list outranks the count, so the count is never drawn in its place;
    // where the two disagree the panel states it instead of resolving it. The
    // token counts running tasks, so only running tasks are comparable with it.
    if let Some(reported) = agent.facts.background_running
        && reported as usize != agent.tasks.running()
    {
        page.push(field(
            "tokens",
            format!(
                "report {reported} running; the bus lists {}",
                agent.tasks.running()
            ),
        ));
    }
}

/// One task as the details draw it: its id, its state word as published, how
/// long it has run, how long ago it last produced output, how much it has
/// produced, and its exit code once there is one. A field the publisher did not
/// send draws nothing — absent is not a zero.
/// A published task's identity and the measures beside it, as one line's value:
/// the facts that never collapse.
fn task_summary(task: &Task, now_unix_ms: u64) -> String {
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
    parts.join(" · ")
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
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use crate::app::Action;
    use crate::herdr::decode_snapshot;
    use crate::model::{
        AgentObservation, BinaryFreshness, BinaryIdentity, BinaryUnknown, CpuPercent,
        DescendantResources, DescendantSample, FleetObservation, ForegroundEvidence, HerdsmanFacts,
        LocalFacts, Location, Pane, ProcessIdentity, ProcessResources, ProcessState, RuntimeStatus,
        SemanticState, Tab, TerminalMode, Total, Workspace,
    };
    use crate::runtime::Target;
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

    /// A draw of one named page: the panel keeps a fact on one page, and a test
    /// about that fact says which rather than relying on the default.
    fn render_page(
        state: &ObservationState,
        app: &mut App,
        page: DetailPage,
        width: u16,
        height: u16,
    ) -> String {
        let shown = app.detail_page();
        app.select_page(page);
        let screen = render_text(state, app, width, height);
        app.select_page(shown);
        screen
    }

    /// Every page in turn, concatenated: the facts a reader can reach, whichever
    /// page carries them.
    fn render_pages(state: &ObservationState, app: &mut App, width: u16, height: u16) -> String {
        DetailPage::ALL
            .into_iter()
            .map(|page| render_page(state, app, page, width, height))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One page's panel text, with the panel's own wrapping undone and the fleet
    /// column left out: a phrase is asserted as the page carries it rather than
    /// as one row of a narrow panel. The panel is still the one drawn, so a fact
    /// that never reaches it still fails.
    fn panel_text(state: &ObservationState, app: &mut App, page: DetailPage) -> String {
        panel_text_at(state, app, page, 120)
    }

    /// A block row read from the drawn panel: the label and the value that
    /// follows it, with the column's padding flattened out. A value the panel
    /// wrapped comes back as one string, because its rows are joined the same
    /// way.
    fn draws_row(page: &str, label: &str, value: &str) -> bool {
        let flat = page.split_whitespace().collect::<Vec<_>>().join(" ");
        flat.contains(&format!("{label} {value}"))
    }

    /// The same at a chosen terminal width: a value longer than the panel's own
    /// width wraps mid-token, and joining its rows would split it with a space.
    fn panel_text_at(
        state: &ObservationState,
        app: &mut App,
        page: DetailPage,
        width: u16,
    ) -> String {
        let shown = app.detail_page();
        app.select_page(page);
        let mut terminal =
            Terminal::new(TestBackend::new(width, 30)).expect("infallible test backend");
        let mut geometry = Geometry::default();
        terminal
            .draw(|frame| {
                geometry = render(frame, state, app, &mut ListState::default(), 0);
            })
            .expect("draw");
        app.note_layout(geometry.clone());
        app.select_page(shown);
        let details = geometry.details.expect("the details panel is drawn");
        let buffer = terminal.backend().buffer();
        // The panel's own box, dropped: its rows are padded to its width, and
        // joining that padding into a phrase would split it again.
        (details.y + 1..details.bottom().saturating_sub(1))
            .map(|y| {
                (details.x + 1..details.right().saturating_sub(1))
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The rows of the details panel as the screen drew them, one string per
    /// screen row: the wrapping the panel did is kept, so a row that spilled
    /// onto the row below it is two of these.
    fn panel_rows(
        state: &ObservationState,
        app: &mut App,
        page: DetailPage,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let shown = app.detail_page();
        app.select_page(page);
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("infallible test backend");
        let mut geometry = Geometry::default();
        terminal
            .draw(|frame| {
                geometry = render(frame, state, app, &mut ListState::default(), 0);
            })
            .expect("draw");
        app.note_layout(geometry.clone());
        app.select_page(shown);
        let details = geometry.details.expect("the details panel is drawn");
        let buffer = terminal.backend().buffer();
        (details.y + 1..details.bottom().saturating_sub(1))
            .map(|y| {
                (details.x + 1..details.right().saturating_sub(1))
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .filter(|row| !row.is_empty())
            .collect()
    }

    /// The details panel as the screen drew it, one string per drawn cell of
    /// every screen row: a string's place in one of these is the column its cell
    /// is drawn in, which is what the table's columns have to line up in. Rows
    /// the panel left empty are dropped.
    fn panel_cell_rows(
        state: &ObservationState,
        app: &mut App,
        page: DetailPage,
        width: u16,
        height: u16,
    ) -> Vec<Vec<String>> {
        let shown = app.detail_page();
        app.select_page(page);
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("infallible test backend");
        let mut geometry = Geometry::default();
        terminal
            .draw(|frame| {
                geometry = render(frame, state, app, &mut ListState::default(), 0);
            })
            .expect("draw");
        app.note_layout(geometry.clone());
        app.select_page(shown);
        let details = geometry.details.expect("the details panel is drawn");
        let buffer = terminal.backend().buffer();
        (details.y + 1..details.bottom().saturating_sub(1))
            .map(|y| {
                (details.x + 1..details.right().saturating_sub(1))
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()))
            .collect()
    }

    /// The cell a one-cell-per-character string is drawn in on a screen row: the
    /// process a row wrapped into another screen row finds nothing here.
    fn cell_start(cells: &[String], text: &str) -> Option<usize> {
        let wanted: Vec<String> = text.chars().map(|ch| ch.to_string()).collect();
        cells
            .windows(wanted.len())
            .position(|window| window == wanted.as_slice())
    }

    /// A page's lines as one string, flattened the way the panel draws them:
    /// for the page a render builds from a selection rather than from the app's
    /// own state.
    fn page_text(lines: &[Line<'static>]) -> String {
        let mut text = String::new();
        for line in lines {
            for span in &line.spans {
                text.push_str(&span.content);
            }
            text.push('\n');
        }
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// A draw that reports where it put things and stores that layout the way
    /// the main loop does, so the panel scrolls against the content it just
    /// drew.
    fn draw(
        state: &ObservationState,
        app: &mut App,
        width: u16,
        height: u16,
    ) -> (String, Geometry) {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("infallible test backend");
        let mut geometry = Geometry::default();
        terminal
            .draw(|frame| {
                geometry = render(frame, state, app, &mut ListState::default(), 0);
            })
            .expect("draw");
        app.note_layout(geometry.clone());
        let buffer = terminal.backend().buffer();
        let screen = (0..buffer.area.height)
            .map(|y| row_text(buffer, y))
            .collect::<Vec<_>>()
            .join("\n");
        (screen, geometry)
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

    /// A lineage whose UUIDs are derived from small numbers, so a test can name
    /// the exact session a parent link points at.
    fn lineage(uuid: u32, parent: Option<u32>) -> crate::model::Lineage {
        let root = |n: u32| {
            crate::model::SessionUuid::parse(&format!("01a1{n:04x}-0000-4000-8000-{n:012x}"))
                .expect("uuid-shaped")
        };
        crate::model::Lineage {
            session: root(uuid),
            parent: parent.map(root),
        }
    }

    /// A one-workspace fleet holding exactly these agents, each with a pane.
    fn lineage_state(agents: Vec<AgentObservation>) -> ObservationState {
        let panes = agents
            .iter()
            .map(|agent| Pane {
                location: agent.location.clone(),
                label: None,
                title: None,
            })
            .collect();
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
            panes,
            agents,
        });
        state
    }

    #[test]
    fn source_says_what_became_of_the_parent_link() {
        // One worker, and two rows publishing the session its parent link names.
        // The tree does not invent a nesting it cannot choose, so the reason has
        // to be readable: a worker at a workspace root otherwise looks orphaned.
        let agent = |pane: &str, lineage: crate::model::Lineage| AgentObservation {
            location: Location {
                workspace_id: "wH".into(),
                tab_id: "wH:t1".into(),
                pane_id: pane.into(),
            },
            name: Some("pi".into()),
            label: Some(format!("agent {pane}")),
            status: Some(RuntimeStatus::Idle),
            session: None,
            lineage: Some(lineage),
            facts: HerdsmanFacts::default(),
        };

        let ambiguous = lineage_state(vec![
            agent("wH:p2Q", lineage(1, Some(2))),
            agent("wH:p2P", lineage(2, None)),
            agent("wH:p1E", lineage(2, None)),
        ]);
        let mut app = app_for(&ambiguous);
        select_agent(&mut app, "wH:p2Q");
        let source = render_page(&ambiguous, &mut app, DetailPage::Source, 180, 30);
        assert!(
            source.contains("session uuid: 01a10001-0000-4000-8000-000000000001"),
            "{source}"
        );
        assert!(
            source.contains("parent: 01a10002-0000-4000-8000-000000000002"),
            "{source}"
        );
        assert!(
            source.contains("2 rows") && source.contains("publish this session"),
            "the ambiguity is the reason the row is not nested: {source}"
        );

        // The same worker with one claimant: the link the tree used is named.
        let linked = lineage_state(vec![
            agent("wH:p2Q", lineage(1, Some(2))),
            agent("wH:p2P", lineage(2, None)),
        ]);
        let mut app = app_for(&linked);
        select_agent(&mut app, "wH:p2Q");
        let source = render_page(&linked, &mut app, DetailPage::Source, 180, 30);
        assert!(
            source.contains("nested") && source.contains("under wH:p2P · agent wH:p2P"),
            "{source}"
        );
    }

    #[test]
    fn tree_and_detail_show_normalized_facts_without_tokens() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let screen = render_pages(&state, &mut app, 180, 30);

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
        let screen = render_pages(&state, &mut app, 180, 30);

        assert!(screen.contains("retained (shell foreground)"), "{screen}");
        assert!(
            screen.contains("agent (retained — shell foreground)"),
            "{screen}"
        );
        assert!(screen.contains("status (last observed): idle"), "{screen}");
        assert!(screen.contains("not currently observed"), "{screen}");
        // Exactly one row for the pane: the tree draws the name once, and the
        // panel names it twice on Source — its own title and the session's
        // human name.
        let source = render_page(&state, &mut app, DetailPage::Source, 180, 30);
        assert_eq!(source.matches("fleet owner task").count(), 3, "{source}");
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
        let screen = render_pages(&state, &mut app, 180, 30);

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

        // The runtime text reaches the screen on more than one page: the label
        // is a row fact, the role is Overview's and the failure is written on
        // Source. Every page it can appear on is read.
        let mut screen = String::new();
        for page in [DetailPage::Overview, DetailPage::Source] {
            app.select_page(page);
            let mut terminal = Terminal::new(TestBackend::new(90, 30)).expect("infallible");
            terminal
                .draw(|frame| {
                    render(frame, &state, &app, &mut ListState::default(), 0);
                })
                .expect("draw");
            for cell in terminal.backend().buffer().content() {
                screen.push_str(cell.symbol());
            }
        }

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
        // Give the workspace the name the agent's title ends with, so the
        // suffix check below has something to strip on a row that fits: the
        // clipped row this fixture drew by default left that check resting on
        // the width and the mark column rather than on the title.
        for workspace in &mut observation.workspaces {
            if workspace.workspace_id == "wA" {
                workspace.label = Some("nix-homelab".into());
            }
        }
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
        // be the same thing said twice. The mark is read from the icon table
        // rather than written as `π`: in text mode the pi mark *is* `π`, so a
        // literal here would make the result depend on the installed font
        // rather than on the row. Anchoring to the whole finished-session
        // segment is what catches the duplicate — a row that kept the raw
        // title would carry the mark twice, and the anchored text is the mark
        // drawn once, followed by the stripped name.
        let logo = theme::logo(Some("pi"));
        let row = format!(
            "{EXITED} {}01a0edc4 · exited",
            logo.map_or_else(|| "  ".to_string(), |mark| format!("{mark} "))
        );
        assert!(screen.contains(&row), "{screen}");
        assert!(!screen.contains("π - Inspect"), "{screen}");
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
    fn the_footer_names_the_background_task_toggle_in_both_states() {
        let state = fixture_state();
        let mut app = app_for(&state);
        // Agents view starts with task rows hidden, so the hint says what
        // pressing `b` would do.
        let screen = render_text(&state, &app, 120, 24);
        assert!(screen.contains("b show tasks"), "{screen}");

        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('b'),
            crossterm::event::KeyModifiers::NONE,
        ));
        let screen = render_text(&state, &app, 120, 24);
        assert!(screen.contains("b hide tasks"), "{screen}");

        // A narrow terminal that falls back to the terse wording still names
        // the toggle rather than dropping it.
        let narrow = render_text(&state, &app, 40, 24);
        assert!(narrow.contains("b"), "{narrow}");
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
            let screen = render_pages(&state, &mut app, 180, 34);
            assert!(
                screen.contains(&format!("state: {word} (derived from the pane)")),
                "{screen}"
            );
            assert_eq!(
                screen.contains("projection: lost (owner projection)"),
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
        let screen = render_pages(&state, &mut app, 180, 34);

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
        let screen = render_pages(&state, &mut app, 180, 34);

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
        let screen = render_pages(&state, &mut app, 180, 34);

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
        let screen = render_pages(&state, &mut app, 180, 34);

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
        let screen = render_pages(&state, &mut app, 180, 34);

        // Waiting is a routine state, so the row carries it as mark and colour
        // alone; the panel names it and names the projection beside it.
        assert!(screen.contains("worker task"), "{screen}");
        assert!(!screen.contains("worker task · waiting"), "{screen}");
        assert!(
            screen.contains("state: waiting (derived from the pane)"),
            "{screen}"
        );
        assert!(
            screen.contains("projection: blocked (owner projection)"),
            "{screen}"
        );
        assert!(screen.contains("status: idle"), "{screen}");
    }

    #[test]
    fn an_agent_without_herdsman_facts_draws_no_herdsman_field() {
        let state = state_with_facts(HerdsmanFacts::default(), RuntimeStatus::Idle);
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");
        let screen = render_pages(&state, &mut app, 180, 34);

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
        let screen = render_pages(&state, &mut app, 180, 34);

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
            details_rows: 100,
            details_viewport: 15,
            detail_tabs: [
                Some(Rect::new(42, 1, 10, 1)),
                Some(Rect::new(52, 1, 12, 1)),
                Some(Rect::new(64, 1, 8, 1)),
                Some(Rect::new(42, 2, 8, 1)),
            ],
            offset: 0,
            disclosure_markers: Vec::new(),
            process_rows: Vec::new(),
            process_folds: Vec::new(),
            details_content: Rect::new(41, 2, 28, 17),
            confirm_cancel: None,
            confirm_confirm: None,
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
        app.note_layout(geometry.clone());

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
        app.note_layout(geometry.clone());
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
        app.note_layout(geometry.clone());

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

    #[test]
    fn a_click_on_a_page_tab_shows_that_page_and_hands_the_panel_the_keyboard() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let geometry = geometry_of(&app);
        app.note_layout(geometry.clone());
        assert!(!app.details_focused());

        for page in DetailPage::ALL {
            let tab = geometry.detail_tabs[page.index()].expect("the tab is drawn");
            click_at(&mut app, (tab.x, tab.y));
            assert_eq!(app.detail_page(), page);
            assert!(
                app.details_focused(),
                "the pointer put the keyboard in the panel"
            );
        }
    }

    #[test]
    fn every_page_is_reachable_on_a_narrow_terminal() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        // Narrow enough that the details are stacked under the fleet. The second
        // draw is the settled one: the panel is sized from the layout the last
        // one reported.
        draw(&state, &mut app, 46, 20);
        let (screen, mut geometry) = draw(&state, &mut app, 46, 20);
        let details = geometry.details.expect("the panel is drawn");
        assert!(screen.contains("Details"), "{screen}");

        for page in DetailPage::ALL {
            let tab = geometry.detail_tabs[page.index()]
                .unwrap_or_else(|| panic!("{page:?} has no tab on a narrow terminal"));
            assert!(
                tab.x >= details.x && tab.right() <= details.right() && tab.y < details.bottom(),
                "{page:?} draws outside the panel: {tab:?} in {details:?}"
            );
            click_at(&mut app, (tab.x + tab.width / 2, tab.y));
            assert_eq!(app.detail_page(), page);
            let (narrow, drawn) = draw(&state, &mut app, 46, 20);
            geometry = drawn;
            assert!(narrow.contains(page.label()), "{page:?} is named: {narrow}");
            assert!(
                narrow.contains("worker task"),
                "the row's identity stays on every page: {narrow}"
            );
        }
    }

    #[test]
    fn each_page_keeps_its_own_scroll_and_a_new_selection_starts_at_the_top() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        // Short enough that the panel holds more content than its viewport.
        let (_, geometry) = draw(&state, &mut app, 120, 12);
        let max = (geometry.details_rows - geometry.details_viewport as usize) as u16;
        assert!(max > 0, "the panel has more to show than it fits");

        // Overview scrolls to its last line and stops there.
        app.scroll_page_to(u16::MAX);
        assert_eq!(app.details_scroll(), max);
        app.scroll_page(1);
        assert_eq!(app.details_scroll(), max, "the tail is not scrolled past");

        // Another page starts at its own top, and Overview keeps its place.
        app.select_page(DetailPage::Source);
        assert_eq!(app.details_scroll(), 0);
        app.scroll_page(2);
        app.select_page(DetailPage::Overview);
        assert_eq!(app.details_scroll(), max, "Overview kept its place");

        // A new selection starts every page at the top; the page itself stays.
        select_agent(&mut app, "wA:p1");
        assert_eq!(app.details_scroll(), 0);
        assert_eq!(app.detail_page(), DetailPage::Overview);
    }

    #[test]
    fn a_taller_panel_clamps_the_scroll_it_no_longer_needs() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let (_, short) = draw(&state, &mut app, 120, 12);
        app.scroll_page_to(u16::MAX);
        assert!(app.details_scroll() > 0, "there was something to scroll");

        // The terminal grows: the same content fits with less to scroll, and the
        // offset is clamped to the content rather than left past its last line.
        let (screen, tall) = draw(&state, &mut app, 120, 40);
        assert!(short.details_viewport < tall.details_viewport);
        assert!(
            tall.details_rows <= tall.details_viewport as usize,
            "nothing is left to scroll: {:?}",
            tall.details_rows
        );
        assert_eq!(app.details_scroll(), 0, "the offset came back to the top");
        assert!(
            screen.contains("worker task"),
            "the panel is still showing its row: {screen}"
        );
    }

    #[test]
    fn every_fact_of_a_row_is_reachable_somewhere_across_the_pages() {
        let mut state = state_with_facts(
            HerdsmanFacts {
                label: Some("bash-pane-facts".into()),
                role: Some("worker".into()),
                assignment: Some("Run the terminal smoke suite".into()),
                model: Some("example/model".into()),
                session_name: Some("worker session".into()),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Working,
        );
        state.apply_evidence(
            "wH:p1",
            ForegroundEvidence::command(4242, Some("cargo".into()), Some("test".into())),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");

        // Facts are split across the pages, not dropped: the union of the four
        // is what proves each one still reached the screen.
        let pages = render_pages(&state, &mut app, 180, 34);
        for fact in [
            "kind: agent",
            "pid: 4242",
            "observed: cargo",
            "role: worker",
            "assignment: Run the terminal smoke suite",
            "model: example/model",
            "session name: worker session",
            "source: current",
        ] {
            assert!(pages.contains(fact), "missing {fact:?}:\n{pages}");
        }

        // The identity a reader wants first is where they look first: Overview
        // puts the live PID above the long assignment it belongs to, and
        // Processes carries the process itself.
        let overview = render_page(&state, &mut app, DetailPage::Overview, 180, 34);
        let pid = overview.find("pid: 4242").expect("the PID is on Overview");
        let assignment = overview
            .find("assignment: Run the terminal smoke suite")
            .expect("the assignment is on Overview");
        assert!(pid < assignment, "the PID comes first:\n{overview}");
        let processes = render_page(&state, &mut app, DetailPage::Processes, 180, 34);
        assert!(processes.contains("pid: 4242"), "{processes}");
        assert!(processes.contains("observed: cargo"), "{processes}");
    }

    /// The block's marker as the last draw reported it, for the pointer tests.
    fn marker_of(geometry: &Geometry, key: &Disclosure) -> (u16, u16) {
        let rect = geometry
            .disclosure_markers
            .iter()
            .find(|(marker, _)| marker == key)
            .unwrap_or_else(|| panic!("{key:?} drew no marker: {:?}", geometry.disclosure_markers))
            .1;
        (rect.x, rect.y)
    }

    fn is_inside(area: Rect, at: (u16, u16)) -> bool {
        at.0 >= area.x && at.0 < area.right() && at.1 >= area.y && at.1 < area.bottom()
    }

    #[test]
    fn a_long_assignment_stays_behind_its_marker_until_it_is_opened() {
        // Long enough that the panel collapses it, and carrying control
        // sequences: what the marker opens is the text the panel drew before,
        // sanitized the same way.
        let assignment = format!("\u{1b}[31m{}\u{1b}[0m TAIL", "x".repeat(200));
        let state = state_with_facts(
            HerdsmanFacts {
                label: Some("bash-pane-facts".into()),
                assignment: Some(assignment),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Working,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");

        // Closed: the marker, the opening of the text, and nothing of the tail
        // past the digest.
        let (collapsed, geometry) = draw(&state, &mut app, 120, 30);
        assert!(collapsed.contains("▸ assignment:"), "{collapsed}");
        assert!(
            collapsed.contains("xxxxxxxxxx"),
            "the opening of the text stays readable: {collapsed}"
        );
        assert!(!collapsed.contains("TAIL"), "{collapsed}");
        assert!(!collapsed.contains('\u{1b}'), "{collapsed}");
        let marker = marker_of(&geometry, &Disclosure::Assignment);
        assert!(
            is_inside(geometry.details.expect("the panel is drawn"), marker),
            "the marker is drawn in the panel it belongs to: {marker:?}"
        );

        // The marker is that block's target: a click on it opens the one block
        // and acts on no row.
        assert_eq!(click_at(&mut app, marker), None);
        assert!(app.disclosure_open(&Disclosure::Assignment));
        let (opened, _) = draw(&state, &mut app, 120, 30);
        assert!(
            opened.contains("TAIL"),
            "the whole text is reachable:\n{opened}"
        );
        // The row separators are the only control characters a joined screen
        // has, so an escape among them is a sequence that reached the panel.
        assert!(
            !opened.chars().any(|ch| ch.is_control() && ch != '\n'),
            "{opened}"
        );

        // The terminal narrows until the panel stacks under the fleet: the
        // marker moves with it, and where it used to be drawn is not a target.
        draw(&state, &mut app, 46, 20);
        let (_, narrow) = draw(&state, &mut app, 46, 20);
        assert_eq!(click_at(&mut app, marker), None);
        assert!(
            app.disclosure_open(&Disclosure::Assignment),
            "a position the marker left does not toggle its block"
        );
        let narrow_marker = marker_of(&narrow, &Disclosure::Assignment);
        assert!(
            is_inside(narrow.details.expect("the panel is drawn"), narrow_marker),
            "the marker follows its panel: {narrow_marker:?} in {:?}",
            narrow.details
        );
        assert_eq!(click_at(&mut app, narrow_marker), None);
        assert!(!app.disclosure_open(&Disclosure::Assignment));
        let (closed, _) = draw(&state, &mut app, 46, 20);
        assert!(
            !closed.contains("TAIL"),
            "the block closed again:\n{closed}"
        );
    }

    #[test]
    fn a_page_with_nothing_to_open_reports_no_marker() {
        let state = state_with_facts(
            HerdsmanFacts {
                label: Some("plain".into()),
                ..HerdsmanFacts::default()
            },
            RuntimeStatus::Working,
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wH:p1");

        for page in DetailPage::ALL {
            app.select_page(page);
            let (screen, geometry) = draw(&state, &mut app, 120, 30);
            assert!(app.disclosures().is_empty(), "{page:?} offers a block");
            assert!(
                geometry.disclosure_markers.is_empty(),
                "{page:?} drew a marker with nothing behind it:\n{screen}"
            );
        }
    }

    /// The row at a drawn index, clicked once.
    fn click_row(app: &mut App, geometry: &Geometry, index: usize) -> Option<Action> {
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
        app.note_layout(geometry.clone());
        assert_eq!(drawn_titles(&app), ["wA", "alpha", "wB", "gamma"]);

        // Fold the first workspace by clicking its heading, then the row drawn
        // below it is the next workspace rather than the child it replaced.
        click_row(&mut app, &geometry, 0);
        assert_eq!(drawn_titles(&app), ["wA", "wB", "gamma"]);
        click_row(&mut app, &geometry, 1);
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
        app.note_layout(geometry.clone());
        let index = app
            .visible_rows()
            .iter()
            .position(|row| row.id == &crate::tree::RowId::Agent("wA:p2".to_string()))
            .expect("the retained row is drawn");

        click_row(&mut app, &geometry, index);
        assert!(
            matches!(
                click_row(&mut app, &geometry, index),
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
        app.note_layout(geometry.clone());

        // The row is selected by the first click, so the second is the one that
        // acts.
        let child = 1;
        click_row(&mut app, &geometry, child);
        assert!(matches!(
            click_row(&mut app, &geometry, child),
            Some(Action::Focus(_))
        ));

        state.apply_failure("herdr exited with status 1");
        app.refresh(&state);
        app.note_layout(geometry.clone());
        assert!(
            click_row(&mut app, &geometry, child).is_none(),
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

        // A running command's mark advances with the clock. The row leads with
        // its branch prefix and fold column, so those are skipped to the mark.
        let lead = |screen: &str| {
            screen
                .lines()
                .find(|line| line.contains("nix build .#radar"))
                .and_then(|line| {
                    line.trim_start_matches(['\u{2502}', '\u{251c}', '\u{2514}', '\u{2500}', ' '])
                        .chars()
                        .next()
                })
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
                    binary: Default::default(),
                    resources: None,
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
        // The command, the terminal mode and the age are the Processes page's.
        let details = render_page(&state, &mut app, DetailPage::Processes, 180, 30);
        assert!(details.contains("observed: nvim"), "{details}");
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
    fn a_running_pane_is_led_by_its_command_and_its_process_is_named_in_the_details() {
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
        let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 32);

        // The row leads with the command the pane is running...
        assert!(screen.contains("nix build .#radar"), "{screen}");
        // ...and the page names the process itself rather than the invocation:
        // the observed name is the command the runtime reports.
        assert!(screen.contains("observed: nix"), "{screen}");
        // An identity this machine never read claims nothing: no executable
        // path and no comparison is invented for it.
        assert!(!screen.contains("exe:"), "{screen}");
        assert!(!screen.contains("binary:"), "{screen}");
    }

    /// Foreground evidence for a pane, carrying a binary comparison, so a row
    /// and its details can be read against exactly that identity.
    fn binary_evidence(identity: BinaryIdentity) -> ForegroundEvidence {
        named_binary_evidence("pi", "pi", identity)
    }

    /// The same, for a process the runtime reports under a given name and
    /// command line: the process facts are the runtime's, the comparison is
    /// this machine's.
    fn named_binary_evidence(
        name: &str,
        command: &str,
        identity: BinaryIdentity,
    ) -> ForegroundEvidence {
        let mut evidence =
            ForegroundEvidence::command(4242, Some(name.to_string()), Some(command.to_string()));
        if let ForegroundEvidence::NonShell { local, .. } = &mut evidence {
            *local = LocalFacts {
                running_for: None,
                terminal: TerminalMode::Unknown,
                binary: identity,
                resources: None,
            };
        }
        evidence
    }

    fn stale_identity() -> BinaryIdentity {
        BinaryIdentity {
            freshness: BinaryFreshness::Stale,
            running: Some("/run/pi-1.0.1".into()),
            installed: Some("/nix/store/aaa-pi-1.0.2".into()),
            executable: Some("/run/pi".into()),
            unknown: None,
        }
    }

    /// Foreground evidence for one sampled process: a non-shell command with the
    /// local facts a refresh composed onto it.
    fn sampled(pid: i32, resources: ProcessResources) -> ForegroundEvidence {
        let mut evidence =
            ForegroundEvidence::command(pid, Some("nix".into()), Some("nix build .#radar".into()));
        if let ForegroundEvidence::NonShell { local, .. } = &mut evidence {
            *local = LocalFacts {
                running_for: Some(Duration::from_secs(134)),
                terminal: TerminalMode::Line,
                binary: BinaryIdentity::default(),
                resources: Some(resources),
            };
        }
        evidence
    }

    /// The root identity every sampled-process test reads beneath.
    fn root_identity() -> ProcessIdentity {
        ProcessIdentity {
            boot_id: "6d9d2f0a-2f6f-4a1f-9c2d-2f6f4a1f9c2d".into(),
            pid: 4242,
            start_ticks: 9_812,
        }
    }

    /// One process a scan verified beneath a root: the identity it was read as,
    /// the process it was read beneath, and what that read carried.
    fn descendant(
        pid: i32,
        parent: &ProcessIdentity,
        name: &str,
        state: ProcessState,
        cpu: Option<CpuPercent>,
        rss_bytes: Option<u64>,
    ) -> DescendantSample {
        DescendantSample {
            identity: ProcessIdentity {
                boot_id: parent.boot_id.clone(),
                pid,
                start_ticks: parent.start_ticks + u64::from(pid as u32),
            },
            parent: parent.clone(),
            name: name.to_string(),
            state,
            cpu,
            rss_bytes,
        }
    }

    /// The table's drawn lines for one projection, without the page around them:
    /// the layout tests read the cells a row is built from.
    fn table_lines(
        resources: &ProcessResources,
        root_name: &str,
        width: usize,
    ) -> Vec<Line<'static>> {
        let folded = HashSet::new();
        let table = ProcessTable::of(&folded, resources).expect("a verified tree");
        let mut page = PageLines::default();
        process_table_lines(&table, root_name, width, &table.root, &folded, &mut page);
        page.lines
    }

    /// The tree the process-table tests draw: a quiet root, a busy shell with a
    /// build beneath it, and a process with neither an interval nor a size to
    /// show.
    ///
    /// The members come in no order a reader can follow — a child before its
    /// parent, and the siblings shuffled — because that is the order a scan
    /// walking the table read them in.
    fn read_tree() -> DescendantResources {
        let root = root_identity();
        let shell = descendant(
            300,
            &root,
            "bash",
            ProcessState::Sleeping,
            Some(CpuPercent::from_hundredths(1_250)),
            Some(3 * 1024 * 1024),
        );
        let editor = descendant(310, &root, "node", ProcessState::Running, None, None);
        let build = descendant(
            311,
            &shell.identity,
            "esbuild-service-worker",
            ProcessState::DiskSleep,
            Some(CpuPercent::from_hundredths(2_000)),
            Some(64 * 1024 * 1024),
        );
        DescendantResources {
            observed: Some(3),
            members: vec![build, editor, shell],
            rss_bytes: Total::Complete(67 * 1024 * 1024),
            cpu: Total::Complete(CpuPercent::from_hundredths(3_250)),
        }
    }

    /// A root and two descendants observed under names that take more cells than
    /// they have characters — one wider than its characters, one whose characters
    /// take no cells of their own — at pids shorter than the table's own `pid`
    /// heading.
    ///
    /// The names are what a layout counted in characters gets wrong, and the pids
    /// are what a column measured from its values alone gets wrong.
    fn wide_resources() -> ProcessResources {
        let root = ProcessIdentity {
            boot_id: root_identity().boot_id,
            pid: 7,
            start_ticks: 9_812,
        };
        ProcessResources {
            identity: root.clone(),
            state: ProcessState::Sleeping,
            rss_bytes: Some(8 * 1024 * 1024),
            cpu: Some(CpuPercent::from_hundredths(0)),
            descendants: DescendantResources {
                observed: Some(2),
                members: vec![
                    descendant(
                        88,
                        &root,
                        "日本語のビルド",
                        ProcessState::Sleeping,
                        Some(CpuPercent::from_hundredths(1_250)),
                        Some(3 * 1024 * 1024),
                    ),
                    descendant(
                        9,
                        &root,
                        "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
                        ProcessState::Running,
                        Some(CpuPercent::from_hundredths(2_000)),
                        Some(64 * 1024 * 1024),
                    ),
                ],
                rss_bytes: Total::Complete(67 * 1024 * 1024),
                cpu: Total::Complete(CpuPercent::from_hundredths(3_250)),
            },
        }
    }

    /// The pane whose table is drawn from [`wide_resources`], its process
    /// observed under a name that is twice as wide as it is long.
    fn wide_tree() -> ForegroundEvidence {
        let mut evidence = sampled(7, wide_resources());
        if let ForegroundEvidence::NonShell { name, .. } = &mut evidence {
            *name = Some("日本語のルート".into());
        }
        evidence
    }

    /// A pane whose process was sampled, with everything the scan confirmed
    /// beneath it and the comparison this machine made of the running file.
    fn sampled_tree(binary: BinaryIdentity) -> ForegroundEvidence {
        let mut evidence = sampled(
            4242,
            resources(
                // A measured idle interval is a measurement: the root is quiet
                // and the shell beneath it is not.
                Some(CpuPercent::from_hundredths(0)),
                Some(8 * 1024 * 1024),
                read_tree(),
            ),
        );
        if let ForegroundEvidence::NonShell { local, .. } = &mut evidence {
            local.binary = binary;
        }
        evidence
    }

    /// A sampled process, with whatever the scan observed beneath it.
    fn resources(
        cpu: Option<CpuPercent>,
        rss_bytes: Option<u64>,
        descendants: DescendantResources,
    ) -> ProcessResources {
        ProcessResources {
            identity: root_identity(),
            state: ProcessState::Sleeping,
            rss_bytes,
            cpu,
            descendants,
        }
    }

    /// Nothing observed beneath a root, with every process of the scan read.
    fn no_descendants() -> DescendantResources {
        DescendantResources {
            members: Vec::new(),
            observed: Some(0),
            rss_bytes: Total::Complete(0),
            cpu: Total::Complete(CpuPercent::from_hundredths(0)),
        }
    }

    #[test]
    fn an_idle_root_and_a_busy_descendant_are_drawn_apart() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    // A measured idle interval is a measurement: zero, not
                    // unknown.
                    Some(CpuPercent::from_hundredths(0)),
                    Some(8 * 1024 * 1024),
                    DescendantResources {
                        members: Vec::new(),
                        observed: Some(2),
                        rss_bytes: Total::Complete(512 * 1024 * 1024),
                        // Work on more than one CPU carries past 100%.
                        cpu: Total::Complete(CpuPercent::from_hundredths(12_500)),
                    },
                ),
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        // The section heading and the kernel's own state as one short name: the
        // incarnation the reading belongs to, and what the name means, are the
        // block's.
        assert!(processes.contains("process state  sleeping"), "{processes}");
        assert!(
            !processes.contains("waiting, and wakeable"),
            "the kernel's state is a value, not a sentence: {processes}"
        );
        assert!(!processes.contains("boot "), "{processes}");
        assert!(!processes.contains("start ticks"), "{processes}");
        // The root's own figures are its own: the build beneath it is not added
        // to them. Two facts share the line, with the values aligned.
        assert!(processes.contains("cpu  0.0%  rss  8.0 MiB"), "{processes}");
        // The descendants are a separate sum, under a heading of their own.
        assert!(
            processes.contains("descendants count  2 processes"),
            "{processes}"
        );
        assert!(
            processes.contains("cpu  125.0%  rss  512.0 MiB"),
            "{processes}"
        );
        // The sum is qualified as what it is, and never added to the root's:
        // the sentence that says so is held behind the block with the birth
        // identity, and neither is on the line a reader scans.
        assert!(
            !processes.contains("not a workload or assignment total"),
            "{processes}"
        );
        app.toggle_block(&Disclosure::Process);
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        // Both are held behind the block and wrapped into its value column, so
        // the sentence and the birth identity are asserted in their parts.
        assert!(
            processes.contains("observed processes beneath this one"),
            "{processes}"
        );
        assert!(processes.contains("counts in each)"), "{processes}");
        assert!(
            draws_row(&processes, "birth", "pid 4242 · start ticks 9812"),
            "{processes}"
        );
        assert!(
            draws_row(&processes, "boot", "6d9d2f0a-2f6f-4a1f-9c2d"),
            "{processes}"
        );
    }

    #[test]
    fn cpu_without_an_interval_is_unavailable_rather_than_zero() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(4242, resources(None, Some(4_096), no_descendants())),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        // No interval to measure — a first reading, a counter that went
        // backwards and a failed read all say this, and none is a zero. The
        // value keeps the line; the reason it is not one is drawn beneath it.
        assert!(processes.contains("cpu  unavailable"), "{processes}");
        assert!(
            processes.contains("no interval of this process has been measured"),
            "{processes}"
        );
        // A descendant total that really is zero is drawn as a measurement.
        assert!(processes.contains("cpu  0.0%"), "{processes}");
        // The root's own line, between the state above it and the descendants
        // below it, is the one that must not read as a zero.
        let root = processes
            .split("state  sleeping")
            .nth(1)
            .and_then(|rest| rest.split("descendants").next())
            .expect("the state and the descendant heading");
        assert!(root.contains("cpu  unavailable"), "{processes}");
        assert!(!root.contains("0.0%"), "{processes}");
        // Nothing observed beneath the root is a fact about the root, not a
        // missing measurement.
        assert!(processes.contains("count  0 processes"), "{processes}");
    }

    #[test]
    fn a_partial_or_unknown_total_says_why() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(4_200)),
                    Some(1_048_576),
                    DescendantResources {
                        members: Vec::new(),
                        observed: Some(3),
                        rss_bytes: Total::Partial(
                            2_048,
                            "a process beneath this one was reparented while the table was read"
                                .into(),
                        ),
                        cpu: Total::Unknown("the process scan was cancelled".into()),
                    },
                ),
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        // The root's own reading is unaffected by what could not be totalled
        // beneath it.
        assert!(processes.contains("cpu  42.0%"), "{processes}");
        // A lower bound keeps the value it does cover, marked as the bound it
        // is, and says why on the line beneath it.
        assert!(processes.contains("rss  ≥2.0 KiB"), "{processes}");
        assert!(
            processes
                .contains("a process beneath this one was reparented while the table was read"),
            "{processes}"
        );
        // An unknown total is unavailable with its reason, never a complete zero.
        assert!(processes.contains("cpu  unavailable"), "{processes}");
        assert!(
            processes.contains("the process scan was cancelled"),
            "{processes}"
        );
        assert!(processes.contains("count  3 processes"), "{processes}");
    }

    #[test]
    fn descendants_that_could_not_be_enumerated_are_unavailable() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(100)),
                    Some(4_096),
                    DescendantResources {
                        members: Vec::new(),
                        observed: None,
                        rss_bytes: Total::Unknown("the process table could not be read".into()),
                        cpu: Total::Unknown("the process table could not be read".into()),
                    },
                ),
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        assert!(processes.contains("count  unavailable"), "{processes}");
        assert!(
            processes.contains("nothing beneath this process could be enumerated"),
            "{processes}"
        );
        assert!(processes.contains("rss  unavailable"), "{processes}");
        assert!(processes.contains("cpu  unavailable"), "{processes}");
        // Each value that could not be totalled says so, with the reason the
        // table gave once per value it belongs to.
        assert_eq!(
            processes
                .matches("the process table could not be read")
                .count(),
            2,
            "{processes}"
        );
    }

    #[test]
    fn a_stale_or_retained_row_withholds_its_metrics() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(12_340)),
                    Some(1_024),
                    no_descendants(),
                ),
            ),
        );
        // The evidence survives a failed collection, and none of it may be
        // drawn as a live measurement.
        state.apply_failure("herdr exited with status 1");
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(
            processes.contains("withheld — the source is not current"),
            "{processes}"
        );
        assert!(!processes.contains("birth"), "{processes}");
        assert!(!processes.contains("cpu"), "{processes}");
        assert!(!processes.contains("rss"), "{processes}");
        assert!(!processes.contains("descendants"), "{processes}");

        // A retained row is an association nobody observes now: its
        // last-observed process is not a reading either.
        let retained = retained_working_state();
        let mut app = app_for(&retained);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&retained, &mut app, DetailPage::Processes);
        assert!(
            processes.contains("withheld — this row is retained, not currently observed"),
            "{processes}"
        );
        assert!(!processes.contains("birth"), "{processes}");
        assert!(!processes.contains("descendants"), "{processes}");
    }

    #[test]
    fn a_process_with_no_sample_invents_no_metric() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            ForegroundEvidence::command(4242, Some("pi".into()), Some("pi".into())),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&state, &mut app, DetailPage::Processes);

        // The process is named and nothing was sampled for it: the page says so
        // rather than heading a section and drawing a birth identity or a
        // resource it never read.
        assert!(processes.contains("pid: 4242"), "{processes}");
        assert!(
            processes
                .contains("metrics: unavailable — this refresh took no sample of this process"),
            "{processes}"
        );
        assert!(!processes.contains("birth"), "{processes}");
        assert!(!processes.contains("rss"), "{processes}");
        assert!(!processes.contains("descendants"), "{processes}");
    }

    /// Whether one drawn row of the panel carries every one of these cells: a
    /// table row is asserted as the cells a reader sees rather than as the
    /// padding between them.
    fn row_has(rows: &[String], cells: &[&str]) -> bool {
        rows.iter().any(|row| {
            let flat = row.split_whitespace().collect::<Vec<_>>().join(" ");
            cells.iter().all(|cell| flat.contains(cell))
        })
    }

    #[test]
    fn a_branch_too_deep_for_the_name_column_keeps_the_levels_nearest_the_process() {
        let root = root_identity();
        let mut parent = root.clone();
        let mut members = Vec::new();
        for (depth, pid) in (500..=508).enumerate() {
            let member = descendant(
                pid,
                &parent,
                &format!("process-at-depth-{depth}"),
                ProcessState::Sleeping,
                None,
                None,
            );
            parent = member.identity.clone();
            members.push(member);
        }
        let resources = resources(
            None,
            None,
            DescendantResources {
                observed: Some(members.len() as u32),
                members,
                rss_bytes: Total::Complete(0),
                cpu: Total::Complete(CpuPercent::from_hundredths(0)),
            },
        );
        let table = ProcessTable::of(&HashSet::new(), &resources).expect("a verified tree");
        assert_eq!(table.rows.len(), 10, "the root and nine processes under it");

        // No width the panel can be, however narrow, is allowed to wrap a row
        // into the row below it or to run past its own edge.
        for width in [8, 12, 20, 28, 58] {
            let lines = table_lines(&resources, "nix", width);
            assert!(
                lines.iter().all(|line| line.width() <= width),
                "a row is wider than the panel at {width}: {lines:?}"
            );
            assert_eq!(
                page_rows(&lines, width as u16).0,
                lines.len(),
                "a row wrapped at {width}"
            );
        }

        // The deepest process keeps the levels nearest it rather than drawing
        // flush with the root's own children, and says its ancestry is cut.
        let deepest = table_lines(&resources, "nix", 14)
            .pop()
            .expect("the last row of the table");
        let deepest: String = deepest
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let deepest = deepest.trim_start();
        assert!(deepest.starts_with('…'), "{deepest:?}");
        assert!(deepest.contains('└'), "{deepest:?}");
    }

    #[test]
    fn a_wide_name_is_measured_in_cells_and_never_draws_past_the_panel() {
        let resources = wide_resources();

        // Every width the page is ever drawn at, from the narrowest content
        // column the details panel has (30 cells of panel less its borders) to
        // well past the columns: no line is wider than the panel it is drawn in.
        // A row laid out in characters overruns the panel, and the panel wraps
        // the rest of the process into the row below it.
        for width in 28..=200 {
            for line in table_lines(&resources, "日本語のルート", width) {
                assert!(
                    line.width() <= width,
                    "at {width} cells: {} drawn",
                    line.width()
                );
            }
        }

        // A name with characters wider than one cell is shortened to a width,
        // never emptied of the characters a terminal draws:
        let wide: String = table_lines(&resources, "日本語のルート", 200)
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(wide.contains("日本語のルート"), "{wide}");
        assert!(wide.contains("日本語のビルド"), "{wide}");
        assert!(wide.contains("e\u{301}e\u{301}e\u{301}"), "{wide}");
    }

    #[test]
    fn the_table_heading_is_part_of_the_column_it_names() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", wide_tree());
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.toggle_block(&Disclosure::ProcessTable);

        // Every pid in this tree is shorter than the heading that names the
        // column: were the column measured from its values alone, the heading
        // would sit one cell from the name of whichever process fills the name
        // column, in the cell the gap is there to keep clear.
        let rows = panel_cell_rows(&state, &mut app, DetailPage::Processes, 84, 40);
        let header = rows
            .iter()
            .position(|cells| cell_start(cells, "name") == Some(TABLE_MARKER))
            .unwrap_or_else(|| panic!("the table's heading: {rows:?}"));
        let table = &rows[header..];
        let heading = cell_start(&table[0], "pid").expect("the pid heading");
        let name_end = (0..heading)
            .rev()
            .find(|index| {
                table[1..]
                    .iter()
                    .any(|cells| !cells[*index].trim().is_empty())
            })
            .unwrap_or_else(|| panic!("a name in the column: {table:?}"));
        let gap = heading - name_end - 1;
        assert!(
            gap >= TABLE_GAP,
            "the heading is {gap} cells from the name column: {table:?}"
        );
    }

    #[test]
    fn wide_names_keep_the_numeric_columns_in_one_cell_in_every_row() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", wide_tree());
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.toggle_block(&Disclosure::ProcessTable);

        // What the table draws, row by row: the heading, then the root and the
        // processes beneath it in pid order. The heading is wider than every pid
        // under it.
        let headings = ["pid", "cpu", "rss"];
        let drawn = [
            ["7", "0.0%", "8.0 MiB"],
            ["9", "20.0%", "64.0 MiB"],
            ["88", "12.5%", "3.0 MiB"],
        ];
        let widths: Vec<usize> = (0..headings.len())
            .map(|column| {
                drawn
                    .iter()
                    .map(|row| display_cells(row[column]))
                    .chain([display_cells(headings[column])])
                    .max()
                    .unwrap()
            })
            .collect();

        // 84 columns of terminal put the page in its narrowest content column
        // (28 cells), where the marker column costs the resident set its own:
        // 140 and 200 leave the page wide enough for every column. The columns
        // go one at a time, the widest fact first — never the pid.
        for (width, height, columns) in [
            (200u16, 40u16, &headings[..]),
            (140, 40, &headings[..]),
            (84, 40, &headings[..2]),
        ] {
            let rows = panel_cell_rows(&state, &mut app, DetailPage::Processes, width, height);
            let header = rows
                .iter()
                .position(|cells| cell_start(cells, "name") == Some(TABLE_MARKER))
                .unwrap_or_else(|| panic!("the table's heading at {width}: {rows:?}"));
            let table = &rows[header..];
            let heading = &table[0];
            let shown = |column: &str| cell_start(heading, column).is_some();
            for column in &headings {
                assert_eq!(
                    shown(column),
                    columns.contains(column),
                    "{column:?} at {width} columns: {heading:?}"
                );
            }

            // Each row of the table is one line of the panel: its name, its pid
            // and its figures are drawn on the same screen row, so nothing
            // wrapped a process into the row below — and every pid is drawn.
            let mut starts: Vec<Vec<usize>> = Vec::new();
            for (index, values) in drawn.iter().enumerate() {
                let cells = table
                    .get(index + 1)
                    .unwrap_or_else(|| panic!("row {index} at {width} columns: {table:?}"));
                let mut row = Vec::new();
                for (position, heading) in headings.iter().enumerate() {
                    if !shown(heading) {
                        continue;
                    }
                    row.push(
                        cell_start(cells, values[position])
                            .map(|start| start + display_cells(values[position]))
                            .unwrap_or_else(|| {
                                panic!(
                                    "{:?} is not on its own row at {width}: {table:?}",
                                    values[position]
                                )
                            }),
                    );
                }
                starts.push(row);
            }

            // The heading and every process beneath it put the columns they draw
            // in the same cells: the values are right-aligned in their columns, so
            // a column ends in one cell and begins in one cell, whatever the name
            // beside it is made of.
            for (index, row) in starts.iter().enumerate() {
                let offset = |values: &Vec<usize>| -> Vec<usize> {
                    values
                        .iter()
                        .zip(headings.iter().filter(|heading| shown(heading)))
                        .map(|(end, heading)| {
                            end - widths[headings
                                .iter()
                                .position(|name| name == heading)
                                .expect("a drawn column")]
                        })
                        .collect()
                };
                assert_eq!(
                    offset(row),
                    offset(&starts[0]),
                    "row {index} draws its numbers elsewhere at {width}: {table:?}"
                );
            }
        }
    }

    #[test]
    fn the_process_table_draws_the_confirmed_tree_in_the_columns_it_names() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", sampled_tree(BinaryIdentity::default()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");

        // Collapsed, the page pays one line for the table and draws none of the
        // processes beneath the root.
        let collapsed = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        assert!(collapsed.contains("▸ process table"), "{collapsed}");
        for name in ["bash", "node", "esbuild"] {
            assert!(!collapsed.contains(name), "{name:?}: {collapsed}");
        }

        app.toggle_block(&Disclosure::ProcessTable);
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        let flat = processes.split_whitespace().collect::<Vec<_>>().join(" ");

        // The columns are named, and the first row is the root itself: the
        // anchor every branch below it hangs from, and the reason the root's own
        // figures are here rather than among the sums.
        assert!(processes.contains("▾ process table"), "{processes}");
        assert!(flat.contains("name pid cpu rss"), "{processes}");
        // One row per confirmed process, each at the depth its parent puts it,
        // with the siblings of one parent by pid — the order the scan read them
        // in was no order at all.
        for row in [
            "nix 4242 0.0% 8.0 MiB",
            "├─ bash 300 12.5% 3.0 MiB",
            "│ └─ esbuild-service-worker 311 20.0% 64.0 MiB",
            "└─ node 310 — —",
        ] {
            assert!(flat.contains(row), "{row:?} is not drawn: {processes}");
        }
        let drawn: Vec<usize> = [
            "nix 4242",
            "bash 300",
            "esbuild-service-worker 311",
            "node 310",
        ]
        .iter()
        .map(|row| {
            flat.find(row)
                .unwrap_or_else(|| panic!("{row:?}: {processes}"))
        })
        .collect();
        assert!(drawn.is_sorted(), "not in hierarchy order: {processes}");

        // A process whose interval was never measured draws a dash — not the
        // zero a measured idle interval is — and the table says what it means.
        let node = processes
            .split("└─ node")
            .nth(1)
            .expect("the node row is drawn");
        assert!(!node.contains("0.0%"), "{processes}");
        assert!(processes.contains("— not measured"), "{processes}");
        // While the root, quiet, was measured: its own zero is a reading.
        assert!(flat.contains("nix 4242 0.0%"), "{processes}");
    }

    #[test]
    fn a_narrow_panel_keeps_every_process_row_on_one_line() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", sampled_tree(BinaryIdentity::default()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.toggle_block(&Disclosure::ProcessTable);

        // The narrowest content column the details are ever drawn in: the panel
        // beside the tree is never narrower, and below that width the whole
        // body is the panel.
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 84, 40);
        let header = rows
            .iter()
            .position(|row| row.trim_start().starts_with("name"))
            .unwrap_or_else(|| panic!("the table's own header: {rows:?}"));
        let table = &rows[header..];

        // Every row is one line of the panel: the name, the pid and what the
        // row has to say about it are on the same line, and nothing spilled
        // into the row below.
        for cells in [
            &["nix", "4242", "0.0%"][..],
            &["bash", "300", "12.5%"][..],
            &["esbuil…", "311", "20.0%"][..],
            &["node", "310", "—"][..],
        ] {
            assert!(row_has(table, cells), "{cells:?} is not one row: {rows:?}");
        }
        // The resident set is the column that went, and a name the panel cannot
        // hold whole is shortened rather than wrapped onto the row below. The
        // heading names the columns that were drawn, and no others.
        assert!(!row_has(table, &["rss"]), "the size is drawn: {rows:?}");
        assert!(!row_has(table, &["MiB"]), "the size is drawn: {rows:?}");
        assert!(row_has(table, &["esbuil…"]), "{rows:?}");
        // The fold marker is drawn where there is a branch to fold, and the cell
        // is left blank where there is none: the row's own marker column.
        assert!(row_has(table, &["▾", "nix"]), "{rows:?}");
        assert!(row_has(table, &["▾", "bash"]), "{rows:?}");
        assert!(
            !row_has(table, &["▾", "node"]),
            "a leaf is foldable: {rows:?}"
        );
        // The pid is the column that never goes.
        for pid in ["4242", "300", "311", "310"] {
            assert!(row_has(table, &[pid]), "{pid} is not drawn: {rows:?}");
        }
    }

    #[test]
    fn a_short_panel_reaches_every_process_row_by_scrolling() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", sampled_tree(BinaryIdentity::default()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        app.toggle_block(&Disclosure::ProcessTable);

        // A panel with room for a few rows draws what it can hold and keeps the
        // rest a scroll away, which is where the table is.
        let (_, geometry) = draw(&state, &mut app, 200, 14);
        assert!(
            geometry.details_rows > geometry.details_viewport as usize,
            "the page is not longer than the panel: {geometry:?}"
        );
        app.scroll_page_to(u16::MAX);
        let (screen, geometry) = draw(&state, &mut app, 200, 14);
        assert_eq!(
            app.details_scroll() as usize,
            geometry.details_rows - geometry.details_viewport as usize,
            "the last row of the page is not the last the panel can reach"
        );

        // Every row of the table is on a screen row of its own — none of them
        // wrapped into the row below — and the last of them is reachable.
        let screen_rows: Vec<String> = screen.lines().map(str::to_string).collect();
        for cells in [
            &["├─", "bash", "300", "12.5%", "3.0 MiB"][..],
            &[
                "│",
                "└─",
                "esbuild-service-worker",
                "311",
                "20.0%",
                "64.0 MiB",
            ][..],
            &["└─", "node", "310", "—"][..],
        ] {
            assert!(
                row_has(&screen_rows, cells),
                "{cells:?} is not one row: {screen}"
            );
        }
        assert!(screen.contains("— not measured"), "{screen}");
    }

    #[test]
    fn a_selected_descendant_draws_its_own_sample_and_not_the_root_binary() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", sampled_tree(stale_identity()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let shell = read_tree()
            .members
            .into_iter()
            .find(|member| member.identity.pid == 300)
            .expect("the shell's own verified row");

        // The page a render builds for the reader's selection: the block above
        // the heading describes the shell, from its own sample.
        let page = detail_page_lines(
            &state,
            &app,
            DetailPage::Processes,
            58,
            Some(&shell.identity),
        );
        let processes = page_text(&page.lines);
        let (selected, root) = processes
            .split_once("foreground root")
            .unwrap_or_else(|| panic!("the root's evidence, under its own heading: {processes}"));
        assert!(selected.contains("pid: 300"), "{processes}");
        assert!(selected.contains("observed: bash"), "{processes}");
        assert!(selected.contains("cpu 12.5%"), "{processes}");
        assert!(selected.contains("rss 3.0 MiB"), "{processes}");
        // Nothing of the root's binary identity is attributed to the child: the
        // comparison is made from the root's own executable, and a descendant's
        // sample says nothing about which build the root runs.
        for borrowed in ["package", "binary", "/run/pi", "stale", "current"] {
            assert!(
                !selected.contains(borrowed),
                "{borrowed:?} is attributed to the selected row: {processes}"
            );
        }
        // It is still drawn, under the root's own heading, with the root's pid
        // and the qualified sums beneath it.
        assert!(root.contains("pid: 4242"), "{processes}");
        assert!(root.contains("observed: nix"), "{processes}");
        assert!(root.contains("package: /run/pi-1.0.1"), "{processes}");
        assert!(root.contains("binary:"), "{processes}");
        assert!(root.contains("(stale)"), "{processes}");
        assert!(
            root.contains("descendants count 3 processes"),
            "{processes}"
        );

        // A selection this observation does not carry — a process that exited,
        // or a pid the kernel handed on — draws the root rather than fitting a
        // stranger's facts to it.
        let replaced = ProcessIdentity {
            start_ticks: 1,
            ..shell.identity.clone()
        };
        let page = detail_page_lines(&state, &app, DetailPage::Processes, 58, Some(&replaced));
        let processes = page_text(&page.lines);
        assert!(!processes.contains("foreground root"), "{processes}");
        assert!(processes.contains("observed: nix"), "{processes}");
    }

    #[test]
    fn the_drawn_page_describes_the_selected_process_and_draws_the_root_once() {
        // The production path: the selection the cursor made is what the drawn
        // page describes, not one handed to the line builder directly.
        let (state, mut app) = process_table_app(sampled_tree(stale_identity()));
        let shell = tree_identity(300);
        press_key(&mut app, KeyCode::Char('t'));
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.process_selection(), Some(&shell));

        let panel = panel_text(&state, &mut app, DetailPage::Processes);
        let (above, table) = panel
            .split_once("process table")
            .expect("the table's marker");
        let (selected, root) = above
            .split_once("foreground root")
            .expect("the root's own heading");
        // The selected row's own sample: its name, its incarnation and its
        // figures, drawn from the observation the page was built against.
        assert!(draws_row(selected, "pid:", "300"), "{panel}");
        assert!(draws_row(selected, "observed:", "bash"), "{panel}");
        assert!(draws_row(selected, "state", "sleeping"), "{panel}");
        assert!(draws_row(selected, "cpu", "12.5%"), "{panel}");
        assert!(draws_row(selected, "rss", "3.0 MiB"), "{panel}");
        for borrowed in ["package", "binary", "(stale)", "/run/pi"] {
            assert!(
                !selected.contains(borrowed),
                "{borrowed:?} is attributed to the child: {panel}"
            );
        }
        // The root's own evidence stays under its own heading...
        assert!(draws_row(root, "pid:", "4242"), "{panel}");
        assert!(root.contains("(stale)"), "{panel}");
        // ...and its figures are its row in the table, not a second section
        // above it.
        assert!(
            !draws_row(root, "cpu", "0.0%"),
            "the root's own section is drawn twice: {panel}"
        );
        assert!(draws_row(table, "nix", "4242"), "{panel}");
    }

    #[test]
    fn the_process_table_is_offered_only_where_a_scan_verified_the_tree() {
        // A scan that read the table and found nothing beneath the root is a
        // measured empty observation: the root is still a row, and the only one.
        let mut live = fixture_state();
        live.apply_evidence(
            "wA:p2",
            sampled(4242, resources(None, Some(4_096), no_descendants())),
        );
        let mut app = app_for(&live);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        assert_eq!(
            app.disclosures(),
            vec![Disclosure::Process, Disclosure::ProcessTable]
        );
        app.toggle_block(&Disclosure::ProcessTable);
        let processes = panel_text_at(&live, &mut app, DetailPage::Processes, 200);
        let flat = processes.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("nix 4242 — 4.0 KiB"), "{processes}");
        assert!(processes.contains("count  0 processes"), "{processes}");

        // A scan that could not enumerate is not a tree of nothing: the page
        // offers no table, and the sums say what could not be read instead.
        let mut unknown = fixture_state();
        unknown.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(0)),
                    Some(4_096),
                    DescendantResources {
                        observed: None,
                        members: Vec::new(),
                        rss_bytes: Total::Unknown("the process scan was cancelled".into()),
                        cpu: Total::Unknown("the process scan was cancelled".into()),
                    },
                ),
            ),
        );
        let mut app = app_for(&unknown);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        assert_eq!(app.disclosures(), vec![Disclosure::Process]);
        let processes = panel_text_at(&unknown, &mut app, DetailPage::Processes, 200);
        assert!(!processes.contains("process table"), "{processes}");
        assert!(processes.contains("count  unavailable"), "{processes}");
        assert!(
            processes.contains("nothing beneath this process could be enumerated"),
            "{processes}"
        );
    }

    #[test]
    fn the_process_table_marker_opens_it_from_the_page() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", sampled_tree(BinaryIdentity::default()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);

        let (closed, mut geometry) = draw(&state, &mut app, 120, 30);
        assert!(!closed.contains("esbuild"), "{closed}");
        // Both of the page's blocks are drawn, each with the marker that answers
        // for it, in the order the page offers them.
        let details = marker_of(&geometry, &Disclosure::Process);
        let table = marker_of(&geometry, &Disclosure::ProcessTable);
        assert!(details.1 < table.1, "{details:?} and {table:?}");
        let rows_closed = geometry.details_rows;

        // A click on the table's marker opens the block, and the page it is
        // drawn into is longer than the page without it.
        assert_eq!(click_at(&mut app, table), None);
        assert!(app.disclosure_open(&Disclosure::ProcessTable));
        let (open, drawn) = draw(&state, &mut app, 120, 30);
        geometry = drawn;
        let open_rows: Vec<String> = open.lines().map(str::to_string).collect();
        assert!(row_has(&open_rows, &["bash", "300"]), "{open}");
        assert!(
            geometry.details_rows > rows_closed,
            "{} is not longer than {rows_closed}",
            geometry.details_rows
        );

        // The marker still answers on the longer page: it closes what it opened.
        let table = marker_of(&geometry, &Disclosure::ProcessTable);
        click_at(&mut app, table);
        assert!(!app.disclosure_open(&Disclosure::ProcessTable));
        let (closed, _) = draw(&state, &mut app, 120, 30);
        assert!(!closed.contains("esbuild"), "{closed}");
    }

    /// A key event the panel's own map answers, for the tests that move the
    /// process cursor rather than the page.
    fn press_key(app: &mut App, code: KeyCode) -> Option<Action> {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// The identity of one process the shared `read_tree` scan verified, by pid:
    /// what a cursor is asserted against, rather than a pid the kernel may hand
    /// on to another process.
    fn tree_identity(pid: i32) -> ProcessIdentity {
        read_tree()
            .members
            .into_iter()
            .find(|member| member.identity.pid == pid)
            .unwrap_or_else(|| panic!("pid {pid} is one of the tree's"))
            .identity
    }

    /// An app on the Processes page with the row's verified table open and the
    /// keyboard in the details, drawn once so the layout the pointer and the
    /// cursor are measured against exists.
    fn process_table_app(evidence: ForegroundEvidence) -> (ObservationState, App) {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", evidence);
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        app.toggle_block(&Disclosure::ProcessTable);
        press_key(&mut app, KeyCode::Tab);
        assert!(app.details_focused(), "the panel has the keyboard");
        draw(&state, &mut app, 120, 30);
        (state, app)
    }

    #[test]
    fn t_enters_process_row_navigation_only_with_the_table_open() {
        let (state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));

        // With the details focused and the table open, t names the mode where
        // the table is named.
        press_key(&mut app, KeyCode::Char('t'));
        assert!(app.process_navigating());
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(
            rows.iter().any(|row| row.contains("navigating")),
            "the mode is named where the table is: {rows:?}"
        );

        // t again leaves the mode, and the table's own label is back.
        press_key(&mut app, KeyCode::Char('t'));
        assert!(!app.process_navigating());
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(
            !rows.iter().any(|row| row.contains("navigating")),
            "{rows:?}"
        );

        // Closing the table leaves the mode: the mode is the table's.
        press_key(&mut app, KeyCode::Char('t'));
        assert!(app.process_navigating());
        app.toggle_block(&Disclosure::ProcessTable);
        assert!(!app.process_navigating());
        app.toggle_block(&Disclosure::ProcessTable);

        // With the table open but the keyboard in the tree, t is not the
        // table's, and with the table closed there is no mode to enter.
        press_key(&mut app, KeyCode::Esc);
        assert!(!app.details_focused());
        press_key(&mut app, KeyCode::Char('t'));
        assert!(!app.process_navigating(), "the tree's t is not the table's");
        press_key(&mut app, KeyCode::Tab);
        app.toggle_block(&Disclosure::ProcessTable);
        press_key(&mut app, KeyCode::Char('t'));
        assert!(!app.process_navigating(), "a closed table has no rows");
    }

    #[test]
    fn the_process_cursor_moves_over_drawn_rows_without_wrapping() {
        let (_state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let shell = tree_identity(300);
        let build = tree_identity(311);
        let editor = tree_identity(310);
        let fleet = selected_id(&app);

        press_key(&mut app, KeyCode::Char('t'));
        // The cursor starts on the row's own process, which the page draws
        // without a separate selection.
        assert_eq!(app.process_selection(), None, "the root is the start");
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.process_selection(), Some(&shell));
        press_key(&mut app, KeyCode::Down);
        assert_eq!(app.process_selection(), Some(&build));
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.process_selection(), Some(&editor));
        // Past the last drawn row the cursor stays where it is: the mode never
        // wraps.
        press_key(&mut app, KeyCode::Char('j'));
        press_key(&mut app, KeyCode::Down);
        assert_eq!(app.process_selection(), Some(&editor));
        // Home and End reach the limits, and the first row does not wrap to the
        // last either.
        press_key(&mut app, KeyCode::Home);
        assert_eq!(app.process_selection(), None);
        press_key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.process_selection(), None, "the first row does not wrap");
        press_key(&mut app, KeyCode::End);
        assert_eq!(app.process_selection(), Some(&editor));
        assert_eq!(selected_id(&app), fleet, "no row key touched the fleet");
    }

    #[test]
    fn folding_a_branch_hides_its_descendants_and_keeps_the_cursor_reachable() {
        let (state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let shell = tree_identity(300);
        let editor = tree_identity(310);

        press_key(&mut app, KeyCode::Char('t'));
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.process_selection(), Some(&shell));
        // Enter folds the shell's branch: the build beneath it goes and the
        // shell stays, drawn with the marker that says so.
        press_key(&mut app, KeyCode::Enter);
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(rows.iter().any(|row| row.contains("300")), "{rows:?}");
        assert!(
            !rows.iter().any(|row| row.contains("311")),
            "the folded branch is not drawn: {rows:?}"
        );
        // A fold is keyed by the process, not by its position: the editor is a
        // leaf, so folding it changes nothing.
        press_key(&mut app, KeyCode::End);
        assert_eq!(app.process_selection(), Some(&editor));
        press_key(&mut app, KeyCode::Char(' '));
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(rows.iter().any(|row| row.contains("310")), "{rows:?}");
        // Space opens the shell's branch again.
        press_key(&mut app, KeyCode::Home);
        press_key(&mut app, KeyCode::Char('j'));
        press_key(&mut app, KeyCode::Char(' '));
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(
            rows.iter().any(|row| row.contains("311")),
            "the branch opens again: {rows:?}"
        );
        assert!(app.process_folded().is_empty());
    }

    #[test]
    fn a_process_row_click_selects_and_enters_and_a_fold_marker_only_folds() {
        let (state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let shell = tree_identity(300);
        let fleet = selected_id(&app);
        let (_screen, geometry) = draw(&state, &mut app, 120, 30);

        // The whole row is the target: a click on it selects that process and
        // enters the table's local mode, and acts on nothing else.
        let row = geometry
            .process_rows
            .iter()
            .find(|(identity, _, _)| identity == &shell)
            .and_then(|(_, _, rect)| *rect)
            .expect("the shell's row is on screen");
        assert_eq!(click_at(&mut app, (row.right() - 1, row.y)), None);
        assert_eq!(app.process_selection(), Some(&shell));
        assert!(app.process_navigating(), "a row click enters the mode");
        assert!(app.details_focused());
        assert_eq!(selected_id(&app), fleet, "the fleet row is untouched");

        // The marker cell folds only the branch it labels: with the root's row
        // selected, folding the shell leaves the cursor on the root.
        let (_screen, geometry) = draw(&state, &mut app, 120, 30);
        press_key(&mut app, KeyCode::Home);
        assert_eq!(app.process_selection(), None);
        let marker = geometry
            .process_folds
            .iter()
            .find(|(identity, _)| identity == &shell)
            .map(|(_, rect)| *rect)
            .expect("the shell's branch has a marker");
        assert_eq!(click_at(&mut app, (marker.x, marker.y)), None);
        assert_eq!(
            app.process_selection(),
            None,
            "the marker did not select its own row"
        );
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(
            !rows.iter().any(|row| row.contains("esbuild")),
            "the marker folded its branch: {rows:?}"
        );
    }

    #[test]
    fn a_selected_process_that_vanished_or_was_recycled_returns_to_the_root() {
        let (mut state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let shell = tree_identity(300);

        press_key(&mut app, KeyCode::Char('t'));
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.process_selection(), Some(&shell));

        // The shell exits: the next observation carries no process with its
        // identity, and the compact block returns to the root rather than
        // inventing a sample for a row that is gone.
        let mut without = read_tree();
        without
            .members
            .retain(|member| member.identity.pid != 300 && member.parent.pid != 300);
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(0)),
                    Some(8 * 1024 * 1024),
                    without,
                ),
            ),
        );
        app.refresh(&state);
        assert_eq!(app.process_selection(), None, "the root is the fallback");
        assert!(app.process_folded().is_empty());

        // The kernel hands the pid on to another process: the identity is not
        // the row, so the old selection and its folds stay with the process
        // that is gone.
        let (mut state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        press_key(&mut app, KeyCode::Char('t'));
        press_key(&mut app, KeyCode::Char('j'));
        press_key(&mut app, KeyCode::Enter); // the shell's branch is folded
        assert!(!app.process_folded().is_empty());
        let mut recycled = read_tree();
        for member in &mut recycled.members {
            if member.identity.pid == 300 {
                member.identity.start_ticks += 1_000;
            }
            if member.parent.pid == 300 {
                member.parent.start_ticks += 1_000;
            }
        }
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(0)),
                    Some(8 * 1024 * 1024),
                    recycled,
                ),
            ),
        );
        app.refresh(&state);
        assert_eq!(
            app.process_selection(),
            None,
            "a recycled pid is another row"
        );
        assert!(app.process_folded().is_empty(), "no fold is inherited");
        let rows = panel_rows(&state, &mut app, DetailPage::Processes, 120, 30);
        assert!(
            rows.iter().any(|row| row.contains("311")),
            "the replacement's branch is not folded: {rows:?}"
        );
    }

    #[test]
    fn another_selection_drops_the_process_cursor_and_its_folds() {
        let (state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let shell = tree_identity(300);
        press_key(&mut app, KeyCode::Char('t'));
        press_key(&mut app, KeyCode::Char('j'));
        press_key(&mut app, KeyCode::Enter);
        assert_eq!(app.process_selection(), Some(&shell));
        assert!(!app.process_folded().is_empty());

        // Another row is another process: the cursor, the mode and the folds
        // are the row's, and are not carried onto the row that is selected
        // next.
        select_agent(&mut app, "wA:p1");
        assert!(!app.process_navigating());
        assert_eq!(app.process_selection(), None);
        assert!(app.process_folded().is_empty());
        assert!(
            app.process_table_on_page()
                .is_none_or(|table| table.root != shell)
        );
        let _ = state;
    }

    #[test]
    fn process_navigation_issues_no_fleet_lifecycle_or_mux_action() {
        let (_state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let fleet = selected_id(&app);
        press_key(&mut app, KeyCode::Char('t'));

        for code in [
            KeyCode::Char('j'),
            KeyCode::Down,
            KeyCode::Char('k'),
            KeyCode::Up,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Enter,
            KeyCode::Char(' '),
        ] {
            assert_eq!(press_key(&mut app, code), None, "{code:?} acted");
        }
        // The page's lifecycle keys are the panel's either way: they open no
        // confirmation and focus no pane.
        for code in [KeyCode::Char('x'), KeyCode::Char('X'), KeyCode::Char('r')] {
            assert_eq!(press_key(&mut app, code), None, "{code:?} is the panel's");
        }
        assert!(app.confirmation().is_none());
        assert_eq!(selected_id(&app), fleet, "no key moved the fleet");
        assert!(app.details_focused(), "the panel kept the keyboard");
    }

    #[test]
    fn the_cursor_is_scrolled_into_sight_and_the_targets_follow_a_resize() {
        let (state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        press_key(&mut app, KeyCode::Char('t'));

        // A panel shorter than the page: End puts the cursor on the last row
        // and the page scrolls the least that draws it.
        let (_screen, geometry) = draw(&state, &mut app, 120, 12);
        assert!(
            geometry
                .process_rows
                .iter()
                .any(|(_, _, rect)| rect.is_none()),
            "the page is longer than the panel"
        );
        press_key(&mut app, KeyCode::End);
        // Selecting a descendant adds its own compact block to the page, so the
        // panel scrolls against the page the selection produced: the move draws
        // once, and the reveal is applied from the rows that draw reported.
        draw(&state, &mut app, 120, 12);
        let (_screen, geometry) = draw(&state, &mut app, 120, 12);
        let last = geometry.process_rows.last().expect("the table has rows");
        assert!(last.2.is_some(), "the cursor's row is drawn: {geometry:?}");
        assert!(last.1 > 0, "the page scrolled to it");

        // A resize moves every row; the targets are the rectangles this draw
        // reported, not the ones the last layout had.
        let (screen, resized) = draw(&state, &mut app, 60, 30);
        assert!(
            resized
                .process_rows
                .iter()
                .all(|(_, _, rect)| rect.is_some()),
            "every row is on screen after the resize: {screen}"
        );
        let known: Vec<ProcessIdentity> = resized
            .process_rows
            .iter()
            .map(|(identity, _, _)| identity.clone())
            .collect();
        let shell = tree_identity(300);
        let row = resized
            .process_rows
            .iter()
            .find(|(identity, _, _)| identity == &shell)
            .and_then(|(_, _, rect)| *rect)
            .expect("the shell's row is drawn after the resize");
        assert!(known.contains(&shell));
        assert_eq!(click_at(&mut app, (row.right() - 1, row.y)), None);
        assert_eq!(app.process_selection(), Some(&shell));
    }

    #[test]
    fn a_refresh_leaves_no_stale_process_pointer_target() {
        let (mut state, mut app) = process_table_app(sampled_tree(BinaryIdentity::default()));
        let build = tree_identity(311);
        let fleet = selected_id(&app);
        let (_screen, geometry) = draw(&state, &mut app, 120, 30);
        let build_row = geometry
            .process_rows
            .iter()
            .find(|(identity, _, _)| identity == &build)
            .and_then(|(_, _, rect)| *rect)
            .expect("the build's row is on screen");

        // The scan no longer verifies the build, and the row it was drawn in is
        // another process now: the click answers to what is drawn there, never
        // to the identity that was.
        let mut without = read_tree();
        without.members.retain(|member| member.identity.pid != 311);
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(0)),
                    Some(8 * 1024 * 1024),
                    without,
                ),
            ),
        );
        app.refresh(&state);
        draw(&state, &mut app, 120, 30);
        assert_eq!(
            click_at(&mut app, (build_row.right() - 1, build_row.y)),
            None
        );
        assert_ne!(
            app.process_selection(),
            Some(&build),
            "the vanished row answered a click"
        );
        assert_eq!(selected_id(&app), fleet, "the fleet is untouched");
    }

    #[test]
    fn every_kernel_state_is_one_short_name() {
        // The value that changes while a reader watches is one word, never a
        // sentence about what the scheduler is doing: a changing sentence reads
        // as a changing claim.
        for (state, word) in [
            (ProcessState::Running, "running"),
            (ProcessState::Sleeping, "sleeping"),
            (ProcessState::DiskSleep, "disk wait"),
            (ProcessState::Stopped, "stopped"),
            (ProcessState::TracingStop, "traced"),
            (ProcessState::Zombie, "zombie"),
            (ProcessState::Dead, "dead"),
            (ProcessState::Idle, "idle"),
            (ProcessState::Other('W'), "unknown (W)"),
        ] {
            let rendered = state_word(state);
            assert_eq!(rendered, word, "{state:?}");
            assert!(
                rendered
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || " ()".contains(ch)),
                "{rendered:?} is a value, not a sentence"
            );
        }
    }

    /// A sampled process that also carries a binary comparison, so a page can
    /// be read with both its measurements and its identity.
    fn sampled_binary(
        pid: i32,
        resources: ProcessResources,
        name: &str,
        identity: BinaryIdentity,
    ) -> ForegroundEvidence {
        let mut evidence = sampled(pid, resources);
        if let ForegroundEvidence::NonShell {
            name: reported,
            local,
            ..
        } = &mut evidence
        {
            *reported = Some(name.to_string());
            local.binary = identity;
        }
        evidence
    }

    #[test]
    fn the_metric_sections_stay_compact_in_a_narrow_panel() {
        // A first sample — no interval for the CPU — over descendants whose
        // resident set is only a lower bound: both kinds of qualification are
        // drawn on the narrowest panel the side-by-side layout draws.
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    None,
                    Some(7 * 1024 * 1024),
                    DescendantResources {
                        members: Vec::new(),
                        observed: Some(2),
                        rss_bytes: Total::Partial(
                            1_024,
                            "a descendant exited while the table was read".into(),
                        ),
                        cpu: Total::Complete(CpuPercent::from_hundredths(11_820)),
                    },
                ),
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);

        // 28 cells of content: each section is its heading and short labelled
        // values, every value drawn whole.
        let (screen, _) = draw(&state, &mut app, 90, 30);
        assert!(screen.contains("process"), "{screen}");
        assert!(screen.contains("state  sleeping"), "{screen}");
        assert!(screen.contains("cpu  unavailable"), "{screen}");
        assert!(screen.contains("rss  7.0 MiB"), "{screen}");
        assert!(screen.contains("descendants"), "{screen}");
        assert!(screen.contains("count  2 processes"), "{screen}");
        assert!(screen.contains("cpu  118.2%"), "{screen}");
        assert!(screen.contains("rss  ≥1.0 KiB"), "{screen}");

        // A reason is drawn on its own row, under the column the values are
        // drawn in and not under the labels: the values stay comparable with
        // the row above, and the reason reads as a reason.
        let rows: Vec<&str> = screen.lines().collect();
        for (value, reason) in [
            ("unavailable", "no interval of this"),
            ("≥1.0 KiB", "a descendant exited"),
        ] {
            let value_row = rows
                .iter()
                .find(|row| row.contains(value))
                .unwrap_or_else(|| panic!("no row draws {value:?}: {screen}"));
            let reason_row = rows
                .iter()
                .find(|row| row.contains(reason))
                .unwrap_or_else(|| panic!("no row draws {reason:?}: {screen}"));
            assert_eq!(
                reason_row.find(reason),
                value_row.find(value),
                "the reason is drawn under its value: {screen}"
            );
        }

        // Two facts that fit together share the line even here: the horizontal
        // space is what the panel is being spent on, and a fact with no reason
        // costs nothing but its own width.
        let mut measured = fixture_state();
        measured.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(1_240)),
                    Some(86 * 1024 * 1024),
                    no_descendants(),
                ),
            ),
        );
        let mut app = app_for(&measured);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        let (screen, _) = draw(&measured, &mut app, 90, 30);
        assert!(screen.contains("cpu  12.4%  rss  86.0 MiB"), "{screen}");
        assert!(screen.contains("count  0 processes"), "{screen}");
    }

    #[test]
    fn the_pages_rows_are_the_rows_the_panel_draws() {
        // The panel measures each page line on its own and adds them up, so the
        // sum must be the rows of the whole page: a word too long to break, a
        // run of spaces and an empty line are where a count taken from the text
        // rather than from the widget would disagree.
        let lines = vec![
            Line::from(""),
            Line::from(format!("a very long identifier {}", "x".repeat(60))),
            Line::from("two  spaces  between  words"),
            Line::from(""),
            Line::from("short"),
        ];
        for width in [8u16, 12, 20, 40] {
            let (total, offsets) = page_rows(&lines, width);
            let whole = Paragraph::new(lines.clone())
                .wrap(PAGE_WRAP)
                .line_count(width);
            assert_eq!(total, whole, "the rows of the page at width {width}");
            assert_eq!(offsets.len(), lines.len(), "one row per line to start on");
            assert_eq!(offsets[0], 0, "the page starts at its first row");
            assert!(
                offsets.windows(2).all(|pair| pair[0] < pair[1]),
                "every line takes a row of its own: {offsets:?} at width {width}"
            );
        }
    }

    #[test]
    fn the_end_of_a_wrapped_page_is_reachable_on_a_small_panel() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(0)),
                    Some(8 * 1024 * 1024),
                    DescendantResources {
                        members: Vec::new(),
                        observed: Some(2),
                        rss_bytes: Total::Complete(512 * 1024 * 1024),
                        cpu: Total::Complete(CpuPercent::from_hundredths(12_500)),
                    },
                ),
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);

        // A panel with room for less than the page. The metric lines wrap, so
        // the page occupies more rows than it has lines, and End reaches its
        // last row rather than stopping at the last one that starts on it. The
        // block is open, which is what makes the page that much longer.
        app.toggle_block(&Disclosure::Process);
        let (_, short) = draw(&state, &mut app, 120, 12);
        assert!(
            short.details_rows > short.details_viewport as usize,
            "the panel has more to show than it fits: {short:?}"
        );
        let max = (short.details_rows - short.details_viewport as usize) as u16;
        app.scroll_page_to(u16::MAX);
        assert_eq!(app.details_scroll(), max, "the clamp counts drawn rows");
        let (scrolled, _) = draw(&state, &mut app, 120, 12);
        // The page ends in the descendant sums, and End reaches that row
        // however far the block and the wraps pushed it down.
        assert!(
            scrolled.contains("512.0 MiB"),
            "the last row of the page is drawn: {scrolled}"
        );
        // The end is the end: a further scroll key does not move the page.
        app.scroll_page(3);
        assert_eq!(app.details_scroll(), max);

        // Narrow enough that the panel stacks under the fleet, and wraps the
        // same page further still: every row stays reachable there too.
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        app.toggle_block(&Disclosure::Process);
        draw(&state, &mut app, 46, 20);
        let (_, narrow) = draw(&state, &mut app, 46, 20);
        app.scroll_page_to(u16::MAX);
        assert_eq!(
            app.details_scroll(),
            (narrow.details_rows - narrow.details_viewport as usize) as u16
        );
        let (scrolled, _) = draw(&state, &mut app, 46, 20);
        assert!(
            scrolled.contains("512.0 MiB"),
            "the last row is drawn on a stacked panel: {scrolled}"
        );

        // The values are on the page, not only in its last rows.
        let page = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        assert!(page.contains("cpu  125.0%  rss  512.0 MiB"), "{page}");
    }

    #[test]
    fn a_live_stale_agent_is_marked_and_names_both_installations() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", binary_evidence(stale_identity()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 34);

        let mark = theme::stale_mark();
        assert!(screen.contains(&format!("{mark} worker task")), "{screen}");
        // The details name the process, the executable within its package and
        // the verdict: a mark, the word, and the installed target as compactly
        // as the package line names the running one. Both roots are the block's.
        let page = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(page.contains("observed: pi"), "{page}");
        assert!(page.contains("package: /run/pi-1.0.1"), "{page}");
        // A wrapper outside the package it starts is drawn where it is: there
        // is no root to cut it against, and no store prefix to repeat either.
        assert!(page.contains("exe: /run/pi"), "{page}");
        assert!(
            page.contains(&format!(
                "binary: {} (stale) installed pi-1.0.2",
                theme::stale_mark()
            )),
            "{page}"
        );
        assert!(!page.contains("/nix/store/aaa-pi-1.0.2"), "{page}");
        app.toggle_block(&Disclosure::Process);
        let page = panel_text(&state, &mut app, DetailPage::Processes);
        // The roots share no prefix down to a component boundary, so each is
        // drawn whole where it lives rather than cut against the other.
        assert!(page.contains("/run/pi-1.0.1"), "{page}");
        assert!(page.contains("/nix/store/aaa-pi-1.0.2"), "{page}");
        assert!(page.contains("exe"), "{page}");
        assert!(page.contains("/run/pi"), "{page}");

        // The mark wears the configured stale role, not a hardcoded colour.
        let mut terminal = Terminal::new(TestBackend::new(180, 34)).expect("infallible");
        terminal
            .draw(|frame| {
                render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let cell = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == mark)
            .expect("the stale mark is drawn");
        assert_eq!(cell.style().fg, Some(theme::palette().stale));
    }

    #[test]
    fn a_readable_other_build_is_unmarked_and_says_it_is_not_installed() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            binary_evidence(BinaryIdentity {
                freshness: BinaryFreshness::Current,
                running: Some("/home/dev/pi".into()),
                installed: Some("/nix/store/aaa-pi-1.0.2".into()),
                executable: Some("/home/dev/pi".into()),
                unknown: None,
            }),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 34);

        assert!(
            !screen.contains(&format!("{} worker task", theme::stale_mark())),
            "{screen}"
        );
        // The running package is named, and the verdict says what the
        // difference is: another build, not a replacement. Neither root is on
        // the line a reader scans; both are in the block.
        let page = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(page.contains("package: /home/dev/pi"), "{page}");
        assert!(page.contains("binary: other build"), "{page}");
        assert!(!page.contains("/nix/store/aaa-pi-1.0.2"), "{page}");
        assert!(!page.contains("current"), "{page}");
        app.toggle_block(&Disclosure::Process);
        let page = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(page.contains("/home/dev/pi"), "{page}");
        assert!(page.contains("/nix/store/aaa-pi-1.0.2"), "{page}");
    }

    #[test]
    fn a_current_comparison_stays_readable_without_a_mark() {
        // A running executable that is the installed one has no warning to
        // give, and no verdict either: the page names the process and the
        // package once, with the executable inside it.
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            binary_evidence(BinaryIdentity {
                freshness: BinaryFreshness::Current,
                running: Some("/nix/store/aaa-pi-1.0.2".into()),
                installed: Some("/nix/store/aaa-pi-1.0.2".into()),
                executable: Some("/nix/store/aaa-pi-1.0.2/lib/pi/pi".into()),
                unknown: None,
            }),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        assert!(processes.contains("observed: pi"), "{processes}");
        assert!(processes.contains("package: pi-1.0.2"), "{processes}");
        assert!(processes.contains("exe: lib/pi/pi"), "{processes}");
        // One package, named once, and nothing claiming a verdict: a current
        // comparison says so by having no mark and no words.
        assert!(!processes.contains("/nix/store/"), "{processes}");
        assert!(!processes.contains("binary:"), "{processes}");
        assert!(!processes.contains("current"), "{processes}");
        app.toggle_block(&Disclosure::Process);
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        // One root, named once, and the executable inside it: the comparison
        // named the same root on both sides, so there is nothing to tell apart.
        assert!(
            draws_row(&processes, "root", "/nix/store/aaa-pi-1.0.2"),
            "{processes}"
        );
        assert!(draws_row(&processes, "exe", "lib/pi/pi"), "{processes}");
        let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 34);
        assert!(
            !screen.contains(&format!("{} worker task", theme::stale_mark())),
            "{screen}"
        );
    }

    #[test]
    fn an_unknown_comparison_states_why_it_could_not_be_made() {
        // One wording per typed reason, and no mark whatever the reason: an
        // unknown comparison is never claimed either way. A running file that
        // was read stays nameable beside the reason it could not be used.
        for (reason, executable, word, sentence) in [
            (
                BinaryUnknown::NoCounterpart,
                Some("/nix/store/aaa-nvim/bin/nvim"),
                "no counterpart",
                "unknown — no installed counterpart resolves",
            ),
            (
                BinaryUnknown::UnsupportedLauncher,
                Some("/nix/store/aaa-pi-bolt/bin/pi-bolt"),
                "payload unknown",
                "unknown — the installed counterpart's payload could not be identified",
            ),
            (
                BinaryUnknown::Unreadable,
                None,
                "unreadable",
                "unknown — the running executable could not be read",
            ),
            (
                BinaryUnknown::NotCompared,
                Some("/nix/store/aaa-nvim/bin/nvim"),
                "not the named program",
                "unknown — the running file is not the program the runtime named",
            ),
        ] {
            let mut state = fixture_state();
            state.apply_evidence(
                "wA:p2",
                binary_evidence(BinaryIdentity {
                    executable: executable.map(str::to_string),
                    unknown: Some(reason),
                    ..BinaryIdentity::default()
                }),
            );
            let mut app = app_for(&state);
            select_agent(&mut app, "wA:p2");
            let processes = panel_text(&state, &mut app, DetailPage::Processes);
            assert!(processes.contains(word), "{reason:?}: {processes}");
            assert!(!processes.contains(sentence), "{reason:?}: {processes}");
            // The reason in full, and the path it is about, are the block's.
            app.toggle_block(&Disclosure::Process);
            let open = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
            let sentence = sentence.trim_start_matches("unknown — ");
            assert!(draws_row(&open, "unknown", sentence), "{reason:?}: {open}");
            match executable {
                Some(path) => assert!(open.contains(path), "{reason:?}: {open}"),
                // An unreadable link has no path to name, so neither the
                // executable nor an identity is drawn.
                None => assert!(!open.contains("exe:"), "{reason:?}: {open}"),
            }
            let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 34);
            assert!(
                !screen.contains(&format!("{} worker task", theme::stale_mark())),
                "{reason:?}: {screen}"
            );
        }

        // An identity nothing was compared against and which carries no reason
        // has nothing to state, so it claims nothing.
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", binary_evidence(BinaryIdentity::default()));
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(!processes.contains("exe:"), "{processes}");
        assert!(!processes.contains("binary:"), "{processes}");
    }

    #[test]
    fn an_ordinary_pane_shows_the_same_process_identity_as_an_agent() {
        // One seam draws both rows: a pane with a process in it gets the
        // executable and the comparison as an agent row does.
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p3",
            named_binary_evidence(
                "pi-bolt",
                "pi-bolt --approve",
                BinaryIdentity {
                    freshness: BinaryFreshness::Stale,
                    running: Some("/nix/store/aaa-pi-bolt-0.7.1".into()),
                    installed: Some("/nix/store/bbb-pi-bolt-0.7.1".into()),
                    executable: Some("/nix/store/aaa-pi-bolt-0.7.1/lib/pi-bolt/pi".into()),
                    unknown: None,
                },
            ),
        );
        let mut app = app_for(&state);
        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        // Wide enough that the executable's own path is asserted as one token.
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        assert!(processes.contains("observed: pi-bolt"), "{processes}");
        // The package once, with the hash that tells the two builds of this
        // version apart, the executable inside it and a marked verdict.
        assert!(
            processes.contains("package: pi-bolt-0.7.1 aaa"),
            "{processes}"
        );
        assert!(processes.contains("exe: lib/pi-bolt/pi"), "{processes}");
        assert!(
            processes.contains(&format!(
                "binary: {} (stale) installed pi-bolt-0.7.1 bbb",
                theme::stale_mark()
            )),
            "{processes}"
        );
        assert!(!processes.contains("/nix/store/"), "{processes}");
        // The store the two roots sit in is stated once, each root carries the
        // part that names its build, and the executable is named inside the
        // running one: three rows, three pieces of one path, nothing repeated.
        app.toggle_block(&Disclosure::Process);
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        assert!(draws_row(&processes, "store", "/nix/store/"), "{processes}");
        assert!(
            draws_row(&processes, "running", "aaa-pi-bolt-0.7.1"),
            "{processes}"
        );
        assert!(
            draws_row(&processes, "installed", "bbb-pi-bolt-0.7.1"),
            "{processes}"
        );
        assert!(
            draws_row(&processes, "exe", "lib/pi-bolt/pi"),
            "{processes}"
        );
    }

    #[test]
    fn the_process_block_holds_what_the_page_draws_short() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled_binary(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(1_240)),
                    Some(86 * 1024 * 1024),
                    DescendantResources {
                        members: Vec::new(),
                        observed: Some(2),
                        rss_bytes: Total::Partial(
                            410 * 1024 * 1024,
                            "a descendant exited while the table was read".into(),
                        ),
                        cpu: Total::Complete(CpuPercent::from_hundredths(11_820)),
                    },
                ),
                "pi-bolt",
                BinaryIdentity {
                    freshness: BinaryFreshness::Stale,
                    running: Some("/nix/store/aaa-pi-bolt-0.7.1".into()),
                    installed: Some("/nix/store/bbb-pi-bolt-0.7.1".into()),
                    executable: Some("/nix/store/aaa-pi-bolt-0.7.1/lib/pi-bolt/pi".into()),
                    unknown: None,
                },
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);

        // The narrowest panel the side-by-side layout draws, and a wide one:
        // what a reader scans is the package once, the executable inside it and
        // a marked verdict. No store prefix is repeated, no incarnation, no
        // meaning and no sentence, and nothing claims the running build is the
        // installed one.
        for width in [90, 120] {
            let closed = panel_text_at(&state, &mut app, DetailPage::Processes, width);
            assert!(
                closed.contains("package: pi-bolt-0.7.1 aaa"),
                "{width}: {closed}"
            );
            assert!(closed.contains("exe: lib/pi-bolt/pi"), "{width}: {closed}");
            assert!(
                closed.contains(&format!(
                    "binary: {} (stale) installed pi-bolt-0.7.1 bbb",
                    theme::stale_mark()
                )),
                "{width}: {closed}"
            );
            assert!(!closed.contains("/nix/store/"), "{width}: {closed}");
            assert!(!closed.contains("boot "), "{width}: {closed}");
            assert!(!closed.contains("start ticks"), "{width}: {closed}");
            assert!(
                !closed.contains("waiting, but wakeable"),
                "{width}: {closed}"
            );
            assert!(!closed.contains("assignment total"), "{width}: {closed}");
            assert!(!closed.contains("current"), "{width}: {closed}");
            assert!(closed.contains("▸ details"), "{width}: {closed}");
        }

        // Everything the page drew short is one keypress away: the store both
        // roots sit in, the part of each that names its build, the executable
        // inside the running one, the birth the reading belongs to, what the
        // state it is showing means, and what a sum is. The other kernel states
        // are the kernel's, and a list of them described is a paragraph where a
        // fact belongs.
        app.toggle_block(&Disclosure::Process);
        let open = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        for fact in [
            "store     /nix/store/",
            "running   aaa-pi-bolt-0.7.1",
            "installed bbb-pi-bolt-0.7.1",
            "exe       lib/pi-bolt/pi",
            "birth     pid 4242 · start ticks 9812",
            "boot      6d9d2f0a-2f6f-4a1f-9c2d",
            "state     sleeping — waiting, but wakeable",
            "observed processes beneath this one",
            "counts in each)",
        ] {
            assert!(open.contains(fact), "{fact:?} is not in the block: {open}");
        }
        assert!(!open.contains("zombie"), "{open}");
    }

    #[test]
    fn a_word_wider_than_the_column_breaks_where_it_can_be_read() {
        // A store path breaks at a store and a boot id at a hyphen, rather than
        // inside a hash.
        assert_eq!(
            wrap_value("see /nix/store/aaa-pi-1.0.2", 12),
            vec!["see", "/nix/store/", "aaa-pi-1.0.2"]
        );
        assert_eq!(
            wrap_value("6d9d2f0a-2f6f-4a1f-9c2d-2f6f4a1f9c2d", 25),
            vec!["6d9d2f0a-2f6f-4a1f-9c2d-", "2f6f4a1f9c2d"]
        );
        // A run of characters with nothing to break at still gets columns.
        assert_eq!(wrap_value("aaaaaaaa", 3), vec!["aaa", "aaa", "aa"]);
    }

    #[test]
    fn the_block_draws_facts_beside_their_labels_and_hangs_what_wraps() {
        // The reader's complaints, each one an assertion: the store prefix
        // repeated on every row, values stacked under their labels, a wrapped
        // fact landing under the label instead of its value, and a glossary of
        // states the row is not in.
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            sampled_binary(
                4242,
                resources(
                    Some(CpuPercent::from_hundredths(1_240)),
                    Some(86 * 1024 * 1024),
                    DescendantResources {
                        members: Vec::new(),
                        observed: Some(2),
                        rss_bytes: Total::Partial(
                            410 * 1024 * 1024,
                            "a descendant exited while the table was read".into(),
                        ),
                        cpu: Total::Complete(CpuPercent::from_hundredths(11_820)),
                    },
                ),
                "pi-bolt",
                BinaryIdentity {
                    freshness: BinaryFreshness::Stale,
                    running: Some("/nix/store/aaa-pi-bolt-0.7.1".into()),
                    installed: Some("/nix/store/bbb-pi-bolt-0.7.1".into()),
                    executable: Some("/nix/store/aaa-pi-bolt-0.7.1/lib/pi-bolt/pi".into()),
                    unknown: None,
                },
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        app.toggle_block(&Disclosure::Process);

        // One store, named once: each root is the part past it.
        let open = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        assert_eq!(open.matches("/nix/store/").count(), 1, "{open}");
        // Every fact beside its label, in one column.
        for row in [
            "store     /nix/store/",
            "running   aaa-pi-bolt-0.7.1",
            "installed bbb-pi-bolt-0.7.1",
            "exe       lib/pi-bolt/pi",
            "state     sleeping — waiting, but wakeable",
        ] {
            assert!(open.contains(row), "{row:?} is not a row: {open}");
        }
        // The meanings of the states this row is not in are the kernel's.
        for absent in ["zombie", "disk wait", "tracing", "reaped"] {
            assert!(!open.contains(absent), "{absent:?}: {open}");
        }

        // A fact too long for the panel is continued under the value column
        // rather than under the label it belongs to.
        let screen = render_page(&state, &mut app, DetailPage::Processes, 90, 34);
        assert!(
            screen
                .lines()
                .any(|line| line.contains("│          beneath this one")),
            "{screen}"
        );
    }

    /// The selected row is a live process: it offers one block on the Processes
    /// page, drawn as the block a marker opens.
    fn assert_process_block(state: &ObservationState, case: &str) {
        let mut app = app_for(state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        assert_eq!(
            app.disclosures(),
            vec![Disclosure::Process],
            "{case}: the page's blocks"
        );
        let page = panel_text(state, &mut app, DetailPage::Processes);
        assert!(page.contains("▸ details"), "{case}: {page}");
    }

    /// The same row with no live process: nothing to hold behind a marker, and
    /// no marker answering to nothing.
    fn assert_no_process_block(state: &ObservationState, case: &str) {
        let mut app = app_for(state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        assert!(app.disclosures().is_empty(), "{case}: a block is offered");
        let page = panel_text(state, &mut app, DetailPage::Processes);
        assert!(!page.contains("details"), "{case}: {page}");
    }

    #[test]
    fn the_process_block_is_offered_only_where_a_process_is_drawn() {
        // A shell, an inconclusive foreground and a pane nothing was read for
        // name no process, so the page has nothing verbose to hold.
        for (evidence, case) in [
            (Some(ForegroundEvidence::Shell), "a shell"),
            (
                Some(ForegroundEvidence::Inconclusive),
                "an inconclusive foreground",
            ),
            (None, "no evidence"),
        ] {
            let mut state = fixture_state();
            if let Some(evidence) = evidence {
                state.apply_evidence("wA:p2", evidence);
            }
            assert_no_process_block(&state, case);
        }
        // A retained row draws no process facts at all.
        assert_no_process_block(&retained_working_state(), "a retained row");
        // Nor does a last-good row of a stale inventory, which draws the
        // process withheld rather than as live.
        let mut stale = fixture_state();
        stale.apply_evidence("wA:p2", binary_evidence(stale_identity()));
        stale.apply_failure("herdr exited with status 1");
        assert_no_process_block(&stale, "a stale source");
        // And a row that does name one offers exactly that block.
        let mut live = fixture_state();
        live.apply_evidence("wA:p2", binary_evidence(stale_identity()));
        assert_process_block(&live, "a live process");
    }

    #[test]
    fn the_process_block_opens_from_the_page_at_either_width() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", binary_evidence(stale_identity()));
        // A click on the marker, and Enter from the keyboard the page's own tab
        // hands the panel: both toggle the one block, at either panel width.
        for width in [120, 90] {
            let mut app = app_for(&state);
            select_agent(&mut app, "wA:p2");
            app.select_page(DetailPage::Processes);
            draw(&state, &mut app, width, 30);
            let (_, mut geometry) = draw(&state, &mut app, width, 30);
            let marker = marker_of(&geometry, &Disclosure::Process);
            let details = geometry.details.expect("the panel is drawn");
            assert!(is_inside(details, marker), "{width}: {marker:?}");
            assert!(!app.disclosure_open(&Disclosure::Process));

            assert_eq!(click_at(&mut app, marker), None);
            assert!(app.disclosure_open(&Disclosure::Process));
            let (open, drawn) = draw(&state, &mut app, width, 30);
            geometry = drawn;
            // The block's own rows are what says the body is drawn, at either
            // width: a root long enough to wrap is still the same row.
            assert!(open.contains("running"), "{width}: {open}");
            assert!(
                draws_row(&open, "installed", "/nix/store/aaa-pi-"),
                "{width}: {open}"
            );

            let tab = geometry.detail_tabs[DetailPage::Processes.index()].expect("the tab");
            click_at(&mut app, (tab.x, tab.y));
            assert!(app.details_focused());
            let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.handle_key(key), None);
            assert!(!app.disclosure_open(&Disclosure::Process));
            let (closed, _) = draw(&state, &mut app, width, 30);
            assert!(!closed.contains("running"), "{width}: {closed}");
            assert!(closed.contains("▸ details"), "{width}: {closed}");
        }
    }

    #[test]
    fn the_observed_name_is_no_invocation_and_its_arguments_are_not_drawn() {
        // A bolt payload is exec-replaced, so the runtime reports the payload's
        // own name: the page says what was observed, never what the pane was
        // started with. Its arguments can carry a prompt, so none are drawn.
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            named_binary_evidence(
                "pi\u{7}",
                "/nix/store/aaa\u{7}-pi-bolt-0.7.1/lib/pi-bolt/pi --approve \
                 --system-prompt You are a helpful assistant \
                 --append-system-prompt Task: do the thing",
                BinaryIdentity {
                    freshness: BinaryFreshness::Current,
                    running: Some("/nix/store/aaa-pi-bolt-0.7.1".into()),
                    installed: Some("/nix/store/aaa-pi-bolt-0.7.1".into()),
                    executable: Some("/nix/store/aaa\u{7}-pi-bolt-0.7.1/lib/pi-bolt/pi".into()),
                    unknown: None,
                },
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        // Wide enough that the executable's own path is asserted as one token.
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);

        // The name and the path are shown sanitized, and the path is drawn
        // inside the package that carries it.
        assert!(processes.contains("observed: pi"), "{processes}");
        assert!(processes.contains("exe: lib/pi-bolt/pi"), "{processes}");
        // No argument reaches the page — above all not a prompt.
        for argument in [
            "--approve",
            "--system-prompt",
            "--append-system-prompt",
            "You are a helpful assistant",
            "Task: do the thing",
        ] {
            assert!(
                !processes.contains(argument),
                "{argument:?} is an argument, not the process: {processes}"
            );
        }
        // And how the executable's own path was cut into the rows: the root it
        // lives in, then the file's place under that root.
        app.toggle_block(&Disclosure::Process);
        let processes = panel_text_at(&state, &mut app, DetailPage::Processes, 200);
        assert!(
            draws_row(&processes, "root", "/nix/store/aaa-pi-bolt-0.7.1"),
            "{processes}"
        );
        assert!(
            draws_row(&processes, "exe", "lib/pi-bolt/pi"),
            "{processes}"
        );
        // And nothing on the page claims to be what started the pane.
        for word in ["invocation", "launcher", "alias", "started with"] {
            assert!(!processes.contains(word), "{word:?}: {processes}");
        }
    }

    #[test]
    fn a_retained_shell_or_inconclusive_row_draws_no_process_identity() {
        // A retained row's process is gone; a shell or an inconclusive answer
        // names none. None of them may borrow the identity fields.
        let mut retained = fixture_state();
        let mut without_worker = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        without_worker
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p2");
        retained.apply_success(without_worker);
        retained.apply_evidence("wA:p2", ForegroundEvidence::Shell);
        let mut app = app_for(&retained);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&retained, &mut app, DetailPage::Processes);
        assert!(processes.contains("withheld"), "{processes}");
        assert!(!processes.contains("observed:"), "{processes}");
        assert!(!processes.contains("exe:"), "{processes}");
        assert!(!processes.contains("binary:"), "{processes}");

        // A shell and an inconclusive answer say what the foreground is, and
        // claim no process.
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", ForegroundEvidence::Shell);
        state.apply_evidence("wA:p3", ForegroundEvidence::Inconclusive);
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(
            processes.contains("foreground: shell — nothing in the foreground"),
            "{processes}"
        );
        assert!(!processes.contains("observed:"), "{processes}");

        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        let processes = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(
            processes.contains("foreground: unknown — PID fields disagree"),
            "{processes}"
        );
        assert!(!processes.contains("observed:"), "{processes}");
        assert!(!processes.contains("binary:"), "{processes}");
    }

    #[test]
    fn a_stale_source_does_not_present_binary_freshness_as_current() {
        let mut state = fixture_state();
        state.apply_evidence("wA:p2", binary_evidence(stale_identity()));
        // The last-good inventory and its foreground evidence survive a failed
        // collection; neither may be presented as a fresh comparison.
        state.apply_failure("herdr exited with status 1");
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let source = render_page(&state, &mut app, DetailPage::Source, 180, 34);
        assert!(source.contains("source: stale"), "{source}");

        // The process, its executable and the comparison are withheld, not
        // drawn from the last-good evidence.
        let processes = render_page(&state, &mut app, DetailPage::Processes, 180, 34);
        assert!(
            processes.contains("withheld — the source is not current"),
            "{processes}"
        );
        assert!(!processes.contains("observed:"), "{processes}");
        assert!(!processes.contains("exe:"), "{processes}");
        assert!(!processes.contains("binary:"), "{processes}");
        assert!(
            !processes.contains(&format!("{} worker task", theme::stale_mark())),
            "{processes}"
        );
    }

    #[test]
    fn live_foreground_evidence_names_its_pid_on_an_agent_and_on_a_pane() {
        let mut state = fixture_state();
        state.apply_evidence(
            "wA:p2",
            ForegroundEvidence::command(4242, Some("pi".into()), Some("pi".into())),
        );
        state.apply_evidence(
            "wA:p3",
            ForegroundEvidence::command(7, Some("nvim".into()), Some("nvim notes.md".into())),
        );
        let mut app = app_for(&state);

        select_agent(&mut app, "wA:p2");
        let agent = render_text(&state, &app, 180, 34);
        assert!(agent.contains("pid: 4242"), "{agent}");

        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        // Overview names the PID beside the location; Processes carries the
        // command that PID is running.
        let overview = render_text(&state, &app, 180, 30);
        assert!(overview.contains("pid: 7"), "{overview}");
        let processes = render_page(&state, &mut app, DetailPage::Processes, 180, 30);
        assert!(processes.contains("observed: nvim"), "{processes}");
        assert!(processes.contains("pid: 7"), "{processes}");
    }

    #[test]
    fn absent_inconclusive_or_stale_evidence_names_no_pid() {
        // Nothing was asked about the pane: there is no process to name, on
        // any page that could name one.
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let screen = render_pages(&state, &mut app, 180, 34);
        assert!(!screen.contains("pid:"), "{screen}");

        // A shell is not a foreground process, and disagreeing PID fields
        // name none either.
        let mut shell = fixture_state();
        shell.apply_evidence("wA:p2", ForegroundEvidence::Shell);
        shell.apply_evidence("wA:p3", ForegroundEvidence::Inconclusive);
        let mut app = app_for(&shell);
        select_agent(&mut app, "wA:p2");
        let screen = render_pages(&shell, &mut app, 180, 34);
        assert!(!screen.contains("pid:"), "{screen}");
        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        let screen = render_pages(&shell, &mut app, 180, 30);
        assert!(!screen.contains("pid:"), "{screen}");

        // A stale source's last-good evidence is not a process known to be
        // running now.
        let mut stale = fixture_state();
        stale.apply_evidence(
            "wA:p2",
            ForegroundEvidence::command(4242, Some("pi".into()), Some("pi".into())),
        );
        stale.apply_failure("herdr exited with status 1");
        let mut app = app_for(&stale);
        select_agent(&mut app, "wA:p2");
        let screen = render_pages(&stale, &mut app, 180, 34);
        assert!(screen.contains("source: stale"), "{screen}");
        assert!(!screen.contains("pid:"), "{screen}");
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

    #[test]
    fn a_confirmation_names_the_target_and_records_its_drawn_buttons() {
        let state = fixture_state();
        let mut app = app_for(&state);
        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        app.begin_action(crate::app::Operation::ClosePane, &state);
        assert!(
            app.confirmation().is_some(),
            "an unmanaged pane is eligible"
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("infallible");
        let mut geometry = Geometry::default();
        terminal
            .draw(|frame| {
                geometry = render(frame, &state, &app, &mut ListState::default(), 0);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let screen: String = (0..buffer.area.height)
            .map(|y| row_text(buffer, y))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(screen.contains("Close pane"), "{screen}");
        assert!(screen.contains("wA:p3"), "{screen}");
        assert!(screen.contains("This may lose:"), "{screen}");
        assert!(screen.contains("[ Cancel ]"), "{screen}");
        assert!(screen.contains("[ Confirm ]"), "{screen}");

        // The pointer is mapped to the buttons actually drawn, and the default
        // selection is Cancel (reversed), not the destructive button.
        let cancel = geometry.confirm_cancel.expect("cancel button recorded");
        let confirm = geometry.confirm_confirm.expect("confirm button recorded");
        assert_eq!(buffer[(cancel.x, cancel.y)].symbol(), "[");
        assert_eq!(buffer[(confirm.x, confirm.y)].symbol(), "[");
        assert!(
            buffer[(cancel.x, cancel.y)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "Cancel is the default selection"
        );
    }

    #[test]
    fn a_managed_agent_shows_no_confirmation() {
        let state = fixture_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p1");
        app.begin_action(crate::app::Operation::ClosePane, &state);
        assert!(app.confirmation().is_none());

        let screen = render_text(&state, &app, 100, 30);
        assert!(!screen.contains("[ Confirm ]"), "{screen}");
        assert!(screen.contains("managed"), "{screen}");
    }

    #[test]
    fn a_confirmation_renders_on_every_terminal_size() {
        let state = fixture_state();
        let mut app = app_for(&state);
        show_all_panes(&mut app);
        select_row(&mut app, crate::tree::RowId::Pane("wA:p3".into()));
        app.begin_action(crate::app::Operation::ClosePane, &state);
        for (width, height) in [(20u16, 6u16), (40, 10), (200, 60)] {
            let _ = render_text(&state, &app, width, height);
        }
    }

    /// One managed worker with a complete published identity.
    fn managed_state() -> ObservationState {
        let location = Location {
            workspace_id: "wM".into(),
            tab_id: "wM:t1".into(),
            pane_id: "wM:p1".into(),
        };
        let session = crate::model::SessionUuid::parse("01a10c77-8a6b-7035-8a0e-b1fa607bb507")
            .expect("a UUID");
        let mut state = ObservationState::new();
        state.apply_success(FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wM".into(),
                label: Some("managed".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wM:t1".into(),
                workspace_id: "wM".into(),
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
                label: None,
                status: Some(RuntimeStatus::Idle),
                session: None,
                lineage: Some(crate::model::Lineage {
                    session: session.clone(),
                    parent: Some(session),
                }),
                facts: HerdsmanFacts {
                    managed_metadata: true,
                    label: Some("implementer-1".into()),
                    run: Some("8f2b1c34-5d6e-4f70-8a91-2b3c4d5e6f71".into()),
                    state: Some(SemanticState::Idle),
                    ..Default::default()
                },
            }],
        });
        state
    }

    #[test]
    fn a_restart_confirmation_names_the_worker() {
        let state = managed_state();
        let mut app = app_for(&state);
        select_agent(&mut app, "wM:p1");
        app.begin_action(crate::app::Operation::Restart, &state);
        assert!(app.confirmation().is_some(), "an idle worker is eligible");
        let screen = render_text(&state, &app, 100, 30);
        assert!(screen.contains("Restart worker"), "{screen}");
        assert!(screen.contains("implementer-1"), "{screen}");
        assert!(screen.contains("[ Cancel ]"), "{screen}");
    }

    #[test]
    fn lifecycle_outcomes_render_on_the_footer_apart_from_the_source() {
        let state = fixture_state();
        let mut app = app_for(&state);
        app.apply_managed_update(crate::lifecycle::Update {
            id: "r1".into(),
            label: "implementer-1".into(),
            operation: crate::control::Operation::Close,
            kind: crate::lifecycle::UpdateKind::Started,
        });
        let screen = render_text(&state, &app, 120, 30);
        assert!(screen.contains("implementer-1"), "{screen}");
        assert!(screen.contains("started"), "{screen}");
        assert!(screen.contains("c dismiss"), "{screen}");
        let notice = screen
            .lines()
            .find(|line| line.contains("started"))
            .expect("a lifecycle line");
        assert!(
            !notice.contains("source:"),
            "kept apart from the source diagnostic: {notice}"
        );

        // `c` dismisses the outcome and the key hints return.
        press(&mut app, 'c');
        assert!(app.lifecycle_notices().is_empty());
        let screen = render_text(&state, &app, 120, 30);
        assert!(!screen.contains("started"), "{screen}");
        assert!(screen.contains("focus"), "{screen}");
    }
}
