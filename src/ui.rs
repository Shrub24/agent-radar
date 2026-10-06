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

use crate::app::{App, DetailPage, Disclosure, Geometry, Operation, VisibleRow};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bus::Task;
use crate::model::{
    AgentState, BinaryFreshness, BinaryIdentity, CpuPercent, ForegroundEvidence, HerdsmanFacts,
    ProcessState, SessionIdentity, TerminalMode, Total,
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

    let details = app
        .shows_details()
        .then(|| detail_page_lines(state, app, app.detail_page()));
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
    }

    let (confirm_cancel, confirm_confirm) = draw_confirmation(frame, app);

    Geometry {
        tree_panel: tree_area,
        tree_content,
        details: detail_area,
        details_rows,
        details_viewport,
        detail_tabs,
        disclosure_markers,
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
        let width = (body.width / 3).clamp(DETAIL_MIN_WIDTH, DETAIL_MAX_WIDTH);
        let [tree, detail_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(width)]).areas(body);
        return (tree, Some(detail_area));
    }

    // Stacked: as many rows as the content needs, counting the rows a long
    // identity will actually wrap onto and the rows the page tabs take, and
    // never more than half the body.
    let columns = body.width.saturating_sub(2).max(1) as u32;
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

/// How a live agent's running executable compares with the program installed
/// now, in words.
///
/// Two identities that differ without a replacement are a deliberate other
/// build — a checkout or a second installation — not a stale one; matching
/// installations say nothing, and an unknown comparison is never claimed.
fn binary_line(identity: &BinaryIdentity) -> Option<Line<'static>> {
    let running = identity.running.as_deref();
    let installed = identity.installed.as_deref();
    let verdict = match identity.freshness {
        BinaryFreshness::Stale => "stale",
        BinaryFreshness::Current if running != installed => "not the installed program",
        _ => return None,
    };
    Some(field(
        "binary",
        format!(
            "{verdict} — running {}, installed {}",
            sanitize(running?),
            sanitize(installed?)
        ),
    ))
}

/// What a descendant sum is, said on the page that draws one: the processes the
/// kernel reports beneath a root, which is neither who owns them nor what work
/// they serve — and a resident set that counts a page shared by two processes
/// in each of them.
const DESCENDANT_SUM: &str = "observed processes beneath this one, not a workload or assignment total \
     (a page shared with another process counts in each)";

/// The metrics for a row whose foreground process was sampled: the incarnation
/// the reading belongs to, the kernel's own state, what that process is using,
/// and what is observed beneath it.
///
/// The process's own figures and its descendants' are drawn apart and never
/// added together: a build under a pane is the build's CPU, not the pane's.
/// Nothing here is inherited from an owner, a session or a background task — a
/// task's published PID arrives without a birth identity and is not sampled at
/// all. A value this machine could not measure says so with its reason rather
/// than as a zero, and a sum that could not cover every member is drawn as the
/// lower bound it is.
fn metric_lines(foreground: Option<&ForegroundEvidence>) -> Vec<Line<'static>> {
    let Some(ForegroundEvidence::NonShell { local, .. }) = foreground else {
        // A shell or an inconclusive answer names no process, so nothing was
        // sampled and there is no metric to draw.
        return Vec::new();
    };
    let Some(resources) = local.resources.as_ref() else {
        return vec![field(
            "metrics",
            format!(
                "{} — this refresh took no sample of this process",
                unavailable()
            ),
        )];
    };
    let identity = &resources.identity;
    let descendants = &resources.descendants;
    vec![
        field(
            "birth",
            format!(
                "pid {} · boot {} · start ticks {}",
                identity.pid,
                sanitize(&identity.boot_id),
                identity.start_ticks,
            ),
        ),
        field("state", state_line(resources.state)),
        field("cpu", cpu_line(resources.cpu)),
        field("rss", rss_line(resources.rss_bytes)),
        field(
            "descendants",
            match descendants.observed {
                Some(count) => format!(
                    "{count} {} observed beneath this one",
                    if count == 1 { "process" } else { "processes" }
                ),
                None => format!(
                    "{} — nothing beneath this process could be enumerated",
                    unavailable()
                ),
            },
        ),
        field(
            "descendant cpu",
            total_line(&descendants.cpu, |cpu| cpu_line(Some(*cpu))),
        ),
        field(
            "descendant rss",
            total_line(&descendants.rss_bytes, |bytes| rss_line(Some(*bytes))),
        ),
        field("descendant sum", DESCENDANT_SUM.to_string()),
    ]
}

/// The scheduler state the kernel reported, in words.
///
/// It says only what the scheduler is doing with the process — a sleeping
/// process may be waiting on a socket or on nothing, and a zombie has already
/// exited — so it is drawn as the kernel's own state, never as progress or as a
/// verdict on the work underneath.
fn state_line(state: ProcessState) -> String {
    match state {
        ProcessState::Running => "running — on a CPU or waiting for one".into(),
        ProcessState::Sleeping => "sleeping — waiting, and wakeable".into(),
        ProcessState::DiskSleep => "uninterruptible sleep — blocked in the kernel".into(),
        ProcessState::Stopped => "stopped by a signal".into(),
        ProcessState::TracingStop => "stopped by a tracer".into(),
        ProcessState::Zombie => "zombie — exited, not yet reaped".into(),
        ProcessState::Dead => "dead — gone, or being torn down".into(),
        ProcessState::Idle => "idle — below the scheduler's oldest run queue".into(),
        ProcessState::Other(letter) => {
            format!(
                "unknown — this kernel reports '{}'",
                sanitize(&letter.to_string())
            )
        }
    }
}

/// Interval CPU as a reader wants it: a percentage of one CPU, which work on
/// more than one can carry past 100.
///
/// Unavailable until two readings of the same incarnation make an interval. A
/// first reading, a counter that went backwards, no elapsed time and a failed
/// read all leave nothing to measure — which is not the same as zero, and is
/// not always the warm-up either. A measured idle interval is zero, and is
/// drawn as one.
fn cpu_line(cpu: Option<CpuPercent>) -> String {
    let Some(cpu) = cpu else {
        return format!(
            "{} — no interval of this process has been measured",
            unavailable()
        );
    };
    format!("{:.1}% of one CPU", cpu.hundredths() as f64 / 100.0)
}

/// A process's resident set, or why this machine cannot say. A page count the
/// kernel wrote that is not a size, and a machine that cannot report its page
/// size, both leave nothing to convert.
fn rss_line(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => size(bytes),
        None => format!(
            "{} — the kernel's page count could not be converted",
            unavailable()
        ),
    }
}

/// A total as the page draws it: its value, or why it is not one. A partial
/// total keeps what it does cover and says it is a lower bound; an unknown one
/// is unavailable, with the reason no total could be made.
fn total_line<T>(total: &Total<T>, show: impl Fn(&T) -> String) -> String {
    match total {
        Total::Complete(value) => show(value),
        Total::Partial(value, reason) => {
            format!("{} — a lower bound: {}", show(value), sanitize(reason))
        }
        Total::Unknown(reason) => format!("{} — {}", unavailable(), sanitize(reason)),
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
}

impl<'a> Blocks<'a> {
    fn new(app: &'a App) -> Self {
        Self {
            targets: app.disclosures(),
            highlighted: app.disclosure_target(),
            app,
        }
    }

    /// Whether the page offers this block, and whether it is open. `None` is
    /// content with nothing to open, which is drawn as it always was.
    fn open(&self, key: &Disclosure) -> Option<bool> {
        self.targets
            .contains(key)
            .then(|| self.app.disclosure_open(key))
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
fn detail_page_lines(state: &ObservationState, app: &App, page: DetailPage) -> PageLines {
    let mut page_lines = PageLines::default();
    let Some(row) = app.selected_row() else {
        page_lines.push(Line::from("no row selected"));
        return page_lines;
    };
    page_lines.push(Line::from(Span::styled(
        sanitize(row.node.row.title()),
        Style::new().add_modifier(Modifier::BOLD),
    )));
    let blocks = Blocks::new(app);
    match page {
        DetailPage::Overview => overview_lines(state, &row, &blocks, &mut page_lines),
        DetailPage::Processes => page_lines.extend(process_lines(state, &row)),
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

/// Processes: the process holding this row's location. A row whose process is
/// not currently observed says so rather than drawing last-good facts as live,
/// and a container row has no process to name.
///
/// The measurements come last, after the identity and command they belong to,
/// and they belong to this row's own process: a background task's published PID
/// is named without them, because a PID alone cannot authorise a reading of a
/// process that may have been replaced.
fn process_lines(state: &ObservationState, row: &VisibleRow<'_>) -> Vec<Line<'static>> {
    let current = matches!(state.source_freshness(), SourceFreshness::Current);
    match &row.node.row.kind {
        RowKind::Workspace { .. } => vec![field(
            "process",
            "none — a workspace holds panes, not a process".to_string(),
        )],
        RowKind::Pane(pane) => {
            if !current {
                return vec![withheld("the source is not current")];
            }
            let mut lines = Vec::new();
            lines.extend(live_pid(state, pane.foreground.as_ref()));
            lines.push(field(
                "foreground",
                foreground_text(pane.foreground.as_ref(), pane.command()),
            ));
            lines.push(field("terminal", terminal_line(pane.terminal())));
            lines.push(field(
                "running for",
                pane.running_for().map(duration).unwrap_or_else(unavailable),
            ));
            lines.extend(metric_lines(pane.foreground.as_ref()));
            lines
        }
        RowKind::Agent(agent) => {
            let unavailable_reason = match (&agent.retained, current) {
                (Some(_), _) => Some("this row is retained, not currently observed"),
                (None, false) => Some("the source is not current"),
                (None, true) => None,
            };
            if let Some(reason) = unavailable_reason {
                return vec![withheld(reason)];
            }
            let mut lines = Vec::new();
            lines.extend(live_pid(state, agent.foreground.as_ref()));
            lines.push(field(
                "foreground",
                foreground_text(
                    agent.foreground.as_ref(),
                    agent
                        .foreground
                        .as_ref()
                        .and_then(ForegroundEvidence::command_line),
                ),
            ));
            // A live agent's running executable compared with the program
            // installed now: a machine fact about the process, not the agent.
            lines.extend(agent.binary().and_then(binary_line));
            lines.extend(metric_lines(agent.foreground.as_ref()));
            lines
        }
        RowKind::Task(task) => {
            let published = task.published.as_ref();
            let mut lines = vec![match published.and_then(|published| published.pid) {
                Some(pid) => field("pid", pid.to_string()),
                None => field("pid", unavailable()),
            }];
            lines.push(field("source", task.basis().to_string()));
            // A task's PID arrives without the identity of the process it
            // names, so no sample can be attributed to it: the publisher's
            // captured-at-spawn birth identity is still outstanding.
            lines.push(field(
                "metrics",
                "unavailable — the publisher sends no process birth identity".to_string(),
            ));
            lines
        }
    }
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

/// What the pane's foreground is, in the words the panel uses for it: a shell
/// or an inconclusive answer is not a command to name.
fn foreground_text(foreground: Option<&ForegroundEvidence>, command: Option<String>) -> String {
    match foreground {
        Some(ForegroundEvidence::NonShell { .. }) => command.unwrap_or_else(unavailable),
        Some(ForegroundEvidence::Shell) => "shell — nothing in the foreground".into(),
        Some(ForegroundEvidence::Inconclusive) => "unknown — PID fields disagree".into(),
        None => unavailable(),
    }
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
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use crate::app::Action;
    use crate::herdr::decode_snapshot;
    use crate::model::{
        AgentObservation, BinaryFreshness, BinaryIdentity, CpuPercent, DescendantResources,
        FleetObservation, ForegroundEvidence, HerdsmanFacts, LocalFacts, Location, Pane,
        ProcessIdentity, ProcessResources, ProcessState, RuntimeStatus, SemanticState, Tab,
        TerminalMode, Total, Workspace,
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
        let shown = app.detail_page();
        app.select_page(page);
        let mut terminal =
            Terminal::new(TestBackend::new(120, 30)).expect("infallible test backend");
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
            "foreground: cargo",
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
        assert!(processes.contains("foreground: cargo"), "{processes}");
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
        let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 32);

        assert!(screen.contains("nix build .#radar"), "{screen}");
        assert!(screen.contains("foreground: nix build .#radar"), "{screen}");
        // The executable's own path never reaches the screen.
        assert!(!screen.contains("/nix/store/abc-nix"), "{screen}");
    }

    /// Foreground evidence for a pane, carrying a binary comparison, so a row
    /// and its details can be read against exactly that identity.
    fn binary_evidence(identity: BinaryIdentity) -> ForegroundEvidence {
        let mut evidence = ForegroundEvidence::command(4242, Some("pi".into()), Some("pi".into()));
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

    /// A sampled process, with whatever the scan observed beneath it.
    fn resources(
        cpu: Option<CpuPercent>,
        rss_bytes: Option<u64>,
        descendants: DescendantResources,
    ) -> ProcessResources {
        ProcessResources {
            identity: ProcessIdentity {
                boot_id: "6d9d2f0a-2f6f-4a1f-9c2d-2f6f4a1f9c2d".into(),
                pid: 4242,
                start_ticks: 9_812,
            },
            state: ProcessState::Sleeping,
            rss_bytes,
            cpu,
            descendants,
        }
    }

    /// Nothing observed beneath a root, with every process of the scan read.
    fn no_descendants() -> DescendantResources {
        DescendantResources {
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
        let processes = panel_text(&state, &mut app, DetailPage::Processes);

        // The incarnation the reading belongs to, and the kernel's own state.
        assert!(processes.contains("birth: pid 4242 · boot "), "{processes}");
        assert!(processes.contains("start ticks 9812"), "{processes}");
        assert!(
            processes.contains("state: sleeping — waiting, and wakeable"),
            "{processes}"
        );
        // The root's own figures are its own: the build beneath it is not added
        // to them.
        assert!(processes.contains("cpu: 0.0% of one CPU"), "{processes}");
        assert!(processes.contains("rss: 8.0 MiB"), "{processes}");
        assert!(
            processes.contains("descendants: 2 processes observed beneath this one"),
            "{processes}"
        );
        // The descendants are a separate sum, qualified as what it is.
        assert!(
            processes.contains("descendant cpu: 125.0% of one CPU"),
            "{processes}"
        );
        assert!(
            processes.contains("descendant rss: 512.0 MiB"),
            "{processes}"
        );
        assert!(
            processes.contains("not a workload or assignment total"),
            "{processes}"
        );
        assert!(
            processes.contains("a page shared with another process counts in each"),
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
        let processes = panel_text(&state, &mut app, DetailPage::Processes);

        // No interval to measure — a first reading, a counter that went
        // backwards and a failed read all say this, and none is a zero.
        assert!(
            processes.contains("cpu: unavailable — no interval of this process has been measured"),
            "{processes}"
        );
        // A descendant total that really is zero is drawn as a measurement.
        assert!(
            processes.contains("descendant cpu: 0.0% of one CPU"),
            "{processes}"
        );
        // The root's own line, between the state above it and the resident set
        // below it, is the one that must not read as a zero.
        let root = processes
            .split("state: ")
            .nth(1)
            .and_then(|rest| rest.split("rss: ").next())
            .expect("the state and the resident set");
        assert!(
            root.contains("cpu: unavailable — no interval"),
            "{processes}"
        );
        assert!(!root.contains("0.0%"), "{processes}");
        // Nothing observed beneath the root is a fact about the root, not a
        // missing measurement.
        assert!(
            processes.contains("descendants: 0 processes observed beneath this one"),
            "{processes}"
        );
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
        let processes = panel_text(&state, &mut app, DetailPage::Processes);

        // The root's own reading is unaffected by what could not be totalled
        // beneath it.
        assert!(processes.contains("cpu: 42.0% of one CPU"), "{processes}");
        // A lower bound keeps the value it does cover and says it is one.
        assert!(
            processes.contains(
                "descendant rss: 2.0 KiB — a lower bound: \
                 a process beneath this one was reparented while the table was read"
            ),
            "{processes}"
        );
        // An unknown total is unavailable with its reason, never a complete zero.
        assert!(
            processes.contains("descendant cpu: unavailable — the process scan was cancelled"),
            "{processes}"
        );
        assert!(
            processes.contains("descendants: 3 processes observed beneath this one"),
            "{processes}"
        );
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
                        observed: None,
                        rss_bytes: Total::Unknown("the process table could not be read".into()),
                        cpu: Total::Unknown("the process table could not be read".into()),
                    },
                ),
            ),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let processes = panel_text(&state, &mut app, DetailPage::Processes);

        assert!(
            processes.contains(
                "descendants: unavailable — nothing beneath this process could be enumerated"
            ),
            "{processes}"
        );
        assert!(
            processes.contains("descendant rss: unavailable — the process table could not be read"),
            "{processes}"
        );
        assert!(
            processes.contains("descendant cpu: unavailable — the process table could not be read"),
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
        assert!(!processes.contains("birth:"), "{processes}");
        assert!(!processes.contains("cpu:"), "{processes}");
        assert!(!processes.contains("descendant"), "{processes}");

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
        assert!(!processes.contains("birth:"), "{processes}");
        assert!(!processes.contains("descendant"), "{processes}");
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
        // rather than drawing a birth identity or a resource it never read.
        assert!(processes.contains("pid: 4242"), "{processes}");
        assert!(
            processes
                .contains("metrics: unavailable — this refresh took no sample of this process"),
            "{processes}"
        );
        assert!(!processes.contains("birth:"), "{processes}");
        assert!(!processes.contains("rss:"), "{processes}");
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
        // last row rather than stopping at the last one that starts on it.
        let (_, short) = draw(&state, &mut app, 120, 12);
        assert!(
            short.details_rows > short.details_viewport as usize,
            "the panel has more to show than it fits: {short:?}"
        );
        let max = (short.details_rows - short.details_viewport as usize) as u16;
        app.scroll_page_to(u16::MAX);
        assert_eq!(app.details_scroll(), max, "the clamp counts drawn rows");
        let (scrolled, _) = draw(&state, &mut app, 120, 12);
        assert!(scrolled.contains("descendant rss:"), "{scrolled}");
        assert!(
            scrolled.contains("counts in each)"),
            "the last wrapped row of the page is drawn: {scrolled}"
        );
        // The end is the end: a further scroll key does not move the page.
        app.scroll_page(3);
        assert_eq!(app.details_scroll(), max);

        // Narrow enough that the panel stacks under the fleet, and wraps the
        // same page further still: every row stays reachable there too.
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        app.select_page(DetailPage::Processes);
        draw(&state, &mut app, 46, 20);
        let (_, narrow) = draw(&state, &mut app, 46, 20);
        app.scroll_page_to(u16::MAX);
        assert_eq!(
            app.details_scroll(),
            (narrow.details_rows - narrow.details_viewport as usize) as u16
        );
        let (scrolled, _) = draw(&state, &mut app, 46, 20);
        assert!(scrolled.contains("descendant sum:"), "{scrolled}");
        assert!(
            scrolled.contains("counts in each)"),
            "the last wrapped row is drawn on a stacked panel: {scrolled}"
        );

        // The values are on the page, not only in its last rows.
        let page = panel_text(&state, &mut app, DetailPage::Processes);
        assert!(page.contains("descendant rss: 512.0 MiB"), "{page}");
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
        // The details name the two installations exactly, with no version
        // parsed out of either.
        assert!(screen.contains("binary: stale"), "{screen}");
        assert!(screen.contains("/run/pi-1.0.1"), "{screen}");
        assert!(screen.contains("/nix/store/aaa-pi-1.0.2"), "{screen}");

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
            }),
        );
        let mut app = app_for(&state);
        select_agent(&mut app, "wA:p2");
        let screen = render_page(&state, &mut app, DetailPage::Processes, 180, 34);

        assert!(
            !screen.contains(&format!("{} worker task", theme::stale_mark())),
            "{screen}"
        );
        assert!(
            screen.contains("binary: not the installed program"),
            "{screen}"
        );
        assert!(screen.contains("/home/dev/pi"), "{screen}");
        assert!(screen.contains("/nix/store/aaa-pi-1.0.2"), "{screen}");
    }

    #[test]
    fn matching_or_unreadable_installations_claim_nothing() {
        // A running executable that is the installed one says nothing: there is
        // no warning to give.
        let mut matching = fixture_state();
        matching.apply_evidence(
            "wA:p2",
            binary_evidence(BinaryIdentity {
                freshness: BinaryFreshness::Current,
                running: Some("/nix/store/aaa-pi-1.0.2".into()),
                installed: Some("/nix/store/aaa-pi-1.0.2".into()),
            }),
        );
        let mut app = app_for(&matching);
        select_agent(&mut app, "wA:p2");
        // Read on Processes, the page that carries the comparison: an absence
        // asserted anywhere else would say nothing about it.
        let screen = render_page(&matching, &mut app, DetailPage::Processes, 180, 34);
        assert!(!screen.contains("binary:"), "{screen}");
        assert!(
            !screen.contains(&format!("{} worker task", theme::stale_mark())),
            "{screen}"
        );

        // An unknown comparison — a gone process, no PATH match, an interpreter
        // — is not claimed either way.
        let mut unknown = fixture_state();
        unknown.apply_evidence("wA:p2", binary_evidence(BinaryIdentity::default()));
        let mut app = app_for(&unknown);
        select_agent(&mut app, "wA:p2");
        let screen = render_page(&unknown, &mut app, DetailPage::Processes, 180, 34);
        assert!(!screen.contains("binary:"), "{screen}");
        assert!(
            !screen.contains(&format!("{} worker task", theme::stale_mark())),
            "{screen}"
        );
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

        // The comparison is withheld, not drawn from the last-good evidence.
        let processes = render_page(&state, &mut app, DetailPage::Processes, 180, 34);
        assert!(
            processes.contains("withheld — the source is not current"),
            "{processes}"
        );
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
        assert!(
            processes.contains("foreground: nvim notes.md"),
            "{processes}"
        );
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
