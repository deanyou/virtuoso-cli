use crate::tui::app::overlay::ConfirmOverlay;
use crate::tui::theme::Theme;
use crate::tui::ui::overlay::layout::{centered_message_lines, compact_message_overlay_rect};
use crate::tui::ui::shared::overlay_border_style;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, confirm: &ConfirmOverlay, theme: &Theme, area: Rect) {
    let title = format!(" {} ", confirm.title);
    let rect = compact_message_overlay_rect(area, &title, &confirm.message);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(overlay_border_style(theme, true));
    let inner = block.inner(rect);

    let message_height = inner.height.saturating_sub(2);
    let mut lines = centered_message_lines(&confirm.message, inner.width, message_height);

    lines.push(Line::raw(""));
    let hint_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_dim)
    };
    lines.push(Line::from(Span::styled(
        "y/Enter=confirm   n/Esc=cancel",
        hint_style,
    )));

    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}
