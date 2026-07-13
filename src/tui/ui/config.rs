use crate::tui::app::state::App;
use crate::tui::theme::Theme;
use crate::tui::ui::shared::{kv_line, pane_border_style, selection_style};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render_list(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Config ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, false));

    let items: Vec<ListItem> = app
        .config_fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let display = f.key.strip_prefix("VB_").unwrap_or(&f.key);
            let row_style = if i == app.selected_config {
                selection_style(theme)
            } else if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            };
            ListItem::new(Line::from(Span::styled(format!(" {display}"), row_style)))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

pub fn render_detail(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Field ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, true));

    let Some(f) = app.selected_config_field() else {
        frame.render_widget(Paragraph::new("").block(block), area);
        return;
    };

    let val = if f.value.is_empty() {
        "(not set)"
    } else {
        &f.value
    };

    let lines = vec![
        kv_line("  Key:   ", &f.key, theme, Some(theme.primary)),
        kv_line("  Hint:  ", f.hint, theme, Some(theme.text_dim)),
        Line::default(),
        kv_line("  Value: ", val, theme, Some(theme.accent)),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn render_hint(frame: &mut Frame, _app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Shortcuts ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, false));

    let hint_style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.text_dim)
    };
    let lines = vec![
        Line::from(Span::styled("  Enter/i  open field editor", hint_style)),
        Line::from(Span::styled("  j/k      move selection", hint_style)),
        Line::from(Span::styled("  Tab      next tab", hint_style)),
        Line::from(Span::styled("  ?        help overlay", hint_style)),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
