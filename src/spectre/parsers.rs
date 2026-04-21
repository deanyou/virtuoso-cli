#![allow(dead_code)]

use crate::error::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub fn parse_psf_ascii(raw_dir: &Path) -> Result<HashMap<String, Vec<f64>>> {
    let mut data = HashMap::new();

    let psf_dir = raw_dir.join("psf");
    let results_dir = raw_dir.join("results");

    let search_dirs = [psf_dir, results_dir];

    for dir in &search_dirs {
        if !dir.exists() {
            continue;
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Ok(values) = parse_psf_file(&path) {
                    if !values.is_empty() {
                        let key = path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        data.insert(key, values);
                    }
                }
            }
        }
    }

    Ok(data)
}

fn parse_psf_file(path: &Path) -> Result<Vec<f64>> {
    let content = fs::read_to_string(path)?;
    let mut values = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("title") {
            continue;
        }

        if let Ok(v) = line.parse::<f64>() {
            values.push(v);
        }
    }

    Ok(values)
}
