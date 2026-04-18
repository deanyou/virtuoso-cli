use crate::error::{Result, VirtuosoError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandgapSpec {
    pub ip_type: String,
    pub target: TargetSpec,
    pub params: HashMap<String, ParamRange>,
    #[serde(default = "default_corner")]
    pub corner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSpec {
    #[serde(rename = "Vbg")]
    pub vbg: f64,
    #[serde(rename = "PSRR")]
    pub psrr: Option<f64>,
    #[serde(rename = "TC")]
    pub tc: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

fn default_corner() -> String {
    "tt".to_string()
}

impl BandgapSpec {
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| VirtuosoError::Config(format!("cannot read spec file '{path}': {e}")))?;
        let spec: Self = serde_yaml::from_str(&content)
            .map_err(|e| VirtuosoError::Config(format!("invalid YAML in '{path}': {e}")))?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<()> {
        for (name, range) in &self.params {
            if range.min > range.max {
                return Err(VirtuosoError::Config(format!(
                    "param '{name}': min ({}) > max ({})",
                    range.min, range.max
                )));
            }
            if range.min <= 0.0 || range.max <= 0.0 {
                return Err(VirtuosoError::Config(format!(
                    "param '{name}': W/L values must be positive"
                )));
            }
        }
        Ok(())
    }

    pub fn param_combos(&self) -> Vec<HashMap<String, f64>> {
        let mut combos: Vec<HashMap<String, f64>> = vec![HashMap::new()];
        for (name, range) in &self.params {
            let steps = ((range.max - range.min) / range.step).round() as usize;
            let mut next = Vec::new();
            for i in 0..=steps {
                let v = range.min + range.step * i as f64;
                for base in &combos {
                    let mut c = base.clone();
                    c.insert(name.clone(), v);
                    next.push(c);
                }
            }
            combos = next;
        }
        combos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_yaml(yaml: &str) -> BandgapSpec {
        let spec: BandgapSpec = serde_yaml::from_str(yaml).expect("valid yaml");
        spec
    }

    const VALID_YAML: &str = r#"
ip_type: bandgap
target:
  Vbg: 1.20
  PSRR: 80
  TC: 20
params:
  W:
    min: 1.0e-6
    max: 5.0e-6
    step: 1.0e-6
  L:
    min: 0.18e-6
    max: 1.0e-6
    step: 0.18e-6
corner: tt
"#;

    #[test]
    fn test_valid_bandgap_yaml() {
        let spec = parse_yaml(VALID_YAML);
        assert_eq!(spec.ip_type, "bandgap");
        assert!((spec.target.vbg - 1.20).abs() < 1e-9);
        assert_eq!(spec.corner, "tt");
        assert!(spec.params.contains_key("W"));
        assert!(spec.params.contains_key("L"));
        spec.validate().expect("should be valid");
    }

    #[test]
    fn test_missing_vbg_target() {
        let yaml = r#"
ip_type: bandgap
target:
  PSRR: 80
params:
  W:
    min: 1.0e-6
    max: 5.0e-6
    step: 1.0e-6
"#;
        let result: std::result::Result<BandgapSpec, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "missing Vbg should fail parse");
    }

    #[test]
    fn test_negative_w_or_l() {
        let yaml = r#"
ip_type: bandgap
target:
  Vbg: 1.20
params:
  W:
    min: -1.0e-6
    max: 5.0e-6
    step: 1.0e-6
"#;
        let spec: BandgapSpec = serde_yaml::from_str(yaml).expect("parses");
        assert!(spec.validate().is_err(), "negative min should fail validation");
    }

    #[test]
    fn test_default_corner() {
        let yaml = r#"
ip_type: bandgap
target:
  Vbg: 1.20
params:
  W:
    min: 1.0e-6
    max: 5.0e-6
    step: 1.0e-6
"#;
        let spec: BandgapSpec = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(spec.corner, "tt");
    }

    #[test]
    fn test_param_range_order() {
        let yaml = r#"
ip_type: bandgap
target:
  Vbg: 1.20
params:
  W:
    min: 5.0e-6
    max: 1.0e-6
    step: 1.0e-6
"#;
        let spec: BandgapSpec = serde_yaml::from_str(yaml).expect("parses");
        assert!(spec.validate().is_err(), "min > max should fail");
    }

    #[test]
    fn test_single_param_combo() {
        let yaml = r#"
ip_type: bandgap
target:
  Vbg: 1.20
params:
  W:
    min: 2.0e-6
    max: 2.0e-6
    step: 1.0e-6
"#;
        let spec: BandgapSpec = serde_yaml::from_str(yaml).expect("parses");
        let combos = spec.param_combos();
        assert_eq!(combos.len(), 1);
        assert!((combos[0]["W"] - 2.0e-6).abs() < 1e-15);
    }
}
