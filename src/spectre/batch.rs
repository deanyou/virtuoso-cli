use crate::error::{Result, VirtuosoError};
use crate::spectre::jobs::{Job, JobStatus};
use crate::spectre::runner::SpectreSimulator;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchJob {
    pub params: HashMap<String, f64>,
    pub job_id: String,
    pub status: JobStatus,
    pub error: Option<String>,
    pub raw_dir: Option<String>,
}

/// Replace `${KEY}` placeholders in a netlist template with param values.
pub fn render_template(template: &str, params: &HashMap<String, f64>) -> Result<String> {
    let mut result = template.to_string();
    for (key, val) in params {
        let placeholder = format!("${{{key}}}");
        if !result.contains(&placeholder) {
            return Err(VirtuosoError::Config(format!(
                "template missing placeholder for param '{key}'"
            )));
        }
        result = result.replace(&placeholder, &format!("{val:.6e}"));
    }
    if let Some(start) = result.find("${") {
        let end = result[start..].find('}').unwrap_or(start + 3);
        let token = &result[start..=start + end];
        return Err(VirtuosoError::Config(format!(
            "template has unresolved placeholder: {token}"
        )));
    }
    Ok(result)
}

/// Run multiple Spectre jobs from a netlist template and a list of param combos.
/// Polls until all jobs finish or `timeout_secs` elapses.
pub fn run_batch(
    template: &str,
    combos: Vec<HashMap<String, f64>>,
    timeout_secs: u64,
) -> Result<Vec<BatchJob>> {
    let sim = SpectreSimulator::from_env()?;

    let mut batch: Vec<BatchJob> = Vec::with_capacity(combos.len());
    for params in combos {
        let netlist = render_template(template, &params)?;
        let job = sim.run_async(&netlist)?;
        batch.push(BatchJob {
            params,
            job_id: job.id.clone(),
            status: JobStatus::Running,
            error: None,
            raw_dir: job.raw_dir.clone(),
        });
    }

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if batch.iter().all(|b| b.status != JobStatus::Running) {
            break;
        }
        if Instant::now() >= deadline {
            for b in batch.iter_mut() {
                if b.status == JobStatus::Running {
                    b.status = JobStatus::Failed;
                    b.error = Some(format!("timeout after {timeout_secs}s"));
                }
            }
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
        for b in batch.iter_mut() {
            if b.status != JobStatus::Running {
                continue;
            }
            if let Ok(mut j) = Job::load(&b.job_id) {
                let _ = j.refresh();
                match j.status {
                    JobStatus::Completed => {
                        b.status = JobStatus::Completed;
                        b.raw_dir = j.raw_dir.clone();
                    }
                    JobStatus::Failed => {
                        b.status = JobStatus::Failed;
                        b.error = j.error.clone();
                    }
                    JobStatus::Cancelled => {
                        b.status = JobStatus::Cancelled;
                    }
                    JobStatus::Running => {}
                }
            }
        }
    }

    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_substitution() {
        let template = "W=${W} L=${L}";
        let mut params = HashMap::new();
        params.insert("W".into(), 2.0e-6_f64);
        params.insert("L".into(), 5.0e-7_f64);
        let result = render_template(template, &params).unwrap();
        assert!(result.contains("2.000000e-6") || result.contains("2.000000e"));
        assert!(result.contains("5.000000e-7") || result.contains("5.000000e"));
    }

    #[test]
    fn test_template_missing_param() {
        let template = "W=${W} L=${L}";
        let mut params = HashMap::new();
        params.insert("W".into(), 2.0e-6_f64);
        assert!(render_template(template, &params).is_err());
    }

    #[test]
    fn test_template_all_resolved() {
        let template = "W=${W} extra=${EXTRA}";
        let mut params = HashMap::new();
        params.insert("W".into(), 1.0e-6_f64);
        params.insert("EXTRA".into(), 0.5_f64);
        assert!(render_template(template, &params).is_ok());
    }

    #[test]
    fn test_template_extra_placeholder_fails() {
        let template = "W=${W} ${UNKNOWN}";
        let mut params = HashMap::new();
        params.insert("W".into(), 1.0e-6_f64);
        assert!(render_template(template, &params).is_err());
    }

    #[test]
    fn test_single_param_combo_render() {
        let template = "parameters W=${W}";
        let mut params = HashMap::new();
        params.insert("W".into(), 3.0e-6_f64);
        let result = render_template(template, &params).unwrap();
        assert!(result.contains("W="));
        assert!(!result.contains("${W}"));
    }
}
