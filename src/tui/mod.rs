mod input;
mod render;
mod state;
mod theme;

use crate::command_log;
use crate::error::Result;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::time::Duration;

fn err(e: impl std::fmt::Display) -> crate::error::VirtuosoError {
    crate::error::VirtuosoError::Execution(e.to_string())
}

fn load_log(state: &mut state::TuiState) {
    if let Ok(content) = std::fs::read_to_string(command_log::log_path()) {
        state.log_lines = content.lines().map(|l| l.to_string()).collect();
        state.log_scroll = state.log_lines.len().saturating_sub(1);
    }
}

pub fn run_tui() -> Result<()> {
    let mut state = state::TuiState::new();
    let theme = theme::Theme::default();
    load_log(&mut state);

    enable_raw_mode().map_err(err)?;
    stdout().execute(EnterAlternateScreen).map_err(err)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend).map_err(err)?;

    let result = run_loop(&mut terminal, &mut state, &theme);

    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut state::TuiState,
    theme: &theme::Theme,
) -> Result<()> {
    loop {
        terminal
            .draw(|frame| render::render(frame, state, theme))
            .map_err(err)?;

        if !event::poll(Duration::from_millis(500)).map_err(err)? {
            state.spinner_frame = state.spinner_frame.wrapping_add(1);
            if let Some((_, at)) = &state.status_msg {
                if at.elapsed().as_secs() >= 3 {
                    state.status_msg = None;
                }
            }
            continue;
        }

        let ev = event::read().map_err(err)?;

        if let Event::Key(key) = ev {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match input::handle_key(state, key) {
                input::EventAction::Quit => break,
                input::EventAction::Refresh => {
                    state.refresh();
                    load_log(state);
                    state.set_status("Refreshed");
                }
                input::EventAction::ShowLog => {
                    state.show_log = true;
                }
                input::EventAction::CancelJob => {
                    let idx = state.selected_job;
                    if let Some(job) = state.jobs.get_mut(idx) {
                        if job.status == crate::spectre::jobs::JobStatus::Running {
                            let _ = job.cancel();
                        }
                    }
                    if let Some(job) = state.jobs.get(idx) {
                        state.set_status(&format!("Cancelled job {}", job.id));
                    }
                }
                input::EventAction::Continue => {}
            }
        }
    }

    Ok(())
}
