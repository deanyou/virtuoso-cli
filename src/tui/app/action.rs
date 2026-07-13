use crate::spectre::jobs::JobStatus;
use crate::tui::app::overlay::{LogOverlay, Overlay};
use crate::tui::app::state::{App, StatusKind};

/// Intent produced by the event handler. The event handler must not mutate
/// anything beyond the event-local state; all data-changing operations flow
/// through `handle_action`.
pub enum Action {
    None,
    Quit,
    Refresh,
    CancelJob(usize),
    SaveConfig,
    Status(String, StatusKind),
}

pub fn handle_action(app: &mut App, action: Action) {
    match action {
        Action::None => {}
        Action::Quit => app.should_quit = true,
        Action::Refresh => {
            crate::tui::app::data::refresh(app);
            let lines = crate::tui::app::data::load_log_lines();
            // If a log overlay is already open, refresh its content in place;
            // otherwise just cache nothing — `l` key reopens with fresh data.
            if let Overlay::Log(_) = app.overlay {
                app.overlay = Overlay::Log(LogOverlay::new(lines));
            }
            app.set_status("Refreshed", StatusKind::Ok);
        }
        Action::CancelJob(idx) => {
            if let Some(job) = app.jobs.get_mut(idx) {
                if job.status == JobStatus::Running {
                    let _ = job.cancel();
                    let id = job.id.clone();
                    app.set_status(format!("Cancelled job {id}"), StatusKind::Warn);
                } else {
                    app.set_status("Job not running", StatusKind::Info);
                }
            }
        }
        Action::SaveConfig => match crate::tui::app::data::save_config(app) {
            Ok(_) => app.set_status("Config saved to .env", StatusKind::Ok),
            Err(e) => app.set_status(format!("Save failed: {e}"), StatusKind::Err),
        },
        Action::Status(msg, kind) => app.set_status(msg, kind),
    }
}
