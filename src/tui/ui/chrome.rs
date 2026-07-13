use crate::tui::app::state::{App, Tab};
use crate::tui::theme::Theme;
use crate::tui::ui::shared::{active_chip_style, inactive_chip_style, truncate_to_display_width};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

const TITLE: &str = " vcli TUI ";

pub fn render_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(if theme.no_color {
            Style::default()
        } else {
            Style::default().fg(theme.border)
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build tab chips
    let (tabs_spans, tabs_width) = build_tabs(app.tab, theme);
    // Build right-side status badges
    let (status_spans, status_width) = build_status(app, theme);

    let title_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };
    let title_width = UnicodeWidthStr::width(TITLE) as u16;

    // Layout: title (left) | tabs (center-ish) | status (right)
    // If width is tight, truncate status first, then tabs; title never truncates.
    let total = inner.width;
    let gutter = 2u16;
    let title_right = inner.x + title_width.min(total);

    let needs = title_width + gutter + tabs_width + gutter + status_width;
    let available_for_status = if needs <= total {
        status_width
    } else {
        // Shrink status to whatever remains after title + gutters + tabs.
        let used = title_width + gutter + tabs_width + gutter;
        total.saturating_sub(used)
    };

    let status_start = inner.x + total.saturating_sub(available_for_status);
    let tabs_start = title_right + gutter;
    let tabs_end = status_start.saturating_sub(gutter).max(tabs_start);

    let title_area = Rect::new(inner.x, inner.y, title_width.min(total), 1);
    let tabs_area = Rect::new(tabs_start, inner.y, tabs_end.saturating_sub(tabs_start), 1);
    let status_area = Rect::new(status_start, inner.y, available_for_status, 1);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(TITLE, title_style))).alignment(Alignment::Left),
        title_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(tabs_spans)).alignment(Alignment::Left),
        tabs_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(truncate_status(
            status_spans,
            available_for_status,
        )))
        .alignment(Alignment::Right),
        status_area,
    );
}

fn build_tabs(active: Tab, theme: &Theme) -> (Vec<Span<'static>>, u16) {
    let mut spans = Vec::with_capacity(Tab::all().len() * 2);
    let mut width = 0u16;
    for (idx, tab) in Tab::all().iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw(" "));
            width += 1;
        }
        let label = format!(" {} ", tab.label());
        let w = UnicodeWidthStr::width(label.as_str()) as u16;
        let style = if *tab == active {
            active_chip_style(theme)
        } else {
            inactive_chip_style(theme)
        };
        spans.push(Span::styled(label, style));
        width += w;
    }
    (spans, width)
}

fn build_status(app: &App, theme: &Theme) -> (Vec<Span<'static>>, u16) {
    let mut spans = Vec::new();
    let mut width = 0u16;

    // Tunnel badge
    let (tunnel_label, tunnel_color) = match &app.tunnel_state {
        Some(_) => ("Tunnel: active", theme.success),
        None => ("Tunnel: local", theme.text_dim),
    };
    let tunnel_text = format!(" {tunnel_label} ");
    width += UnicodeWidthStr::width(tunnel_text.as_str()) as u16;
    let tunnel_style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(tunnel_color)
    };
    spans.push(Span::styled(tunnel_text, tunnel_style));

    // Counts
    let counts = format!(" S:{} J:{} ", app.sessions.len(), app.jobs.len());
    width += UnicodeWidthStr::width(counts.as_str()) as u16;
    let counts_style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.text_dim)
    };
    spans.push(Span::styled(counts, counts_style));

    (spans, width)
}

/// If the status area is narrower than the full status text, truncate spans
/// so the layout doesn't overflow. `available` is the total columns this
/// paragraph can consume.
fn truncate_status(spans: Vec<Span<'static>>, available: u16) -> Vec<Span<'static>> {
    let total: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total <= available as usize {
        return spans;
    }
    // Simple fallback: concatenate text and truncate via ellipsis.
    let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let trimmed = truncate_to_display_width(&joined, available as usize);
    vec![Span::raw(trimmed)]
}
