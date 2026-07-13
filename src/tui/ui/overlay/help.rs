use crate::tui::theme::Theme;
use crate::tui::ui::overlay::layout::centered_rect_fixed;
use crate::tui::ui::shared::{kv_line, overlay_border_style};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, theme: &Theme, area: Rect) {
    let width = 56.min(area.width.saturating_sub(4));
    let height = 18.min(area.height.saturating_sub(2));
    let rect = centered_rect_fixed(width, height, area);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(overlay_border_style(theme, false));

    let lines = vec![
        kv_line("  Tab / ]   ", "next tab", theme, None),
        kv_line("  BackTab/[ ", "previous tab", theme, None),
        kv_line("  j / ↓     ", "move down", theme, None),
        kv_line("  k / ↑     ", "move up", theme, None),
        kv_line("  r         ", "refresh data", theme, None),
        kv_line("  l         ", "open command log", theme, None),
        kv_line("  ?         ", "toggle this help", theme, None),
        Line::raw(""),
        kv_line("  Sessions  ", "list active Virtuoso sessions", theme, None),
        kv_line("  Jobs      ", "x = cancel running job", theme, None),
        kv_line("  Config    ", "Enter/i = edit field", theme, None),
        Line::raw(""),
        kv_line("  q / Ctrl-C", "quit", theme, None),
    ];

    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), {
        let mut inner = rect;
        inner.x += 1;
        inner.y += 1;
        inner.width = inner.width.saturating_sub(2);
        inner.height = inner.height.saturating_sub(2);
        inner
    });
}
