//! How an open [`Picker`] is drawn: a centred block with the query above a rule
//! and the rows below it, the chosen row carried by the theme's selection colour.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph};

use crate::picker::Picker;
use crate::{theme, ui};

/// Draws `picker` in the middle of the screen. Nothing else is drawn in that
/// block: a picker is a moment, not a panel. In a popup of its own, `fill` makes
/// the block the whole frame, since the popup already is the centred block.
pub fn draw<T>(frame: &mut Frame, picker: &Picker<T>, fill: bool) {
    let ink = theme::palette();
    let rows = picker.rows();
    let screen = frame.area();
    // A fixed block rather than a panel that grows to the edges: it keeps its
    // shape whatever the query selects, and scrolls inside itself on a short
    // terminal.
    let popup = if fill {
        screen
    } else {
        ui::centered(
            screen,
            screen.width.saturating_sub(8).min(72),
            (rows.len() as u16 + 6)
                .min(screen.height.saturating_mul(3) / 5)
                .clamp(6, screen.height),
        )
    };
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::new().fg(ink.border))
        .title(Span::styled(
            format!(" {} ", picker.title()),
            Style::new().fg(ink.heading).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            format!(" {} ", picker.hint()),
            Style::new().fg(ink.subtle),
        )))
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [query_area, rule_area, list_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::new().fg(ink.heading)),
            Span::styled(picker.query().to_string(), Style::new().fg(ink.heading)),
            Span::styled("▏", Style::new().fg(ink.border)),
        ])),
        query_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(rule_area.width as usize),
            Style::new().fg(ink.border),
        ))),
        rule_area,
    );
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "nothing matches",
                Style::new().fg(ink.subtle),
            ))),
            list_area,
        );
        return;
    }
    let items = rows
        .into_iter()
        .map(|(label, keys)| {
            let keys = keys.unwrap_or_default();
            // The shortcut sits against the right edge, so the labels read as one
            // column however long they are.
            let pad = (list_area.width as usize)
                .saturating_sub(label.chars().count() + keys.chars().count() + 1);
            ListItem::new(Line::from(vec![
                Span::styled(label.to_string(), Style::new().fg(ink.muted)),
                Span::raw(" ".repeat(pad)),
                Span::styled(keys.to_string(), Style::new().fg(ink.subtle)),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(picker.index()));
    frame.render_stateful_widget(
        List::new(items).highlight_style(
            Style::new()
                .bg(ink.selection)
                .fg(ink.heading)
                .add_modifier(Modifier::BOLD),
        ),
        list_area,
        &mut state,
    );
}
