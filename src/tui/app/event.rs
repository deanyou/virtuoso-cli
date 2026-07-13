use crate::spectre::jobs::JobStatus;
use crate::tui::app::action::Action;
use crate::tui::app::overlay::{
    ConfigFormState, ConfirmAction, ConfirmOverlay, LogOverlay, Overlay, TextInput,
};
use crate::tui::app::state::{App, Tab};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Priority cascade: overlay → globals → tab content. An active overlay
/// suppresses all other keys so vim motions inside a log viewer never leak
/// into tab switching.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    if app.overlay.is_active() {
        return overlay_key(app, key);
    }

    // Globals first — available everywhere except inside an overlay.
    if let Some(a) = global_key(app, key) {
        return a;
    }

    // Tab content
    content_key(app, key)
}

fn global_key(app: &mut App, key: KeyEvent) -> Option<Action> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
        (KeyCode::Char('r'), _) => Some(Action::Refresh),
        (KeyCode::Char('l'), _) => {
            app.overlay = Overlay::Log(LogOverlay::new(crate::tui::app::data::load_log_lines()));
            Some(Action::None)
        }
        (KeyCode::Char('?'), _) => {
            app.overlay = Overlay::Help;
            Some(Action::None)
        }
        (KeyCode::Tab, _) | (KeyCode::Char(']'), _) => {
            app.tab = app.tab.next();
            Some(Action::None)
        }
        (KeyCode::BackTab, _) | (KeyCode::Char('['), _) => {
            app.tab = app.tab.prev();
            Some(Action::None)
        }
        _ => None,
    }
}

fn content_key(app: &mut App, key: KeyEvent) -> Action {
    match (key.code, app.tab) {
        // j/k or arrow navigation across all tabs
        (KeyCode::Char('j') | KeyCode::Down, _) => {
            app.move_selection(1);
            Action::None
        }
        (KeyCode::Char('k') | KeyCode::Up, _) => {
            app.move_selection(-1);
            Action::None
        }

        // Config: enter the form overlay for the selected field
        (KeyCode::Enter | KeyCode::Char('i'), Tab::Config) => {
            if let Some(field) = app.selected_config_field() {
                app.overlay = Overlay::Form(ConfigFormState {
                    field_idx: app.selected_config,
                    key: field.key.clone(),
                    hint: field.hint,
                    value: TextInput::new(&field.value),
                });
            }
            Action::None
        }

        // Jobs: 'c' would normally be a global (quit-like), but per the cascade
        // globals already handled Ctrl-C. Plain 'c' in Jobs tab falls through to
        // content to open the cancel confirm overlay.
        (KeyCode::Char('x'), Tab::Jobs) => cancel_job_prompt(app),

        _ => Action::None,
    }
}

fn cancel_job_prompt(app: &mut App) -> Action {
    let Some(job) = app.jobs.get(app.selected_job) else {
        return Action::None;
    };
    if job.status != JobStatus::Running {
        return Action::Status(
            "Selected job is not running".into(),
            crate::tui::app::state::StatusKind::Info,
        );
    }
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: "Cancel Job".into(),
        message: format!("Kill running job {}?", job.id),
        action: ConfirmAction::CancelJob(app.selected_job),
    });
    Action::None
}

fn overlay_key(app: &mut App, key: KeyEvent) -> Action {
    // Split dispatch from mutation to avoid holding a borrow on `app.overlay`
    // while also needing a mutable borrow on `app` itself.
    match &app.overlay {
        Overlay::None => Action::None,
        Overlay::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                app.overlay = Overlay::None;
            }
            Action::None
        }
        Overlay::Log(_) => log_overlay_key(app, key),
        Overlay::Confirm(_) => confirm_overlay_key(app, key),
        Overlay::Form(_) => form_overlay_key(app, key),
    }
}

fn log_overlay_key(app: &mut App, key: KeyEvent) -> Action {
    let Overlay::Log(log) = &mut app.overlay else {
        return Action::None;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('l') => {
            app.overlay = Overlay::None;
        }
        KeyCode::Char('j') | KeyCode::Down if log.scroll + 1 < log.lines.len() => {
            log.scroll += 1;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            log.scroll = log.scroll.saturating_sub(1);
        }
        KeyCode::Char('g') => log.scroll = 0,
        KeyCode::Char('G') => log.scroll = log.lines.len().saturating_sub(1),
        _ => {}
    }
    Action::None
}

fn confirm_overlay_key(app: &mut App, key: KeyEvent) -> Action {
    let Overlay::Confirm(confirm) = &app.overlay else {
        return Action::None;
    };
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
            let action = match confirm.action {
                ConfirmAction::CancelJob(idx) => Action::CancelJob(idx),
            };
            app.overlay = Overlay::None;
            action
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.overlay = Overlay::None;
            Action::None
        }
        _ => Action::None,
    }
}

fn form_overlay_key(app: &mut App, key: KeyEvent) -> Action {
    let Overlay::Form(form) = &mut app.overlay else {
        return Action::None;
    };
    match key.code {
        KeyCode::Esc => {
            app.overlay = Overlay::None;
            Action::None
        }
        KeyCode::Enter => {
            let new_value = form.value.as_str().to_string();
            let idx = form.field_idx;
            if let Some(field) = app.config_fields.get_mut(idx) {
                field.value = new_value;
            }
            app.overlay = Overlay::None;
            Action::SaveConfig
        }
        KeyCode::Backspace => {
            form.value.backspace();
            Action::None
        }
        KeyCode::Left => {
            form.value.move_left();
            Action::None
        }
        KeyCode::Right => {
            form.value.move_right();
            Action::None
        }
        KeyCode::Home => {
            form.value.home();
            Action::None
        }
        KeyCode::End => {
            form.value.end();
            Action::None
        }
        KeyCode::Char('a') if key.modifiers == KeyModifiers::CONTROL => {
            form.value.home();
            Action::None
        }
        KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => {
            form.value.end();
            Action::None
        }
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
            form.value.value.drain(..form.value.cursor);
            form.value.cursor = 0;
            Action::None
        }
        KeyCode::Char(c) => {
            form.value.insert_char(c);
            Action::None
        }
        _ => Action::None,
    }
}
