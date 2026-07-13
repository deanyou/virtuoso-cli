use crate::tui::app::state::{App, Tab};
use crate::tui::theme::Theme;
use crate::tui::ui::{config, jobs, sessions};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

pub fn render_content(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    // Left nav list (tab-specific) + right detail pane split 50/50 vertically
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(30)])
        .split(area);

    let (nav_area, detail_area) = (cols[0], cols[1]);

    let detail_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(detail_area);
    let (detail_top, detail_bot) = (detail_rows[0], detail_rows[1]);

    match app.tab {
        Tab::Sessions => {
            sessions::render_list(frame, app, theme, nav_area);
            sessions::render_detail(frame, app, theme, detail_top);
            sessions::render_tunnel(frame, app, theme, detail_bot);
        }
        Tab::Jobs => {
            jobs::render_list(frame, app, theme, nav_area);
            jobs::render_detail(frame, app, theme, detail_top);
            jobs::render_summary(frame, app, theme, detail_bot);
        }
        Tab::Config => {
            config::render_list(frame, app, theme, nav_area);
            config::render_detail(frame, app, theme, detail_top);
            config::render_hint(frame, app, theme, detail_bot);
        }
    }
}
