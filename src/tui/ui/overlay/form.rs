use crate::tui::app::overlay::ConfigFormState;
use crate::tui::theme::Theme;
use crate::tui::ui::overlay::layout::centered_rect_fixed;
use crate::tui::ui::shared::{kv_line, overlay_border_style};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn render(frame: &mut Frame, form: &ConfigFormState, theme: &Theme, area: Rect) {
    // Fixed width form, height based on content
    let width = 64.min(area.width.saturating_sub(4));
    let height = 9.min(area.height.saturating_sub(2));
    let rect = centered_rect_fixed(width, height, area);
    frame.render_widget(Clear, rect);

    let title = format!(" Edit {} ", form.key);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(overlay_border_style(theme, true));
    let inner = block.inner(rect);

    // Split the cursor from the value so the underline shows where it is
    let (before, after) = form.value.as_str().split_at(form.value.cursor);
    let cursor_char = if after.is_empty() {
        " ".to_string()
    } else {
        after.chars().next().unwrap().to_string()
    };
    let after_tail = if after.is_empty() {
        String::new()
    } else {
        after[cursor_char.len()..].to_string()
    };

    let value_style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.text)
    };
    let cursor_style = if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(theme.text)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };

    let input_line = Line::from(vec![
        Span::styled("  > ", Style::default().fg(theme.accent)),
        Span::styled(before.to_string(), value_style),
        Span::styled(cursor_char, cursor_style),
        Span::styled(after_tail, value_style),
    ]);

    let hint_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_dim)
    };

    let lines = vec![
        kv_line("  Key:   ", &form.key, theme, Some(theme.primary)),
        kv_line("  Hint:  ", form.hint, theme, Some(theme.text_dim)),
        Line::raw(""),
        input_line,
        Line::raw(""),
        Line::from(Span::styled(
            "  Enter=save  Esc=cancel  ←/→=move",
            hint_style,
        )),
    ];

    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);

    // Silence the unused warning if the current row is narrow
    let _ = UnicodeWidthStr::width("");
}
