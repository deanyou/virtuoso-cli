use crate::tui::app::overlay::LogOverlay;
use crate::tui::theme::Theme;
use crate::tui::ui::overlay::layout::centered_rect;
use crate::tui::ui::shared::overlay_border_style;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub fn render(frame: &mut Frame, state: &LogOverlay, theme: &Theme, area: Rect) {
    let rect = centered_rect(85, 85, area);
    frame.render_widget(Clear, rect);

    let title = format!(
        " Command Log [{}/{}] ",
        state.scroll + 1,
        state.lines.len().max(1)
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(overlay_border_style(theme, false));

    let visible_height = rect.height.saturating_sub(2) as usize;
    let start = state.scroll;
    let end = (start + visible_height).min(state.lines.len());
    let visible: Vec<Line> = if start >= state.lines.len() {
        Vec::new()
    } else {
        state.lines[start..end]
            .iter()
            .map(|l| {
                let color = if theme.no_color {
                    theme.text
                } else if l.contains("[SKILL]") {
                    theme.primary
                } else if l.contains("error") || l.contains("Error") {
                    theme.error
                } else {
                    theme.text_dim
                };
                let style = if theme.no_color {
                    Style::default()
                } else {
                    Style::default().fg(color)
                };
                Line::from(Span::styled(l.as_str().to_string(), style))
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(visible)
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}
