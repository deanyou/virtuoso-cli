#![allow(dead_code)]

use crate::error::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Parse standard PSF ASCII directory (non-sweep).
/// Returns signal_name -> values mapping.
pub fn parse_psf_ascii(raw_dir: &Path) -> Result<HashMap<String, Vec<f64>>> {
    let mut data = HashMap::new();

    let psf_dir = raw_dir.join("psf");
    let results_dir = raw_dir.join("results");

    for dir in [&psf_dir, &results_dir] {
        if dir.exists() {
            if let Ok(parsed) = parse_psf_dir(dir) {
                data.extend(parsed);
            }
        }
    }

    Ok(data)
}

/// Parse a PSF directory and return all signal data.
fn parse_psf_dir(dir: &Path) -> Result<HashMap<String, Vec<f64>>> {
    let mut data = HashMap::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(values) = parse_psf_signal_file(&path) {
                let key = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                data.insert(key, values);
            }
        }
    }

    Ok(data)
}

/// Parse a single PSF signal file (tran.tran, dc_op.dc, etc.).
/// Returns None if parsing fails or file is not a valid signal.
fn parse_psf_signal_file(path: &Path) -> Option<Vec<f64>> {
    let content = fs::read_to_string(path).ok()?;
    let content = content.trim();

    // Skip files that look like headers or metadata
    if content.is_empty() || content.starts_with('#') || content.starts_with("title") {
        return None;
    }

    // Try to detect if this is a PSF ASCII file with sections
    if content.contains("SWEEP") || content.contains("TRACE") || content.contains("VALUE") {
        // This is a PSF ASCII format file with section headers
        return parse_psf_ascii_content(content);
    }

    // Fall back to simple float-per-line parsing
    parse_simple_floats(content)
}

/// Parse PSF ASCII format with section headers (SWEEP, TRACE, VALUE).
/// Returns just the sweep values (time/frequency/etc.) for now.
fn parse_psf_ascii_content(content: &str) -> Option<Vec<f64>> {
    let lines: Vec<&str> = content.lines().collect();

    // Find section boundaries
    let mut sections: HashMap<&str, usize> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "SWEEP" || trimmed == "TRACE" || trimmed == "VALUE" || trimmed == "END" {
            sections.insert(trimmed, i);
        }
    }

    let sweep_start = sections.get("SWEEP")? + 1;
    let sweep_end = sections.get("TRACE").copied().unwrap_or(lines.len());

    // Parse sweep values (time/frequency/etc.) using iterator
    let sweep_values: Vec<f64> = lines[sweep_start..sweep_end]
        .iter()
        .enumerate()
        .filter(|(idx, line)| {
            let trimmed = line.trim();
            // Skip empty lines and section headers
            !trimmed.is_empty()
                && trimmed != "TRACE"
                && trimmed != "VALUE"
                && !(*idx == 0 && trimmed.contains('"'))
        })
        .filter_map(|(_, line)| line.trim().parse::<f64>().ok())
        .collect();

    if sweep_values.is_empty() {
        None
    } else {
        Some(sweep_values)
    }
}

/// Simple float-per-line parsing (fallback).
fn parse_simple_floats(content: &str) -> Option<Vec<f64>> {
    let mut values = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(v) = line.parse::<f64>() {
            values.push(v);
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

/// Parse Spectre parametric sweep output directory.
///
/// Two naming conventions are supported:
/// 1. Subdirectory-per-point (classic Spectre):
///    `<raw>/sw1.sweep1/1/tran.tran.tran`
/// 2. Flat per-point files (Spectre X/LX mode):
///    `<raw>/sw1-000_tran.tran.tran`
///
/// Returns `{point_index: {signal: values}}` where point_index starts at 1.
pub fn parse_sweep_psf_directory(
    output_dir: &Path,
) -> Result<HashMap<usize, HashMap<String, Vec<f64>>>> {
    let mut sweep_data: HashMap<usize, HashMap<String, Vec<f64>>> = HashMap::new();

    // Scan for sweep subdirectories at the root level (not inside psf/)
    // Sweep directories like sw1.sweep1 are at the same level as psf/, not inside it
    if let Ok(entries) = fs::read_dir(output_dir) {
        let mut sweep_dirs: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .map(|n| {
                            let name = n.to_string_lossy();
                            name.starts_with("sw") && name.contains(".sweep")
                        })
                        .unwrap_or(false)
            })
            .collect();

        sweep_dirs.sort();

        for sweep_dir in sweep_dirs {
            if let Ok(point_dirs) = sweep_dir.read_dir() {
                for point_entry in point_dirs.flatten() {
                    let point_path = point_entry.path();
                    if !point_path.is_dir() {
                        continue;
                    }

                    if let Ok(point_idx) =
                        point_entry.file_name().to_string_lossy().parse::<usize>()
                    {
                        if let Ok(data) = parse_psf_dir(&point_path) {
                            if !data.is_empty() {
                                sweep_data.insert(point_idx, data);
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: flat per-point files (Spectre X/LX mode)
    // Files named sw1-000_tran.tran.tran, sw1-001_tran.tran.tran, ...
    if sweep_data.is_empty() {
        if let Ok(entries) = fs::read_dir(output_dir) {
            // Pre-compile regex outside the loop for performance
            let flat_pattern = Regex::new(r"^sw\d+-\d+_").ok();
            let idx_pattern = Regex::new(r"^sw\d+-(\d+)_").ok();

            let mut flat_files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .map(|n| {
                                flat_pattern
                                    .as_ref()
                                    .map(|re| re.is_match(&n.to_string_lossy()))
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false)
                })
                .collect();

            flat_files.sort();

            for psf_file in flat_files {
                if let Some(file_name) = psf_file.file_name().and_then(|n| n.to_str()) {
                    if let Some(ref re) = idx_pattern {
                        if let Some(caps) = re.captures(file_name) {
                            if let Some(idx_str) = caps.get(1) {
                                if let Ok(point_idx) = idx_str.as_str().parse::<usize>() {
                                    // Convert from 0-indexed to 1-indexed
                                    let point_idx = point_idx + 1;

                                    if let Some(values) = parse_psf_signal_file(&psf_file) {
                                        let signal_name = psf_file
                                            .file_stem()
                                            .map(|s| s.to_string_lossy().to_string())
                                            .unwrap_or_else(|| format!("point_{}", point_idx));

                                        let mut data: HashMap<String, Vec<f64>> = HashMap::new();
                                        data.insert(signal_name, values);
                                        sweep_data.insert(point_idx, data);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(sweep_data)
}

/// Find the root directory to scan for PSF files.
fn find_psf_scan_root(output_dir: &Path) -> PathBuf {
    // Prefer psf/ subdirectory, then results/, then the dir itself
    let psf = output_dir.join("psf");
    if psf.exists() {
        return psf;
    }
    let results = output_dir.join("results");
    if results.exists() {
        return results;
    }
    output_dir.to_path_buf()
}

/// Parse sweep output and return a flattened view suitable for plotting.
/// Each sweep point becomes a separate entry.
#[derive(Debug)]
pub struct SweepPoint {
    /// 1-indexed sweep point number
    pub index: usize,
    /// Sweep variable value (e.g., time, frequency, parameter value)
    pub sweep_value: f64,
    /// Signal name to values mapping
    pub signals: HashMap<String, Vec<f64>>,
}

impl SweepPoint {
    /// Extract a single scalar value from this sweep point.
    /// Returns the last value of the specified signal.
    pub fn get_scalar(&self, signal: &str) -> Option<f64> {
        self.signals.get(signal).and_then(|v| v.last().copied())
    }

    /// Extract a specific value at index from the signal.
    pub fn get_value_at(&self, signal: &str, index: usize) -> Option<f64> {
        self.signals.get(signal).and_then(|v| v.get(index).copied())
    }

    /// Get the number of samples in the sweep.
    pub fn num_samples(&self) -> usize {
        self.signals.values().next().map(|v| v.len()).unwrap_or(0)
    }
}

/// Parse sweep output and return structured sweep points.
pub fn parse_sweep_flat(output_dir: &Path) -> Result<Vec<SweepPoint>> {
    let sweep_data = parse_sweep_psf_directory(output_dir)?;

    let mut points: Vec<SweepPoint> = Vec::new();
    let mut indices: Vec<usize> = sweep_data.keys().copied().collect();
    indices.sort();

    for idx in indices {
        let data = &sweep_data[&idx];

        // Extract sweep value (usually the first signal, e.g., "time" or parameter)
        let sweep_value = data
            .values()
            .next()
            .and_then(|v| v.first().copied())
            .unwrap_or(idx as f64);

        points.push(SweepPoint {
            index: idx,
            sweep_value,
            signals: data.clone(),
        });
    }

    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_simple_floats() {
        let content = "0.0\n0.1\n0.2\n0.3\n";
        let values = parse_simple_floats(content).unwrap();
        assert_eq!(values, vec![0.0, 0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_parse_sweep_flat_naming() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create flat sweep files (X/LX mode)
        for i in 0..3 {
            let filename = format!("sw1-{:03}_tran.tran", i);
            let path = raw.join(filename);
            let mut file = fs::File::create(&path).unwrap();
            writeln!(file, "0.0\n{}\n", i as f64).unwrap();
        }

        let result = parse_sweep_psf_directory(&raw).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains_key(&1)); // 0-indexed + 1 = 1-indexed
        assert!(result.contains_key(&2));
        assert!(result.contains_key(&3));
    }

    #[test]
    fn test_find_psf_scan_root() {
        let tmp = TempDir::new().unwrap();
        let psf = tmp.path().join("psf");
        fs::create_dir_all(&psf).unwrap();

        let root = find_psf_scan_root(tmp.path());
        assert_eq!(root, psf);
    }

    #[test]
    fn test_find_psf_scan_root_fallback() {
        let tmp = TempDir::new().unwrap();
        // No psf or results subdirectory
        let root = find_psf_scan_root(tmp.path());
        assert_eq!(root, tmp.path());
    }

    #[test]
    fn test_parse_simple_floats_with_comments() {
        let content = "# header\n0.0\n# comment\n0.1\n0.2\n";
        let values = parse_simple_floats(content).unwrap();
        assert_eq!(values, vec![0.0, 0.1, 0.2]);
    }

    #[test]
    fn test_parse_simple_floats_empty() {
        let content = "# only comments\n   \n";
        let values = parse_simple_floats(content);
        assert!(values.is_none());
    }

    #[test]
    fn test_parse_psf_ascii_content_with_sections() {
        let content = r#"SWEEP
0.0
1.0
2.0
TRACE
VALUE
END"#;
        let values = parse_psf_ascii_content(content);
        assert!(values.is_some());
        let v = values.unwrap();
        assert_eq!(v, vec![0.0, 1.0, 2.0]);
    }

    #[test]
    fn test_parse_psf_ascii_content_with_quoted_header() {
        let content = r#"SWEEP
"time"
0.0
1.0
TRACE
VALUE
END"#;
        let values = parse_psf_ascii_content(content);
        assert!(values.is_some());
        let v = values.unwrap();
        assert_eq!(v, vec![0.0, 1.0]);
    }

    #[test]
    fn test_parse_psf_ascii_content_no_sweep_section() {
        let content = "0.0\n1.0\n2.0";
        let values = parse_psf_ascii_content(content);
        assert!(values.is_none());
    }

    #[test]
    fn test_parse_psf_signal_file_skip_header() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.psf");
        // File without SWEEP/TRACE/VALUE sections should fall back to simple float parsing
        fs::write(&path, "0.0\n1.0\n").unwrap();

        let values = parse_psf_signal_file(&path);
        assert!(values.is_some());
        assert_eq!(values.unwrap(), vec![0.0, 1.0]);
    }

    #[test]
    fn test_parse_psf_signal_file_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.psf");
        fs::write(&path, "").unwrap();

        let values = parse_psf_signal_file(&path);
        assert!(values.is_none());
    }

    #[test]
    fn test_parse_psf_signal_file_comment_only() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("comment.psf");
        fs::write(&path, "# this is a comment\n").unwrap();

        let values = parse_psf_signal_file(&path);
        assert!(values.is_none());
    }

    #[test]
    fn test_parse_sweep_classic_subdir_naming() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create classic sweep subdirectory structure: sw1.sweep1/1/, sw1.sweep1/2/, etc.
        for i in 1..=3 {
            let sweep_dir = raw.join(format!("sw1.sweep{}", i)).join(i.to_string());
            fs::create_dir_all(&sweep_dir).unwrap();
            let psf_file = sweep_dir.join("tran.tran.tran");
            let mut file = fs::File::create(&psf_file).unwrap();
            writeln!(file, "0.0\n1.0\n{}", i as f64).unwrap();
        }

        let result = parse_sweep_psf_directory(&raw).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains_key(&1));
        assert!(result.contains_key(&2));
        assert!(result.contains_key(&3));
    }

    #[test]
    fn test_parse_sweep_flat_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let result = parse_sweep_psf_directory(tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_sweep_flat_with_mixed_files() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create flat sweep files (LX mode) with proper PSF content
        for i in 0..2 {
            let filename = format!("sw1-{:03}_tran.tran", i);
            let path = raw.join(filename);
            let content = format!("SWEEP\n0.0\n1.0\n{}\nTRACE\nVALUE\nEND", i as f64 * 0.5);
            fs::write(&path, content).unwrap();
        }

        let result = parse_sweep_psf_directory(&raw).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_parse_sweep_flat_psf_dir_preference() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create psf/ subdirectory (should be preferred over flat files)
        let psf_dir = raw.join("psf");
        fs::create_dir_all(&psf_dir).unwrap();
        // Note: file_stem() strips the last extension, so "tran.tran" -> "tran"
        let psf_file = psf_dir.join("tran.tran");
        let mut file = fs::File::create(&psf_file).unwrap();
        writeln!(file, "0.0\n1.0\n2.0\n").unwrap();

        let result = parse_psf_ascii(&raw).unwrap();
        assert!(!result.is_empty());
        assert!(result.contains_key("tran")); // file_stem of "tran.tran" is "tran"
    }

    #[test]
    fn test_parse_sweep_flat_structured() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create flat sweep files
        for i in 0..3 {
            let filename = format!("sw1-{:03}_tran.tran", i);
            let path = raw.join(filename);
            fs::write(&path, "0.0\n1.0\n2.0\n").unwrap();
        }

        let points = parse_sweep_flat(&raw).unwrap();
        assert_eq!(points.len(), 3);
        // Verify indices are 1-indexed
        assert_eq!(points[0].index, 1);
        assert_eq!(points[1].index, 2);
        assert_eq!(points[2].index, 3);
    }

    #[test]
    fn test_sweep_point_get_scalar() {
        use super::SweepPoint;
        let mut signals = HashMap::new();
        signals.insert("v(out)".to_string(), vec![0.0, 0.5, 1.0, 1.2]);
        let point = SweepPoint {
            index: 1,
            sweep_value: 0.0,
            signals,
        };

        assert_eq!(point.get_scalar("v(out)"), Some(1.2));
        assert_eq!(point.get_scalar("v(in)"), None);
    }
}
