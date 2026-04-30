//! Interactive terminal UI — entry point and event loop.
//!
//! Architecture (borrowed from cc-switch-cli):
//! - `app/`  — App state, overlay enum, event cascade, action handler
//! - `ui/`   — pure rendering: chrome (header), content tabs, overlays, footer
//! - `theme` — colors + no_color accessibility mode
//!
//! Input priority cascade: overlay → globals → tab content. An active overlay
//! suppresses all other keys, so vim motions inside a log viewer never leak
//! into tab switching.

pub mod app;
pub mod theme;
pub mod ui;

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

pub fn run_tui() -> Result<()> {
    let mut app = app::state::App::new();
    let theme = theme::Theme::detect();

    enable_raw_mode().map_err(err)?;
    stdout().execute(EnterAlternateScreen).map_err(err)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend).map_err(err)?;

    let result = run_loop(&mut terminal, &mut app, &theme);

    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut app::state::App,
    theme: &theme::Theme,
) -> Result<()> {
    loop {
        terminal
            .draw(|frame| ui::draw(frame, app, theme))
            .map_err(err)?;

        if !event::poll(Duration::from_millis(500)).map_err(err)? {
            app.spinner_frame = app.spinner_frame.wrapping_add(1);
            app.clear_expired_status();
            app.refresh_daemon_stats();
            continue;
        }

        if let Event::Key(key) = event::read().map_err(err)? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let action = app::on_key(app, key);
            app::handle_action(app, action);
            if app.should_quit {
                break;
            }
        }
    }
    Ok(())
}
