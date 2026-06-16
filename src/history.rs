use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static CMD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cmd_lock() -> std::sync::MutexGuard<'static, ()> {
    CMD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("cmd history lock poisoned")
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkillEntry {
    pub ts: String,
    pub skill: String,
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CmdEntry {
    pub ts: String,
    pub session: Option<String>,
    pub cmd: Vec<String>,
    pub exit_code: i32,
}

pub fn history_dir() -> PathBuf {
    crate::runtime_paths::cache_subdir(&["history"])
}

fn write_jsonl_line(path: &std::path::Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

pub fn append_skill(session_id: &str, skill: &str, ok: bool, output: &str) {
    let dir = history_dir();
    let _ = std::fs::create_dir_all(&dir);
    let entry = SkillEntry {
        ts: Utc::now().to_rfc3339(),
        skill: skill.to_string(),
        ok,
        output: output.chars().take(512).collect(),
    };
    if let Ok(line) = serde_json::to_string(&entry) {
        write_jsonl_line(&dir.join(format!("{session_id}.jsonl")), &line);
    }
}

pub fn append_cmd(args: &[String], session: Option<&str>, exit_code: i32) {
    let _guard = cmd_lock();
    let dir = history_dir();
    let _ = std::fs::create_dir_all(&dir);
    let entry = CmdEntry {
        ts: Utc::now().to_rfc3339(),
        session: session.map(String::from),
        cmd: args.to_vec(),
        exit_code,
    };
    if let Ok(line) = serde_json::to_string(&entry) {
        write_jsonl_line(&dir.join("cmd.jsonl"), &line);
    }
}

fn tail<T>(mut v: Vec<T>, limit: usize) -> Vec<T> {
    if limit > 0 && v.len() > limit {
        v.drain(..v.len() - limit);
    }
    v
}

pub fn load_skill(session_id: &str, limit: usize) -> Vec<SkillEntry> {
    let path = history_dir().join(format!("{session_id}.jsonl"));
    let entries: Vec<SkillEntry> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    tail(entries, limit)
}

pub fn load_cmd(session_filter: Option<&str>, limit: usize) -> Vec<CmdEntry> {
    let _guard = cmd_lock();
    let path = history_dir().join("cmd.jsonl");
    let entries: Vec<CmdEntry> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|e: &CmdEntry| session_filter.is_none_or(|id| e.session.as_deref() == Some(id)))
        .collect();
    tail(entries, limit)
}
