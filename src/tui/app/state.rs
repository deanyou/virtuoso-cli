use crate::models::{SessionInfo, TunnelState};
use crate::spectre::jobs::Job;
use crate::tui::app::overlay::Overlay;
use std::time::Instant;


#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sessions,
    Jobs,
    Config,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Sessions => "Sessions",
            Tab::Jobs => "Jobs",
            Tab::Config => "Config",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::Sessions => Tab::Jobs,
            Tab::Jobs => Tab::Config,
            Tab::Config => Tab::Sessions,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Sessions => Tab::Config,
            Tab::Jobs => Tab::Sessions,
            Tab::Config => Tab::Jobs,
        }
    }

    pub fn all() -> [Tab; 3] {
        [Tab::Sessions, Tab::Jobs, Tab::Config]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Ok,
    Warn,
    Err,
}

pub struct StatusToast {
    pub message: String,
    pub kind: StatusKind,
    pub at: Instant,
}

/// A single .env config field.
pub struct ConfigField {
    pub key: String,
    pub value: String,
    pub hint: &'static str,
}

pub struct App {
    pub tab: Tab,
    pub overlay: Overlay,

    pub sessions: Vec<SessionInfo>,
    pub jobs: Vec<Job>,
    pub tunnel_state: Option<TunnelState>,
    pub config_fields: Vec<ConfigField>,

    pub selected_session: usize,
    pub selected_job: usize,
    pub selected_config: usize,

    pub spinner_frame: usize,
    pub status: Option<StatusToast>,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            tab: Tab::Sessions,
            overlay: Overlay::None,
            sessions: Vec::new(),
            jobs: Vec::new(),
            tunnel_state: None,
            config_fields: Vec::new(),
            selected_session: 0,
            selected_job: 0,
            selected_config: 0,
            spinner_frame: 0,
            status: None,
            should_quit: false,
        };
        crate::tui::app::data::initial_load(&mut app);
        app
    }

    pub fn set_status(&mut self, message: impl Into<String>, kind: StatusKind) {
        self.status = Some(StatusToast {
            message: message.into(),
            kind,
            at: Instant::now(),
        });
    }

    pub fn clear_expired_status(&mut self) {
        if let Some(s) = &self.status {
            if s.at.elapsed().as_secs() >= 3 {
                self.status = None;
            }
        }
    }

    pub fn selected_session_info(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.selected_session)
    }

    pub fn selected_config_field(&self) -> Option<&ConfigField> {
        self.config_fields.get(self.selected_config)
    }

    /// Wrap-safe cursor movement for the active tab's list.
    pub fn move_selection(&mut self, delta: i64) {
        let (cursor, len) = match self.tab {
            Tab::Sessions => (&mut self.selected_session, self.sessions.len()),
            Tab::Jobs => (&mut self.selected_job, self.jobs.len()),
            Tab::Config => (&mut self.selected_config, self.config_fields.len()),
        };
        if len == 0 {
            *cursor = 0;
            return;
        }
        let new = (*cursor as i64 + delta).rem_euclid(len as i64);
        *cursor = new as usize;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
