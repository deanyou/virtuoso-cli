//! Overlay geometry helpers. Ported from cc-switch-cli
//! (`src-tauri/src/cli/tui/ui/overlay/layout.rs`) with trimming to what vtui
//! actually uses. Unicode-width aware via the `unicode-width` crate so CJK
//! and emoji don't break centering.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TOAST_MAX_WIDTH: u16 = 80;

/// Center a rectangle by percentage of its container.
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Center a rectangle with fixed dimensions, clamped to the container.
pub fn centered_rect_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    Rect {
        x: r.x + r.width.saturating_sub(width) / 2,
        y: r.y + r.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Size a compact message overlay around its content. Clamps to
/// `TOAST_MAX_WIDTH` or container width, whichever is smaller.
pub fn compact_message_overlay_rect(content_area: Rect, title: &str, message: &str) -> Rect {
    let lines: Vec<String> = message.lines().map(|l| l.to_string()).collect();
    compact_lines_overlay_rect(content_area, title, &lines)
}

pub fn compact_lines_overlay_rect(content_area: Rect, title: &str, lines: &[String]) -> Rect {
    let max_width = content_area
        .width
        .saturating_sub(4)
        .clamp(1, TOAST_MAX_WIDTH);
    let min_width = 36.min(max_width);
    let content_width = lines
        .iter()
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0)
        .max(UnicodeWidthStr::width(title)) as u16;
    let width = content_width.saturating_add(8).clamp(min_width, max_width);

    let inner_width = width.saturating_sub(2).max(1);
    let wrapped_height = lines
        .iter()
        .map(|line| wrap_message_lines(line, inner_width).len().max(1) as u16)
        .sum::<u16>()
        .max(1);
    let max_height = content_area.height.saturating_sub(4).max(1);
    let height = wrapped_height.saturating_add(3).max(6).min(max_height);

    centered_rect_fixed(width, height, content_area)
}

/// Wrap a message string to fit within `width` display columns, respecting
/// multi-byte characters. Newlines in the source split to new lines; words
/// themselves may break mid-character if a single glyph is wider than
/// `width`.
pub fn wrap_message_lines(message: &str, width: u16) -> Vec<String> {
    let width = width as usize;
    if width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in message.chars() {
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
            continue;
        }
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if current_width + ch_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width = current_width.saturating_add(ch_width);
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Wrap a message and vertically center the wrapped block within `height`
/// rows. Useful for confirmation dialogs.
pub fn centered_message_lines(message: &str, width: u16, height: u16) -> Vec<Line<'static>> {
    let wrapped = wrap_message_lines(message, width);
    let pad = height.saturating_sub(wrapped.len() as u16) / 2;
    let mut out = Vec::with_capacity(pad as usize + wrapped.len());
    for _ in 0..pad {
        out.push(Line::raw(""));
    }
    out.extend(wrapped.into_iter().map(Line::raw));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_ascii_basic() {
        let out = wrap_message_lines("hello world", 5);
        assert_eq!(
            out,
            vec!["hello".to_string(), " worl".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn wrap_respects_newlines() {
        let out = wrap_message_lines("a\nb\nc", 10);
        assert_eq!(out, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn wrap_cjk_counts_2_cols() {
        // "中文" is 2 chars, 4 display cols. width=4 fits on one line.
        let out = wrap_message_lines("中文", 4);
        assert_eq!(out, vec!["中文".to_string()]);
        // width=2 fits just one CJK char per line
        let out2 = wrap_message_lines("中文", 2);
        assert_eq!(out2, vec!["中".to_string(), "文".to_string()]);
    }

    #[test]
    fn centered_rect_fixed_clamps() {
        let r = Rect::new(0, 0, 10, 10);
        let c = centered_rect_fixed(20, 20, r);
        assert_eq!(c.width, 10);
        assert_eq!(c.height, 10);
    }

    #[test]
    fn compact_overlay_min_width() {
        let area = Rect::new(0, 0, 100, 20);
        let r = compact_message_overlay_rect(area, "Title", "msg");
        assert!(r.width >= 36);
        assert!(r.height >= 6);
    }
}
