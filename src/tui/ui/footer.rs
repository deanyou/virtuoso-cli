use crate::tui::app::state::{App, StatusKind};
use crate::tui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render_footer(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let mut spans = Vec::new();

    if let Some(status) = &app.status {
        let color = match status.kind {
            StatusKind::Info => theme.text_dim,
            StatusKind::Ok => theme.success,
            StatusKind::Warn => theme.accent,
            StatusKind::Err => theme.error,
        };
        let style = if theme.no_color {
            Style::default()
        } else {
            Style::default().fg(color)
        };
        spans.push(Span::styled(format!(" {} ", status.message), style));
        spans.push(Span::styled(
            "│",
            if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.border)
            },
        ));
    }

    let dim = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.text_dim)
    };
    spans.push(Span::styled(
        " j/k nav  Tab switch  r refresh  l log  ? help  q quit ",
        dim,
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
