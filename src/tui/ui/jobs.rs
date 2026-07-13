use crate::spectre::jobs::JobStatus;
use crate::tui::app::state::App;
use crate::tui::theme::Theme;
use crate::tui::ui::shared::{kv_line, pane_border_style, selection_style};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

const SPINNERS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

fn job_icon(job: &crate::spectre::jobs::Job, frame_idx: usize) -> &'static str {
    match job.status {
        JobStatus::Completed => "✓",
        JobStatus::Running => SPINNERS[frame_idx % SPINNERS.len()],
        JobStatus::Failed => "✗",
        JobStatus::Cancelled => "⊘",
    }
}

fn job_color(status: JobStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        JobStatus::Completed => theme.success,
        JobStatus::Running => theme.accent,
        JobStatus::Failed => theme.error,
        JobStatus::Cancelled => theme.text_dim,
    }
}

pub fn render_list(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Jobs ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, false));

    if app.jobs.is_empty() {
        let p = Paragraph::new(" No jobs")
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
        .jobs
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let icon = job_icon(j, app.spinner_frame);
            let color = job_color(j.status, theme);
            let icon_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(color)
            };
            let row_style = if i == app.selected_job {
                selection_style(theme)
            } else if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {icon} "), icon_style),
                Span::styled(j.id.clone(), row_style),
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

    let Some(j) = app.jobs.get(app.selected_job) else {
        let p = Paragraph::new("  No job selected")
            .block(block)
            .style(if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            });
        frame.render_widget(p, area);
        return;
    };

    let (status_str, status_color) = match j.status {
        JobStatus::Completed => ("completed", theme.success),
        JobStatus::Running => ("running", theme.accent),
        JobStatus::Failed => ("failed", theme.error),
        JobStatus::Cancelled => ("cancelled", theme.text_dim),
    };

    let mut lines = vec![
        kv_line("  Job ID:  ", &j.id, theme, Some(theme.primary)),
        kv_line("  Status:  ", status_str, theme, Some(status_color)),
        kv_line("  Created: ", &j.created, theme, None),
    ];
    if let Some(ref fin) = j.finished {
        lines.push(kv_line("  Finished:", fin, theme, None));
    }
    if let Some(ref e) = j.error {
        lines.push(kv_line("  Error:   ", e, theme, Some(theme.error)));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn render_summary(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = Block::default()
        .title(" Summary ")
        .borders(Borders::ALL)
        .border_style(pane_border_style(theme, false));

    let running = app
        .jobs
        .iter()
        .filter(|j| j.status == JobStatus::Running)
        .count();
    let completed = app
        .jobs
        .iter()
        .filter(|j| j.status == JobStatus::Completed)
        .count();
    let failed = app
        .jobs
        .iter()
        .filter(|j| j.status == JobStatus::Failed)
        .count();

    let lines = vec![
        kv_line(
            "  Running:   ",
            &running.to_string(),
            theme,
            Some(theme.accent),
        ),
        kv_line(
            "  Completed: ",
            &completed.to_string(),
            theme,
            Some(theme.success),
        ),
        kv_line(
            "  Failed:    ",
            &failed.to_string(),
            theme,
            Some(theme.error),
        ),
        Line::default(),
        Line::from(Span::styled(
            "  x=cancel  ?=help",
            if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_dim)
            },
        )),
    ];

    frame.render_widget(Paragraph::new(lines).block(block), area);
}
