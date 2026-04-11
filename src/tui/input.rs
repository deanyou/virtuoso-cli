use crate::tui::state::{ActiveTab, TuiState};
use crossterm::event::{KeyCode, KeyEvent};

pub enum EventAction {
    Continue,
    Quit,
    Refresh,
    ShowLog,
    CancelJob,
}

pub fn handle_key(state: &mut TuiState, key: KeyEvent) -> EventAction {
    // Log overlay has its own keys
    if state.show_log {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('l') => {
                state.show_log = false;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if state.log_scroll + 1 < state.log_lines.len() {
                    state.log_scroll += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.log_scroll = state.log_scroll.saturating_sub(1);
            }
            KeyCode::Char('G') => {
                state.log_scroll = state.log_lines.len().saturating_sub(1);
            }
            KeyCode::Char('g') => {
                state.log_scroll = 0;
            }
            _ => {}
        }
        return EventAction::Continue;
    }

    match key.code {
        KeyCode::Char('q') => EventAction::Quit,
        KeyCode::Char('r') => EventAction::Refresh,
        KeyCode::Char('l') => EventAction::ShowLog,
        KeyCode::Char('c') => EventAction::CancelJob,

        KeyCode::Tab => {
            state.active_tab = match state.active_tab {
                ActiveTab::Sessions => ActiveTab::Jobs,
                ActiveTab::Jobs => ActiveTab::Sessions,
            };
            EventAction::Continue
        }

        KeyCode::Char('j') | KeyCode::Down => {
            match state.active_tab {
                ActiveTab::Sessions => {
                    if !state.sessions.is_empty() {
                        state.selected_session =
                            (state.selected_session + 1) % state.sessions.len();
                    }
                }
                ActiveTab::Jobs => {
                    if !state.jobs.is_empty() {
                        state.selected_job = (state.selected_job + 1) % state.jobs.len();
                    }
                }
            }
            EventAction::Continue
        }

        KeyCode::Char('k') | KeyCode::Up => {
            match state.active_tab {
                ActiveTab::Sessions => {
                    if !state.sessions.is_empty() {
                        state.selected_session = if state.selected_session == 0 {
                            state.sessions.len() - 1
                        } else {
                            state.selected_session - 1
                        };
                    }
                }
                ActiveTab::Jobs => {
                    if !state.jobs.is_empty() {
                        state.selected_job = if state.selected_job == 0 {
                            state.jobs.len() - 1
                        } else {
                            state.selected_job - 1
                        };
                    }
                }
            }
            EventAction::Continue
        }

        _ => EventAction::Continue,
    }
}
