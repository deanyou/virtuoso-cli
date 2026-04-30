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
        .title(" Sessions ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, false));

    if app.sessions.is_empty() {
        let p = Paragraph::new(" No sessions")
            .block(block)
            .style(if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            });
        frame.render_widget(p, area);
        return;
    }

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let icon_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.success)
            };
            let row_style = if i == app.selected_session {
                selection_style(theme)
            } else if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            };
            ListItem::new(Line::from(vec![
                Span::styled(" ● ", icon_style),
                Span::styled(s.id.clone(), row_style),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

pub fn render_detail(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, true));

    let Some(s) = app.selected_session_info() else {
        let p = Paragraph::new("  Select a session")
            .block(block)
            .style(if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            });
        frame.render_widget(p, area);
        return;
    };

    let mut lines = vec![
        kv_line("  Session: ", &s.id, theme, Some(theme.primary)),
        kv_line("  Port:    ", &s.port.to_string(), theme, None),
        kv_line("  PID:     ", &s.pid.to_string(), theme, None),
        kv_line("  Host:    ", &s.host, theme, None),
        kv_line("  Created: ", &s.created, theme, None),
    ];
    if let Some(stats) = app.daemon_stats.get(&s.port) {
        let uptime = if stats.uptime_secs < 60 {
            format!("{}s", stats.uptime_secs)
        } else if stats.uptime_secs < 3600 {
            format!("{}m{}s", stats.uptime_secs / 60, stats.uptime_secs % 60)
        } else {
            format!("{}h{}m", stats.uptime_secs / 3600, (stats.uptime_secs % 3600) / 60)
        };
        lines.push(kv_line("  Calls:   ", &stats.calls.to_string(), theme, None));
        lines.push(kv_line("  Errors:  ", &stats.errors.to_string(), theme, None));
        lines.push(kv_line("  Uptime:  ", &uptime, theme, None));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn render_tunnel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Tunnel ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, false));

    let lines = if let Some(ref t) = app.tunnel_state {
        vec![
            kv_line("  Status: ", "active", theme, Some(theme.success)),
            kv_line("  Port:   ", &t.port.to_string(), theme, None),
            kv_line("  Remote: ", &t.remote_host, theme, None),
        ]
    } else {
        vec![kv_line(
            "  Status: ",
            "not active (local mode)",
            theme,
            Some(theme.text_dim),
        )]
    };

    frame.render_widget(Paragraph::new(lines).block(block), area);
}
