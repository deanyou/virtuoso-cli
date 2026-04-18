use crate::error::{Result, VirtuosoError};
use crate::spec::bandgap::BandgapSpec;
use crate::spectre::batch::{run_batch, BatchJob};
use crate::spectre::jobs::JobStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OptimState {
    pub optim_id: String,
    pub spec_file: String,
    pub netlist_path: String,
    pub max_iter: u32,
    pub iteration: u32,
    pub status: String,
    pub jobs: Vec<BatchJob>,
    pub best: Option<BestResult>,
    pub corner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BestResult {
    pub iteration: u32,
    pub params: HashMap<String, f64>,
    pub raw_dir: String,
}

impl OptimState {
    fn dir() -> PathBuf {
        let dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("virtuoso_bridge")
            .join("optim");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn path(id: &str) -> PathBuf {
        Self::dir().join(format!("{id}.json"))
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| VirtuosoError::Execution(e.to_string()))?;
        fs::write(Self::path(&self.optim_id), json)
            .map_err(|e| VirtuosoError::Execution(e.to_string()))
    }

    fn load(id: &str) -> Result<Self> {
        let path = Self::path(id);
        let json = fs::read_to_string(&path)
            .map_err(|_| VirtuosoError::NotFound(format!("optim job '{id}' not found")))?;
        serde_json::from_str(&json)
            .map_err(|e| VirtuosoError::Execution(format!("bad optim file: {e}")))
    }
}

pub fn run(spec_file: &str, netlist_file: &str, max_iter: u32, timeout: u64) -> Result<Value> {
    let spec = BandgapSpec::from_file(spec_file)?;
    let template = fs::read_to_string(netlist_file)
        .map_err(|e| VirtuosoError::Config(format!("cannot read netlist '{netlist_file}': {e}")))?;

    let combos = spec.param_combos();
    if combos.is_empty() {
        return Err(VirtuosoError::Config(
            "spec produces no parameter combinations".into(),
        ));
    }

    let optim_id = format!("bg-{}", &uuid::Uuid::new_v4().to_string()[..6]);
    let jobs = run_batch(&template, combos, timeout)?;

    let completed = jobs
        .iter()
        .filter(|j| j.status == JobStatus::Completed)
        .count();
    let failed = jobs.len() - completed;
    let status = if failed == 0 {
        "completed"
    } else if completed == 0 {
        "failed"
    } else {
        "partial"
    };

    let best = jobs
        .iter()
        .filter(|j| j.status == JobStatus::Completed)
        .find_map(|j| {
            j.raw_dir.clone().map(|raw_dir| BestResult {
                iteration: 0,
                params: j.params.clone(),
                raw_dir,
            })
        });

    let total = jobs.len();
    let state = OptimState {
        optim_id: optim_id.clone(),
        spec_file: spec_file.to_string(),
        netlist_path: netlist_file.to_string(),
        max_iter,
        iteration: 1,
        status: status.to_string(),
        jobs,
        best,
        corner: spec.corner,
    };
    state.save()?;

    Ok(serde_json::json!({
        "optim_id": optim_id,
        "spec_file": spec_file,
        "corner": state.corner,
        "iteration": 1,
        "max_iter": max_iter,
        "status": status,
        "total_jobs": total,
        "completed": completed,
        "failed": failed,
        "best": state.best,
    }))
}

pub fn status(optim_id: &str) -> Result<Value> {
    let state = OptimState::load(optim_id)?;
    let completed = state
        .jobs
        .iter()
        .filter(|j| j.status == JobStatus::Completed)
        .count();
    let failed = state.jobs.len() - completed;
    Ok(serde_json::json!({
        "optim_id": optim_id,
        "status": state.status,
        "iteration": state.iteration,
        "max_iter": state.max_iter,
        "total_jobs": state.jobs.len(),
        "completed": completed,
        "failed": failed,
        "best": state.best,
        "corner": state.corner,
    }))
}

pub fn report(optim_id: &str, output: Option<&str>) -> Result<Value> {
    let state = OptimState::load(optim_id)?;

    let mut md = String::new();
    md.push_str("# Bandgap Optimization Report\n\n");
    let _ = writeln!(md, "**Optim ID:** `{}`  ", state.optim_id);
    let _ = writeln!(md, "**Spec:** {}  ", state.spec_file);
    let _ = writeln!(md, "**Corner:** {}  ", state.corner);
    let _ = writeln!(md, "**Status:** {}  \n", state.status);

    md.push_str("## Iteration Summary\n\n");
    let _ = write!(
        md,
        "Iteration {}/{} — {} jobs total\n\n",
        state.iteration,
        state.max_iter,
        state.jobs.len()
    );

    md.push_str("## Parameter Sweep Results\n\n");
    md.push_str("| Status | Params | Raw Dir |\n");
    md.push_str("|--------|--------|--------|\n");
    for job in &state.jobs {
        let params_str: Vec<String> = job
            .params
            .iter()
            .map(|(k, v)| format!("{k}={v:.3e}"))
            .collect();
        let raw = job.raw_dir.as_deref().unwrap_or("-");
        let _ = writeln!(
            md,
            "| {} | {} | {} |",
            serde_json::to_value(job.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            params_str.join(", "),
            raw
        );
    }

    if let Some(ref best) = state.best {
        md.push_str("\n## Best Result\n\n");
        let _ = writeln!(md, "**Iteration:** {}  ", best.iteration);
        for (k, v) in &best.params {
            let _ = writeln!(md, "**{k}:** {v:.3e}  ");
        }
        let _ = writeln!(md, "**Raw dir:** `{}`  ", best.raw_dir);
    }

    if let Some(path) = output {
        fs::write(path, &md)
            .map_err(|e| VirtuosoError::Execution(format!("cannot write report: {e}")))?;
        Ok(serde_json::json!({"written": path, "optim_id": optim_id}))
    } else {
        Ok(serde_json::json!({"report": md, "optim_id": optim_id}))
    }
}
