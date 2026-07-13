pub mod chrome;
pub mod config;
pub mod content;
pub mod footer;
pub mod jobs;
pub mod overlay;
pub mod sessions;
pub mod shared;

use crate::tui::app::state::App;
use crate::tui::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

pub fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header chrome
            Constraint::Min(3),    // content
            Constraint::Length(1), // footer status
        ])
        .split(area);

    chrome::render_header(frame, app, theme, chunks[0]);
    content::render_content(frame, app, theme, chunks[1]);
    footer::render_footer(frame, app, theme, chunks[2]);

    if app.overlay.is_active() {
        overlay::render::render_overlay(frame, app, theme, chunks[1]);
    }
}
