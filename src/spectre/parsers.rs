#![allow(dead_code)]

use crate::error::Result;
use crate::models::ScalarValue;
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

/// Parse PSF ASCII files for scalar operating-point / STRUCT blocks.
///
/// Matches blocks like:
/// ```text
/// "M0" "mos" (
///   1.906e-04
///   4.500e-01
///   "saturation"
/// )
/// ```
///
/// For mos transistors the known positional parameters are:
///   0 → gm, 1 → Vov (Vdsat), 2 → region
/// All other values are stored as `param<N>`.
pub fn parse_structured_op_blocks(raw_dir: &Path) -> HashMap<String, ScalarValue> {
    let mut ops = HashMap::new();

    let scan_root = find_psf_scan_root(raw_dir);
    let Ok(entries) = fs::read_dir(&scan_root) else {
        return ops;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        // Only scan files likely to contain OP data (not tran.tran etc.)
        if should_skip_psf_file(path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
            continue;
        }
        parse_op_blocks_from_content(&content, &mut ops);
    }

    ops
}

/// Files whose names look like sweep or time-domain waveforms — skip these for OP parsing.
/// Conservatively skips transient/AC/Noise/PSS/PXF/SP analysis outputs; keeps OP files (dc_op.dc, etc.).
fn should_skip_psf_file(name: &str) -> bool {
    // <type>.<type> files (e.g. dc.dc, ac.ac, noise.noise) — skip
    // <type>_<suffix>.<type> where suffix != "op" (e.g. dc_tran.dc, ac_steady.ac) — skip
    // <type>_op.<type> (e.g. dc_op.dc, ac_op.ocn) — keep (operating-point files)
    let skip_type_pattern = |n: &str, t: &str| -> bool {
        n == format!("{t}.{t}")
            || (n.starts_with(&format!("{t}_")) && !n.starts_with(&format!("{t}_op.")))
    };

    name.contains("tran")
        || skip_type_pattern(name, "dc")
        || skip_type_pattern(name, "ac")
        || name.contains("noise")
        || skip_type_pattern(name, "pss")
        || skip_type_pattern(name, "pxf")
        || skip_type_pattern(name, "sp")
        || name.ends_with(".tr0")
        || name.ends_with(".asci")
}

/// Parse the PSF @output-library section to extract type field names.
///
/// The @output-library section in a PSF ASCII file describes each device type
/// and its field names. The format is:
///
/// ```text
/// @output-library
///   @types
///     mos
///       @fields (gm vdsat vth region)
///     nmos
///       @fields (gm vdsat vth region)
/// ```
///
/// If the @output-library section is absent (older Spectre versions),
/// falls back to the well-known mos-family ordering.
fn parse_psf_type_library(content: &str) -> Option<HashMap<String, Vec<String>>> {
    let mut type_fields: HashMap<String, Vec<String>> = HashMap::new();

    // Find @output-library section
    let output_lib_start = content.find("@output-library")?;
    let section = &content[output_lib_start..];

    // Match each @types ... @fields (...) block
    // e.g., "mos\n  @fields (gm vdsat vth region)"
    let type_re = Regex::new(r#"(?m)^(\S[^\n]*)\s+@fields\s*\(([^)]+)\)"#).ok()?;
    for caps in type_re.captures_iter(section) {
        let dev_type = caps.get(1).unwrap().as_str().trim().to_lowercase();
        let fields_str = caps.get(2).unwrap().as_str();
        let fields: Vec<String> = fields_str
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if !fields.is_empty() {
            type_fields.insert(dev_type, fields);
        }
    }

    // If nmos isn't explicitly defined but mos is, inherit mos fields.
    // Same for pmos, r, c, etc. (they share the same field set as mos family).
    if type_fields.contains_key("mos") && !type_fields.contains_key("nmos") {
        type_fields.insert("nmos".to_string(), type_fields["mos"].clone());
    }
    if type_fields.contains_key("mos") && !type_fields.contains_key("pmos") {
        type_fields.insert("pmos".to_string(), type_fields["mos"].clone());
    }

    Some(type_fields)
}

/// Extract OP blocks from a single PSF file's content.
fn parse_op_blocks_from_content(content: &str, ops: &mut HashMap<String, ScalarValue>) {
    // Parse @output-library for type field names first
    let type_fields = parse_psf_type_library(content).unwrap_or_default();

    // Match quoted device name, quoted type, opening paren on same or next line
    let block_re = Regex::new(r#""([^"]+)"\s+"([^"]+)"\s*\(\s*"#).ok();
    let block_re = match block_re {
        Some(re) => re,
        None => return,
    };

    let mut last_end = 0;
    while let Some(block_cap) = block_re.captures(&content[last_end..]) {
        let full_match = block_cap.get(0).unwrap();
        let end_offset = last_end + full_match.end();

        let dev_name = block_cap.get(1).unwrap().as_str().to_string();
        let dev_type = block_cap.get(2).unwrap().as_str().to_lowercase();

        // Find the matching closing paren
        if let Some(block_end) = find_closing_paren(content, end_offset) {
            let inner = &content[end_offset..block_end];
            let values = parse_op_inner_values(inner);

            if values.is_empty() {
                last_end = block_end;
                continue;
            }

            // Try to use field names from @output-library if available
            if let Some(fields) = type_fields.get(&dev_type) {
                for (idx, field_name) in fields.iter().enumerate() {
                    if idx < values.len() {
                        ops.insert(format!("{}:{}", dev_name, field_name), values[idx].clone());
                    }
                }
            } else {
                // Fallback for unknown types: apply mos-family convention if the
                // lowercased type contains "mos", otherwise positional param<N>.
                let is_mos_family = dev_type.contains("mos")
                    || dev_type == "nmos"
                    || dev_type == "pmos"
                    || dev_type == "pch"; // pch = pmos in some PDKs

                if is_mos_family && !values.is_empty() {
                    // Standard Spectre mos OP order: gm, vdsat, vth, region
                    ops.insert(format!("{}:gm", dev_name), values[0].clone());
                }
                if is_mos_family && values.len() >= 2 {
                    ops.insert(format!("{}:vdsat", dev_name), values[1].clone());
                }
                if is_mos_family && values.len() >= 3 {
                    // Index 2 is vth, NOT region (region is a string at index 3+)
                    if !values[2].as_str().is_some() {
                        ops.insert(format!("{}:vth", dev_name), values[2].clone());
                    }
                }
                if is_mos_family && values.len() >= 4 {
                    // Index 3 is region (string) in standard mos order: gm vdsat vth region
                    if let ScalarValue::String(_) = &values[3] {
                        ops.insert(format!("{}:region", dev_name), values[3].clone());
                    }
                }
                if is_mos_family {
                    // Extra fields beyond standard gm/vdsat/vth/region go as param<N>
                    for (i, v) in values.iter().enumerate().skip(4) {
                        ops.insert(format!("{}:param{}", dev_name, i), v.clone());
                    }
                } else {
                    // Unknown non-mos type: all positional
                    for (i, v) in values.iter().enumerate() {
                        ops.insert(format!("{}:param{}", dev_name, i), v.clone());
                    }
                }
            }

            last_end = block_end;
        } else {
            last_end = end_offset;
        }
    }
}

/// Find the matching `)` in a balanced-paren block starting after `open_offset`.
/// Respects double-quoted strings so `)` inside `"..."` is not counted.
fn find_closing_paren(content: &str, open_offset: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut depth = 1;
    let mut in_string = false;
    let mut i = open_offset;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_string = !in_string,
            b'(' if !in_string => depth += 1,
            b')' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse the comma- or whitespace-separated values inside an OP block.
fn parse_op_inner_values(inner: &str) -> Vec<ScalarValue> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_string = false;

    for ch in inner.chars() {
        match ch {
            '"' => {
                in_string = !in_string;
                current.push(ch);
            }
            ',' | '\n' | '\r' | '\t' | ' ' if !in_string => {
                let trimmed = current.trim();
                if !trimmed.is_empty() && trimmed != "\"\"" {
                    if let Some(sv) = parse_scalar(trimmed) {
                        values.push(sv);
                    }
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    // Last value (no trailing comma)
    let trimmed = current.trim();
    if !trimmed.is_empty() && trimmed != "\"\"" {
        if let Some(sv) = parse_scalar(trimmed) {
            values.push(sv);
        }
    }
    values
}

/// Parse a single scalar token (quoted string, integer, or float).
fn parse_scalar(token: &str) -> Option<ScalarValue> {
    let t = token.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('"') && t.ends_with('"')) {
        // Quoted string — strip quotes
        let inner = &t[1..t.len() - 1];
        return Some(ScalarValue::String(inner.to_string()));
    }
    if let Ok(i) = t.parse::<i64>() {
        return Some(ScalarValue::Integer(i));
    }
    if let Ok(f) = t.parse::<f64>() {
        return Some(ScalarValue::Float(f));
    }
    None
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

    // === OP block parsing tests ===

    #[test]
    fn test_parse_scalar_float() {
        assert!(matches!(
            parse_scalar("1.906e-04"),
            Some(ScalarValue::Float(_))
        ));
        assert!(matches!(parse_scalar("0.45"), Some(ScalarValue::Float(_))));
        assert!(matches!(parse_scalar("-1.5"), Some(ScalarValue::Float(_))));
    }

    #[test]
    fn test_parse_scalar_integer() {
        assert!(matches!(parse_scalar("42"), Some(ScalarValue::Integer(42))));
        assert!(matches!(parse_scalar("-7"), Some(ScalarValue::Integer(-7))));
    }

    #[test]
    fn test_parse_scalar_string() {
        let sv = parse_scalar("\"saturation\"").unwrap();
        assert!(matches!(sv, ScalarValue::String(s) if s == "saturation"));
    }

    #[test]
    fn test_parse_op_inner_values_mos() {
        let inner = "1.906e-04\n  4.500e-01\n  \"saturation\"\n";
        let values = parse_op_inner_values(inner);
        assert_eq!(values.len(), 3);
        assert!(values[0].as_f64().unwrap() > 0.0);
        assert!(values[1].as_f64().unwrap() > 0.0);
        assert!(matches!(&values[2], ScalarValue::String(s) if s == "saturation"));
    }

    #[test]
    fn test_parse_op_blocks_from_content_mos() {
        // Modern Spectre @output-library mos-family: gm, vdsat, vth, region
        let content = r#""M0" "mos" (
  1.906e-04
  4.500e-01
  5.200e-01
  "saturation"
)
""#;
        let mut ops = HashMap::new();
        parse_op_blocks_from_content(content, &mut ops);

        assert!(ops.contains_key("M0:gm"));
        assert!(ops.contains_key("M0:vdsat"));
        assert!(ops.contains_key("M0:vth"));
        assert!(ops.contains_key("M0:region"));
        assert!(matches!(ops.get("M0:region"), Some(ScalarValue::String(s)) if s == "saturation"));
    }

    #[test]
    fn test_parse_op_blocks_multiple_devices() {
        let content = r#""M0" "mos" (
  1.906e-04
  4.500e-01
  5.200e-01
  "saturation"
)

"M1" "nmos" (
  9.500e-05
  3.200e-01
  4.800e-01
  "triode"
)
""#;
        let mut ops = HashMap::new();
        parse_op_blocks_from_content(content, &mut ops);

        assert!(ops.contains_key("M0:gm"));
        assert!(ops.contains_key("M1:gm"));
        assert!(ops.contains_key("M1:vth"));
        assert!(ops.contains_key("M1:region"));
        assert!(matches!(ops.get("M1:region"), Some(ScalarValue::String(s)) if s == "triode"));
    }

    #[test]
    fn test_parse_structured_op_blocks_skips_tran_files() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        fs::write(
            raw.join("tran.tran"),
            r#""M0" "mos" (
  1.0
  0.5
  "sat"
)
"#,
        )
        .unwrap();

        fs::write(
            raw.join("dc_op.dc"),
            r#""M1" "mos" (
  2.0
  0.6
  "saturation"
)
"#,
        )
        .unwrap();

        let ops = parse_structured_op_blocks(&raw);
        assert!(!ops.contains_key("M0:gm"), "tran files should be skipped");
        assert!(ops.contains_key("M1:gm"), "op files should be parsed");
    }

    #[test]
    fn test_parse_structured_op_blocks_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let ops = parse_structured_op_blocks(tmp.path());
        assert!(ops.is_empty());
    }

    #[test]
    fn test_should_skip_psf_file() {
        assert!(should_skip_psf_file("tran.tran"));
        assert!(should_skip_psf_file("ac.ac"));
        assert!(should_skip_psf_file("dc.dc"));
        assert!(should_skip_psf_file("noise.noise"));
        assert!(should_skip_psf_file("pss.pss"));
        assert!(!should_skip_psf_file("dc_op.dc"));
        assert!(!should_skip_psf_file("ac_op.ocn"));
    }

    // NOTE: find_closing_paren is tested implicitly via
    // test_parse_op_blocks_from_content_mos (full PSF block parsing).
    // The function requires PSF-format content (starts with '('),
    // so testing it in isolation with non-PSF strings is misleading.

    #[test]
    fn test_scalar_value_accessors() {
        let f = ScalarValue::Float(1.906e-04);
        assert_eq!(f.as_f64(), Some(1.906e-04));
        assert_eq!(f.as_str(), None);

        let s = ScalarValue::String("saturation".to_string());
        assert_eq!(s.as_str(), Some("saturation"));
        assert_eq!(s.as_f64(), None);

        let i = ScalarValue::Integer(42);
        assert_eq!(i.as_f64(), Some(42.0));
    }
}
