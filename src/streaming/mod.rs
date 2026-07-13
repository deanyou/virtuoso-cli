//! Streaming job events — progress updates for long-running simulations.
//!
//! Events are emitted via `JobEventSink` implementations as jobs progress.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Job lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    Started {
        job_id: String,
        created: String,
    },
    Progress {
        job_id: String,
        percent: f32,
        message: String,
        iteration: Option<u64>,
        time_point: Option<String>,
    },
    Completed {
        job_id: String,
        duration_ms: u64,
        errors: Vec<String>,
    },
    Failed {
        job_id: String,
        error: String,
    },
    Cancelled {
        job_id: String,
    },
}

impl fmt::Display for JobEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobEvent::Started { job_id, .. } => {
                write!(f, "job {} started", job_id)
            }
            JobEvent::Progress {
                job_id,
                percent,
                message,
                ..
            } => {
                write!(f, "job {}: {:.1}% — {}", job_id, percent, message)
            }
            JobEvent::Completed {
                job_id,
                duration_ms,
                ..
            } => {
                write!(f, "job {} completed in {}ms", job_id, duration_ms)
            }
            JobEvent::Failed { job_id, error } => {
                write!(f, "job {} failed: {}", job_id, error)
            }
            JobEvent::Cancelled { job_id } => {
                write!(f, "job {} cancelled", job_id)
            }
        }
    }
}

/// Sink for job events — implement this to receive streaming updates.
pub trait JobEventSink: Send + Sync {
    fn emit(&self, event: JobEvent);
}

/// Console sink — writes events to stderr as JSON lines.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ConsoleSink;

impl JobEventSink for ConsoleSink {
    fn emit(&self, event: JobEvent) {
        eprintln!("{}", serde_json::to_string(&event).unwrap_or_default());
    }
}

/// Null sink — discards all events.
#[derive(Debug, Clone, Copy)]
pub struct NullSink;

impl JobEventSink for NullSink {
    fn emit(&self, _event: JobEvent) {}
}

/// Broadcast sink — fans out to multiple sinks.
#[allow(dead_code)]
pub struct BroadcastSink {
    sinks: Vec<Arc<dyn JobEventSink>>,
}

impl BroadcastSink {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    #[allow(dead_code)]
    pub fn add_sink(&mut self, sink: Arc<dyn JobEventSink>) {
        self.sinks.push(sink);
    }
}

impl Default for BroadcastSink {
    fn default() -> Self {
        Self::new()
    }
}

impl JobEventSink for BroadcastSink {
    fn emit(&self, event: JobEvent) {
        for sink in &self.sinks {
            sink.emit(event.clone());
        }
    }
}

/// Thread-safe single-consumer event channel.
/// Uses std::sync::mpsc internally; tokio integration comes in P1-1.
#[allow(dead_code)]
pub struct ChannelSink {
    tx: std::sync::Mutex<std::sync::mpsc::Sender<JobEvent>>,
}

impl ChannelSink {
    #[allow(dead_code)]
    pub fn new() -> Self {
        let (tx, _) = std::sync::mpsc::channel();
        Self {
            tx: std::sync::Mutex::new(tx),
        }
    }

    #[allow(dead_code)]
    pub fn subscribe(&mut self) -> Receiver {
        let (_, rx) = std::sync::mpsc::channel();
        Receiver { rx }
    }
}

impl Default for ChannelSink {
    fn default() -> Self {
        Self::new()
    }
}

impl JobEventSink for ChannelSink {
    fn emit(&self, event: JobEvent) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(event);
        }
    }
}

/// Receiver end of a channel (stub for now).
#[allow(dead_code)]
pub struct Receiver {
    rx: std::sync::mpsc::Receiver<JobEvent>,
}

impl Receiver {
    #[allow(dead_code)]
    pub fn try_recv(&mut self) -> Result<JobEvent, std::sync::mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    #[allow(dead_code)]
    pub fn recv(&mut self) -> Result<JobEvent, std::sync::mpsc::RecvError> {
        self.rx.recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_event_serialization() {
        let event = JobEvent::Started {
            job_id: "abc123".into(),
            created: "2024-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("started"));
        assert!(json.contains("abc123"));
    }

    #[test]
    fn job_event_progress_display() {
        let event = JobEvent::Progress {
            job_id: "xyz".into(),
            percent: 45.0,
            message: "t=1200u/3n".into(),
            iteration: Some(1200),
            time_point: Some("1200u".into()),
        };
        assert!(event.to_string().contains("45.0%"));
        assert!(event.to_string().contains("1200u/3n"));
    }
}
