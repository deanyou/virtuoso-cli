use crate::tui::app::overlay::Overlay;
use crate::tui::app::state::App;
use crate::tui::theme::Theme;
use crate::tui::ui::overlay::{confirm, form, help, log};
use ratatui::layout::Rect;
use ratatui::Frame;

pub fn render_overlay(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    match &app.overlay {
        Overlay::None => {}
        Overlay::Log(log_state) => log::render(frame, log_state, theme, area),
        Overlay::Confirm(c) => confirm::render(frame, c, theme, area),
        Overlay::Form(f) => form::render(frame, f, theme, area),
        Overlay::Help => help::render(frame, theme, area),
    }
}
