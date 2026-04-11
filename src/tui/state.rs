use crate::models::{SessionInfo, TunnelState};
use crate::spectre::jobs::Job;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
pub enum ActiveTab {
    Sessions,
    Jobs,
}

pub struct TuiState {
    pub sessions: Vec<SessionInfo>,
    pub jobs: Vec<Job>,
    pub tunnel_state: Option<TunnelState>,
    pub selected_session: usize,
    pub selected_job: usize,
    pub active_tab: ActiveTab,
    pub status_msg: Option<(String, Instant)>,
    pub show_log: bool,
    pub log_lines: Vec<String>,
    pub log_scroll: usize,
    pub spinner_frame: usize,
}

impl TuiState {
    pub fn new() -> Self {
        let sessions = SessionInfo::list().unwrap_or_default();
        let mut jobs = Job::list_all().unwrap_or_default();
        for j in &mut jobs {
            let _ = j.refresh();
        }
        let tunnel_state = TunnelState::load().ok().flatten();

        Self {
            sessions,
            jobs,
            tunnel_state,
            selected_session: 0,
            selected_job: 0,
            active_tab: ActiveTab::Sessions,
            status_msg: None,
            show_log: false,
            log_lines: Vec::new(),
            log_scroll: 0,
            spinner_frame: 0,
        }
    }

    pub fn refresh(&mut self) {
        self.sessions = SessionInfo::list().unwrap_or_default();
        let mut jobs = Job::list_all().unwrap_or_default();
        for j in &mut jobs {
            let _ = j.refresh();
        }
        self.jobs = jobs;
        self.tunnel_state = TunnelState::load().ok().flatten();
    }

    pub fn set_status(&mut self, msg: &str) {
        self.status_msg = Some((msg.to_string(), Instant::now()));
    }

    pub fn selected_session_info(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.selected_session)
    }
}
