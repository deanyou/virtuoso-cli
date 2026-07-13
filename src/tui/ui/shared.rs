use crate::tui::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub fn selection_style(theme: &Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.text)
            .bg(theme.bg_selected)
            .add_modifier(Modifier::BOLD)
    }
}

pub fn active_chip_style(theme: &Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    }
}

pub fn inactive_chip_style(theme: &Theme) -> Style {
    if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.text_dim).bg(theme.surface)
    }
}

pub fn pane_border_style(theme: &Theme, focused: bool) -> Style {
    if theme.no_color {
        if focused {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        }
    } else if focused {
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.border)
    }
}

pub fn overlay_border_style(theme: &Theme, attention: bool) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else if attention {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.primary)
    }
}

pub fn kv_line(
    label: &str,
    value: &str,
    theme: &Theme,
    value_color: Option<Color>,
) -> Line<'static> {
    let value_style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(value_color.unwrap_or(theme.text))
    };
    let label_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_dim)
    };
    Line::from(vec![
        Span::styled(label.to_string(), label_style),
        Span::styled(value.to_string(), value_style),
    ])
}

/// Truncate `s` to fit within `max_width` display columns, appending "…" if
/// the truncation happens. Uses `UnicodeWidthStr` so CJK and emoji are counted
/// as 2 cols each.
pub fn truncate_to_display_width(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let ellipsis_width = 1;
    let budget = max_width.saturating_sub(ellipsis_width);
    let mut out = String::new();
    let mut width = 0usize;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > budget {
            break;
        }
        out.push(ch);
        width += w;
    }
    out.push('…');
    out
}
