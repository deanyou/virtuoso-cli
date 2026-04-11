use crate::spectre::jobs::JobStatus;
use crate::tui::state::{ActiveTab, TuiState};
use crate::tui::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

const SPINNERS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn render(frame: &mut Frame, state: &TuiState, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(frame.area());

    let main_area = chunks[0];
    let status_bar = chunks[1];

    // Main: left panel + right panel
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(30)])
        .split(main_area);

    render_left_panel(frame, state, theme, cols[0]);
    render_right_panel(frame, state, theme, cols[1]);
    render_status_bar(frame, state, theme, status_bar);

    // Log overlay
    if state.show_log {
        render_log_overlay(frame, state, theme);
    }
}

fn render_left_panel(frame: &mut Frame, state: &TuiState, theme: &Theme, area: Rect) {
    let tab_title = match state.active_tab {
        ActiveTab::Sessions => " Sessions ",
        ActiveTab::Jobs => " Jobs ",
    };

    let block = Block::default()
        .title(tab_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.primary));

    match state.active_tab {
        ActiveTab::Sessions => {
            let items: Vec<ListItem> = state
                .sessions
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let icon = "●";
                    let style = if i == state.selected_session {
                        Style::default()
                            .fg(theme.success)
                            .bg(theme.bg_selected)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.text_dim)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {icon} "), style.fg(theme.success)),
                        Span::styled(&s.id, style),
                    ]))
                })
                .collect();

            if items.is_empty() {
                let p = Paragraph::new(" No sessions")
                    .block(block)
                    .style(Style::default().fg(theme.text_dim));
                frame.render_widget(p, area);
            } else {
                let list = List::new(items).block(block);
                frame.render_widget(list, area);
            }
        }
        ActiveTab::Jobs => {
            let items: Vec<ListItem> = state
                .jobs
                .iter()
                .enumerate()
                .map(|(i, j)| {
                    let (icon, color) = match j.status {
                        JobStatus::Completed => ("✓", theme.success),
                        JobStatus::Running => {
                            (SPINNERS[state.spinner_frame % SPINNERS.len()], theme.accent)
                        }
                        JobStatus::Failed => ("✗", theme.error),
                        JobStatus::Cancelled => ("⊘", theme.text_dim),
                    };
                    let style = if i == state.selected_job {
                        Style::default()
                            .fg(color)
                            .bg(theme.bg_selected)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.text_dim)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {icon} "), Style::default().fg(color)),
                        Span::styled(&j.id, style),
                    ]))
                })
                .collect();

            if items.is_empty() {
                let p = Paragraph::new(" No jobs")
                    .block(block)
                    .style(Style::default().fg(theme.text_dim));
                frame.render_widget(p, area);
            } else {
                let list = List::new(items).block(block);
                frame.render_widget(list, area);
            }
        }
    }
}

fn render_right_panel(frame: &mut Frame, state: &TuiState, theme: &Theme, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Top: detail
    render_detail(frame, state, theme, rows[0]);
    // Bottom: jobs summary or tunnel info
    render_bottom(frame, state, theme, rows[1]);
}

fn render_detail(frame: &mut Frame, state: &TuiState, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    match state.active_tab {
        ActiveTab::Sessions => {
            if let Some(s) = state.selected_session_info() {
                let lines = vec![
                    Line::from(vec![
                        Span::styled("  Session: ", Style::default().fg(theme.text_dim)),
                        Span::styled(
                            &s.id,
                            Style::default()
                                .fg(theme.primary)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("  Port:    ", Style::default().fg(theme.text_dim)),
                        Span::styled(s.port.to_string(), Style::default().fg(theme.text)),
                    ]),
                    Line::from(vec![
                        Span::styled("  PID:     ", Style::default().fg(theme.text_dim)),
                        Span::styled(s.pid.to_string(), Style::default().fg(theme.text)),
                    ]),
                    Line::from(vec![
                        Span::styled("  Host:    ", Style::default().fg(theme.text_dim)),
                        Span::styled(&s.host, Style::default().fg(theme.text)),
                    ]),
                    Line::from(vec![
                        Span::styled("  Created: ", Style::default().fg(theme.text_dim)),
                        Span::styled(&s.created, Style::default().fg(theme.text)),
                    ]),
                ];
                let p = Paragraph::new(lines).block(block);
                frame.render_widget(p, area);
            } else {
                let p = Paragraph::new("  Select a session")
                    .block(block)
                    .style(Style::default().fg(theme.text_dim));
                frame.render_widget(p, area);
            }
        }
        ActiveTab::Jobs => {
            if let Some(j) = state.jobs.get(state.selected_job) {
                let (status_str, color) = match j.status {
                    JobStatus::Completed => ("completed", theme.success),
                    JobStatus::Running => ("running", theme.accent),
                    JobStatus::Failed => ("failed", theme.error),
                    JobStatus::Cancelled => ("cancelled", theme.text_dim),
                };
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("  Job ID:  ", Style::default().fg(theme.text_dim)),
                        Span::styled(
                            &j.id,
                            Style::default()
                                .fg(theme.primary)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("  Status:  ", Style::default().fg(theme.text_dim)),
                        Span::styled(status_str, Style::default().fg(color)),
                    ]),
                    Line::from(vec![
                        Span::styled("  Created: ", Style::default().fg(theme.text_dim)),
                        Span::styled(&j.created, Style::default().fg(theme.text)),
                    ]),
                ];
                if let Some(ref fin) = j.finished {
                    lines.push(Line::from(vec![
                        Span::styled("  Finished:", Style::default().fg(theme.text_dim)),
                        Span::styled(fin, Style::default().fg(theme.text)),
                    ]));
                }
                if let Some(ref err) = j.error {
                    lines.push(Line::from(vec![
                        Span::styled("  Error:   ", Style::default().fg(theme.text_dim)),
                        Span::styled(err, Style::default().fg(theme.error)),
                    ]));
                }
                let p = Paragraph::new(lines).block(block);
                frame.render_widget(p, area);
            } else {
                let p = Paragraph::new("  No job selected")
                    .block(block)
                    .style(Style::default().fg(theme.text_dim));
                frame.render_widget(p, area);
            }
        }
    }
}

fn render_bottom(frame: &mut Frame, state: &TuiState, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Tunnel / Info ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    let mut lines = Vec::new();
    if let Some(ref t) = state.tunnel_state {
        lines.push(Line::from(vec![
            Span::styled("  Tunnel:  ", Style::default().fg(theme.text_dim)),
            Span::styled("active", Style::default().fg(theme.success)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Port:    ", Style::default().fg(theme.text_dim)),
            Span::styled(t.port.to_string(), Style::default().fg(theme.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Remote:  ", Style::default().fg(theme.text_dim)),
            Span::styled(&t.remote_host, Style::default().fg(theme.text)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Tunnel:  ", Style::default().fg(theme.text_dim)),
            Span::styled(
                "not active (local mode)",
                Style::default().fg(theme.text_dim),
            ),
        ]));
    }

    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  Sessions: {}  Jobs: {}",
            state.sessions.len(),
            state.jobs.len()
        ),
        Style::default().fg(theme.text_dim),
    )]));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_status_bar(frame: &mut Frame, state: &TuiState, theme: &Theme, area: Rect) {
    let msg = if let Some((ref m, at)) = state.status_msg {
        if at.elapsed().as_secs() < 3 {
            m.as_str()
        } else {
            ""
        }
    } else {
        ""
    };

    let hints = " j/k:navigate  Tab:switch  r:refresh  l:log  c:cancel  q:quit";
    let line = if msg.is_empty() {
        Line::from(Span::styled(hints, Style::default().fg(theme.text_dim)))
    } else {
        Line::from(vec![
            Span::styled(format!(" {msg} "), Style::default().fg(theme.accent)),
            Span::styled("│", Style::default().fg(theme.border)),
            Span::styled(hints, Style::default().fg(theme.text_dim)),
        ])
    };
    let p = Paragraph::new(line);
    frame.render_widget(p, area);
}

fn render_log_overlay(frame: &mut Frame, state: &TuiState, theme: &Theme) {
    let area = frame.area();
    let block = Block::default()
        .title(format!(
            " Command Log [{}/{}] ",
            state.log_scroll + 1,
            state.log_lines.len().max(1)
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let visible_height = area.height.saturating_sub(2) as usize;
    let start = state.log_scroll;
    let end = (start + visible_height).min(state.log_lines.len());
    let visible: Vec<Line> = state.log_lines[start..end]
        .iter()
        .map(|l| {
            let color = if l.contains("[SKILL]") {
                theme.primary
            } else if l.contains("error") || l.contains("Error") {
                theme.error
            } else {
                theme.text_dim
            };
            Line::from(Span::styled(l.as_str(), Style::default().fg(color)))
        })
        .collect();

    let p = Paragraph::new(visible)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(p, area);
}
