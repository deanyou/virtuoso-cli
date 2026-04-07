use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn char(
    lib: &str,
    cell: &str,
    view: &str,
    inst: &str,
    device_type: &str,
    l_values: &[f64],
    vgs_start: f64,
    vgs_stop: f64,
    vgs_step: f64,
    output: &str,
    timeout: u64,
) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    // Setup simulator
    client.execute_skill("simulator('spectre)", None)?;
    let design_result = client.execute_skill(
        &format!("design(\"{lib}\" \"{cell}\" \"{view}\")"),
        None,
    )?;
    if !design_result.ok() || design_result.output.trim() == "nil" {
        return Err(VirtuosoError::NotFound(format!(
            "design {lib}/{cell}/{view} not found"
        )));
    }

    let mut all_data: Vec<Value> = Vec::new();
    let mut total_points = 0;

    for &l in l_values {
        client.execute_skill(&format!("desVar(\"L\" {l:e})"), None)?;

        let mut points: Vec<Value> = Vec::new();
        let mut vgs = vgs_start;

        while vgs <= vgs_stop + vgs_step * 0.01 {
            client.execute_skill(&format!("desVar(\"VGS\" {vgs})"), None)?;
            let rdir = format!("/tmp/char_{device_type}_{l:e}_{vgs}");
            client.execute_skill(&format!("resultsDir(\"{rdir}\")"), None)?;
            client.execute_skill("analysis('dc ?saveOppoint t)", None)?;
            client.execute_skill("save('all)", None)?;
            client.execute_skill("run()", Some(timeout))?;

            // Extract oppoint
            let params = ["gm", "ids", "gds", "vth", "cgs"];
            let mut vals: HashMap<String, f64> = HashMap::new();
            let mut ok = true;

            for p in &params {
                let expr = format!("value(getData(\"{inst}:{p}\" ?result \"dcOpInfo\"))");
                let r = client.execute_skill(&expr, None)?;
                let v = r.output.trim().trim_matches('"');
                if v == "nil" || v.is_empty() {
                    ok = false;
                    break;
                }
                if let Ok(f) = v.parse::<f64>() {
                    vals.insert(p.to_string(), f);
                } else {
                    ok = false;
                    break;
                }
            }

            if ok {
                let gm = vals["gm"];
                let id = vals["ids"].abs();
                let gds = vals["gds"].abs();
                let vth = vals["vth"];
                let cgs = vals["cgs"];

                if id > 1e-15 && gm > 1e-15 {
                    let gmid = gm / id;
                    let gain = gm / gds;
                    let vov = vgs - vth;
                    let ft = gm / (2.0 * std::f64::consts::PI * cgs.abs());

                    points.push(json!({
                        "vgs": (vgs * 100.0).round() / 100.0,
                        "gmid": (gmid * 100.0).round() / 100.0,
                        "gain": (gain * 10.0).round() / 10.0,
                        "id": id,
                        "vov": (vov * 1000.0).round() / 1000.0,
                        "ft": ft,
                        "vth": (vth * 10000.0).round() / 10000.0,
                        "gds": gds,
                    }));
                    total_points += 1;
                }
            }

            vgs += vgs_step;
        }

        if !points.is_empty() {
            all_data.push(json!({
                "l": l,
                "points": points,
            }));
        }
    }

    let output_path = write_lookup_json(output, device_type, all_data)?;

    Ok(json!({
        "status": "success",
        "device": device_type,
        "l_values": l_values.len(),
        "total_points": total_points,
        "output": output_path,
    }))
}

/// Characterize via direct Spectre netlist — no Virtuoso session required.
/// Generates a netlist for each L, runs spectre, parses PSF ASCII oppoint results.
pub fn char_netlist(
    device_type: &str,
    l_values: &[f64],
    vgs_start: f64,
    vgs_stop: f64,
    vgs_step: f64,
    output: &str,
    model_file: &str,
    model_section: &str,
    vdd: f64,
) -> Result<Value> {
    let is_pmos = device_type == "pmos";
    let device_name = if is_pmos { "p12" } else { "n12" };
    let inst_name = if is_pmos { "PM0" } else { "NM0" };
    let vsd = 0.6_f64; // saturation bias

    let mut all_data: Vec<Value> = Vec::new();
    let mut total_points = 0;

    let work_dir = std::path::PathBuf::from(format!("/tmp/vcli_char_{device_type}"));
    std::fs::create_dir_all(&work_dir).map_err(VirtuosoError::Io)?;

    for &l in l_values {
        let netlist_path = work_dir.join(format!("char_{l:e}.scs"));
        let raw_dir = work_dir.join(format!("raw_{l:e}"));
        std::fs::create_dir_all(&raw_dir).map_err(VirtuosoError::Io)?;

        // Build netlist
        let netlist = if is_pmos {
            format!(
                r#"simulator lang=spectre
include "{model_file}" section={model_section}
parameters VDD={vdd} VSD={vsd} W=1u L={l:e} vgs_val={vgs_start}
Vvdd (vdd 0) vsource dc=VDD
Vsg  (vdd g) vsource dc=vgs_val
Vsd  (vdd d) vsource dc=VSD
{inst_name} (d g vdd vdd) {device_name} w=W l=L
vgs_sweep dc param=vgs_val start={vgs_start} stop={vgs_stop} step={vgs_step} oppoint=rawfile
save {inst_name}:oppoint
"#
            )
        } else {
            format!(
                r#"simulator lang=spectre
include "{model_file}" section={model_section}
parameters VDS={vsd} W=1u L={l:e} vgs_val={vgs_start}
Vvgs (g 0) vsource dc=vgs_val
Vvds (d 0) vsource dc=VDS
{inst_name} (d g 0 0) {device_name} w=W l=L
vgs_sweep dc param=vgs_val start={vgs_start} stop={vgs_stop} step={vgs_step} oppoint=rawfile
save {inst_name}:oppoint
"#
            )
        };

        std::fs::write(&netlist_path, &netlist).map_err(VirtuosoError::Io)?;

        // Run spectre
        let raw_str = raw_dir.to_string_lossy().to_string();
        let output_run = std::process::Command::new("spectre")
            .args([
                netlist_path.to_str().unwrap(),
                "+aps",
                "-format", "psfascii",
                "-raw", &raw_str,
            ])
            .output()
            .map_err(|e| VirtuosoError::Execution(format!("spectre failed: {e}")))?;

        if !output_run.status.success() {
            let stderr = String::from_utf8_lossy(&output_run.stderr);
            return Err(VirtuosoError::Execution(format!("spectre error at L={l:e}: {stderr}")));
        }

        // Parse PSF ASCII results
        let psf_path = raw_dir.join("vgs_sweep.dc");
        let psf = std::fs::read_to_string(&psf_path).map_err(VirtuosoError::Io)?;
        let points = parse_psf_oppoint(&psf, inst_name, is_pmos)?;

        eprintln!("L={l:e}: {} points", points.len());
        total_points += points.len();

        if !points.is_empty() {
            all_data.push(json!({ "l": l, "points": points }));
        }
    }

    let output_path = write_lookup_json(output, device_type, all_data)?;

    Ok(json!({
        "status": "success",
        "device": device_type,
        "l_values": l_values.len(),
        "total_points": total_points,
        "output": output_path,
    }))
}

fn write_lookup_json(output: &str, device_type: &str, data: Vec<Value>) -> Result<String> {
    let lookup = json!({
        "process": output.split('/').nth_back(1).unwrap_or("unknown"),
        "device": device_type,
        "w_testbench": 1e-6,
        "w_unit": "meters",
        "id_unit": "amperes (absolute current at w_testbench)",
        "gain_unit": "V/V (gm/gds)",
        "ft_unit": "Hz",
        "sizing_formula": "W_design(um) = Id_needed(A) / id(A) * 1",
        "characterized": chrono::Local::now().format("%Y-%m-%d").to_string(),
        "data": data,
    });
    let output_path = format!("{output}/{device_type}_lookup.json");
    std::fs::create_dir_all(output).map_err(VirtuosoError::Io)?;
    std::fs::write(&output_path, serde_json::to_string_pretty(&lookup).map_err(VirtuosoError::Json)?)
        .map_err(VirtuosoError::Io)?;
    Ok(output_path)
}

/// Parse PSF ASCII oppoint file, extract one data point per VGS sweep step.
fn parse_psf_oppoint(psf: &str, inst: &str, is_pmos: bool) -> Result<Vec<Value>> {
    let gm_key   = format!("\"{}:gm\"", inst);
    let ids_key  = format!("\"{}:ids\"", inst);
    let gds_key  = format!("\"{}:gds\"", inst);
    let vth_key  = format!("\"{}:vth\"", inst);
    let cgs_key  = format!("\"{}:cgs\"", inst);

    let mut points: Vec<Value> = Vec::new();

    // Split into per-sweep-point blocks at "vgs_val" value lines
    let mut current_vgs: Option<f64> = None;
    let mut vals: HashMap<String, f64> = HashMap::new();

    for line in psf.lines() {
        let line = line.trim();

        // Sweep parameter value line: "vgs_val" 3.00e-01
        if line.starts_with("\"vgs_val\"") && !line.contains("sweep") {
            // Flush previous block
            if let Some(vgs) = current_vgs {
                if let Some(pt) = build_point(vgs, &vals, is_pmos) {
                    points.push(pt);
                }
                vals.clear();
            }
            let v: f64 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            current_vgs = Some(v);
            continue;
        }

        // Parse value lines: "KEY" value
        let parse_val = |key: &str| -> Option<f64> {
            if line.starts_with(key) {
                line.split_whitespace().nth(1).and_then(|s| s.parse().ok())
            } else {
                None
            }
        };

        if let Some(v) = parse_val(&gm_key)  { vals.insert("gm".into(), v); }
        if let Some(v) = parse_val(&ids_key) { vals.insert("ids".into(), v); }
        if let Some(v) = parse_val(&gds_key) { vals.insert("gds".into(), v); }
        if let Some(v) = parse_val(&vth_key) { vals.insert("vth".into(), v); }
        if let Some(v) = parse_val(&cgs_key) { vals.insert("cgs".into(), v); }
    }

    // Flush last block
    if let Some(vgs) = current_vgs {
        if let Some(pt) = build_point(vgs, &vals, is_pmos) {
            points.push(pt);
        }
    }

    Ok(points)
}

fn build_point(vgs: f64, vals: &HashMap<String, f64>, is_pmos: bool) -> Option<Value> {
    let gm  = *vals.get("gm")?;
    let id  = vals.get("ids")?.abs();
    let gds = vals.get("gds")?.abs();
    let vth = *vals.get("vth")?;
    let cgs = vals.get("cgs")?.abs();

    if id < 1e-15 || gm < 1e-15 { return None; }

    let gmid = gm / id;
    let gain = gm / gds;
    // For PMOS: vov = VSG - |Vtp| = vgs - |vth|; for NMOS: vov = VGS - Vtn = vgs - vth
    let vov = if is_pmos { vgs - vth.abs() } else { vgs - vth };
    let ft = gm / (2.0 * std::f64::consts::PI * cgs);

    Some(json!({
        "vgs":  (vgs * 1000.0).round() / 1000.0,
        "gmid": (gmid * 100.0).round() / 100.0,
        "gain": (gain * 10.0).round() / 10.0,
        "id":   id,
        "vov":  (vov * 1000.0).round() / 1000.0,
        "ft":   ft,
        "vth":  (vth * 10000.0).round() / 10000.0,
        "gds":  gds,
    }))
}
