use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

#[allow(clippy::too_many_arguments)]
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

    client.execute_skill("simulator('spectre)", None)?;
    let design_result =
        client.execute_skill(&format!("design(\"{lib}\" \"{cell}\" \"{view}\")"), None)?;
    if !design_result.skill_ok() {
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

            // Fetch all 5 oppoint params in one round-trip via SKILL list().
            let params = ["gm", "ids", "gds", "vth", "cgs"];
            let fetch_expr = {
                let gets: Vec<String> = params
                    .iter()
                    .map(|p| format!("value(getData(\"{inst}:{p}\" ?result \"dcOpInfo\"))"))
                    .collect();
                format!("list({})", gets.join(" "))
            };
            let r = client.execute_skill(&fetch_expr, None)?;
            let cleaned = r.output.trim().trim_matches(|c| c == '(' || c == ')');
            let vals: Vec<&str> = cleaned.split_whitespace().collect();

            let mut opvals = OpVals::default();
            let mut ok = vals.len() == params.len();

            if ok {
                for (p, v) in params.iter().zip(vals.iter()) {
                    let v = v.trim_matches('"');
                    if v == "nil" || v.is_empty() {
                        ok = false;
                        break;
                    }
                    match v.parse::<f64>() {
                        Ok(f) => opvals.set(p, f),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
            }

            if ok {
                if let Some(pt) = opvals.build_point(vgs, false) {
                    points.push(pt);
                    total_points += 1;
                }
            }

            vgs += vgs_step;
        }

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

/// Characterize via direct Spectre netlist — no Virtuoso session required.
/// Generates a netlist for each L, runs spectre, parses PSF ASCII oppoint results.
#[allow(clippy::too_many_arguments)]
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
    device_model: &str,
    inst_name: &str,
    vds: f64,
) -> Result<Value> {
    let is_pmos = device_type == "pmos";
    let spectre_cmd = crate::config::Config::from_env()
        .map(|c| c.spectre_cmd)
        .unwrap_or_else(|_| "spectre".into());

    let mut all_data: Vec<Value> = Vec::new();
    let mut total_points = 0;

    let work_dir = std::path::PathBuf::from(format!("/tmp/vcli_char_{device_type}"));
    std::fs::create_dir_all(&work_dir).map_err(VirtuosoError::Io)?;

    for &l in l_values {
        let netlist_path = work_dir.join(format!("char_{l:e}.scs"));
        let raw_dir = work_dir.join(format!("raw_{l:e}"));
        std::fs::create_dir_all(&raw_dir).map_err(VirtuosoError::Io)?;

        let netlist = if is_pmos {
            format!(
                r#"simulator lang=spectre
include "{model_file}" section={model_section}
parameters VDD={vdd} VSD={vds} W=1u L={l:e} vgs_val={vgs_start}
Vvdd (vdd 0) vsource dc=VDD
Vsg  (vdd g) vsource dc=vgs_val
Vsd  (vdd d) vsource dc=VSD
{inst_name} (d g vdd vdd) {device_model} w=W l=L
vgs_sweep dc param=vgs_val start={vgs_start} stop={vgs_stop} step={vgs_step} oppoint=rawfile
save {inst_name}:oppoint
"#
            )
        } else {
            format!(
                r#"simulator lang=spectre
include "{model_file}" section={model_section}
parameters VDS={vds} W=1u L={l:e} vgs_val={vgs_start}
Vvgs (g 0) vsource dc=vgs_val
Vvds (d 0) vsource dc=VDS
{inst_name} (d g 0 0) {device_model} w=W l=L
vgs_sweep dc param=vgs_val start={vgs_start} stop={vgs_stop} step={vgs_step} oppoint=rawfile
save {inst_name}:oppoint
"#
            )
        };

        std::fs::write(&netlist_path, &netlist).map_err(VirtuosoError::Io)?;

        let raw_str = raw_dir.to_str().expect("raw_dir path is UTF-8");
        let output_run = std::process::Command::new(&spectre_cmd)
            .args([
                netlist_path.to_str().expect("netlist path is UTF-8"),
                "+aps",
                "-format",
                "psfascii",
                "-raw",
                raw_str,
            ])
            .output()
            .map_err(|e| VirtuosoError::Execution(format!("spectre failed: {e}")))?;

        if !output_run.status.success() {
            let stderr = String::from_utf8_lossy(&output_run.stderr);
            return Err(VirtuosoError::Execution(format!(
                "spectre error at L={l:e}: {stderr}"
            )));
        }

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
    std::fs::write(
        &output_path,
        serde_json::to_string_pretty(&lookup).map_err(VirtuosoError::Json)?,
    )
    .map_err(VirtuosoError::Io)?;
    Ok(output_path)
}

/// Accumulated oppoint values from one PSF block or one SKILL query set.
#[derive(Default)]
struct OpVals {
    gm: Option<f64>,
    ids: Option<f64>,
    gds: Option<f64>,
    vth: Option<f64>,
    cgs: Option<f64>,
}

impl OpVals {
    fn set(&mut self, key: &str, v: f64) {
        match key {
            "gm" => self.gm = Some(v),
            "ids" => self.ids = Some(v),
            "gds" => self.gds = Some(v),
            "vth" => self.vth = Some(v),
            "cgs" => self.cgs = Some(v),
            _ => {}
        }
    }

    fn build_point(&self, vgs: f64, is_pmos: bool) -> Option<Value> {
        let gm = self.gm?;
        let id = self.ids?.abs();
        let gds = self.gds?.abs();
        let vth = self.vth?;
        let cgs = self.cgs?.abs();

        if id < 1e-15 || gm < 1e-15 {
            return None;
        }

        let gmid = gm / id;
        let gain = gm / gds;
        // vgs represents VSG for PMOS — overdrive is VSG - |Vtp|
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
}

fn parse_psf_oppoint(psf: &str, inst: &str, is_pmos: bool) -> Result<Vec<Value>> {
    let gm_key = format!("\"{}:gm\"", inst);
    let ids_key = format!("\"{}:ids\"", inst);
    let gds_key = format!("\"{}:gds\"", inst);
    let vth_key = format!("\"{}:vth\"", inst);
    let cgs_key = format!("\"{}:cgs\"", inst);

    let mut points: Vec<Value> = Vec::new();
    let mut current_vgs: Option<f64> = None;
    let mut vals = OpVals::default();

    for line in psf.lines() {
        let line = line.trim();

        if line.starts_with("\"vgs_val\"") && !line.contains("sweep") {
            if let Some(vgs) = current_vgs {
                if let Some(pt) = vals.build_point(vgs, is_pmos) {
                    points.push(pt);
                }
                vals = OpVals::default();
            }
            let v: f64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            current_vgs = Some(v);
            continue;
        }

        let parse_val = |key: &str| -> Option<f64> {
            if line.starts_with(key) {
                line.split_whitespace().nth(1).and_then(|s| s.parse().ok())
            } else {
                None
            }
        };

        if let Some(v) = parse_val(&gm_key) {
            vals.gm = Some(v);
        } else if let Some(v) = parse_val(&ids_key) {
            vals.ids = Some(v);
        } else if let Some(v) = parse_val(&gds_key) {
            vals.gds = Some(v);
        } else if let Some(v) = parse_val(&vth_key) {
            vals.vth = Some(v);
        } else if let Some(v) = parse_val(&cgs_key) {
            vals.cgs = Some(v);
        }
    }

    if let Some(vgs) = current_vgs {
        if let Some(pt) = vals.build_point(vgs, is_pmos) {
            points.push(pt);
        }
    }

    Ok(points)
}
