use crate::error::{Result, VirtuosoError};
use crate::output::OutputFormat;
use serde_json::{json, Value};

pub fn size(
    gmid_target: f64,
    l_target: f64,
    gm_req: Option<f64>,
    id_req: Option<f64>,
    pdk: &str,
    device_type: &str,
    format: OutputFormat,
) -> Result<Value> {
    let lookup = load_lookup(pdk, device_type)?;

    // Find closest L
    let l_data = find_closest_l(&lookup, l_target)?;

    // Interpolate at gmid_target
    let point = interpolate_gmid(&l_data, gmid_target)?;

    let id_sim = point["id"].as_f64().unwrap();
    let gain = point["gain"].as_f64().unwrap();
    let ft = point["ft"].as_f64().unwrap_or(0.0);
    let vov = point["vov"].as_f64().unwrap_or(0.0);
    let vth = point["vth"].as_f64().unwrap_or(0.0);

    // W = Id_needed / id_sim_at_W=1µm → result in µm
    // id_sim is in A at W_tb=1µm, so W(µm) = Id(A) / id_sim(A) * W_tb(µm) = Id/id_sim * 1
    let (w, id_design) = if let Some(gm) = gm_req {
        let id = gm / gmid_target;
        let w = id / id_sim; // ratio = W in µm
        (w, id)
    } else if let Some(id) = id_req {
        let w = id / id_sim;
        (w, id)
    } else {
        (1.0, id_sim) // default W=1µm
    };

    let result = json!({
        "status": "success",
        "device": device_type,
        "pdk": pdk,
        "gmid": gmid_target,
        "l_nm": l_target * 1e9,
        "w_um": (w * 100.0).round() / 100.0,
        "id_uA": (id_design * 1e6 * 100.0).round() / 100.0,
        "gain_VV": gain,
        "gain_dB": (20.0 * gain.log10() * 10.0).round() / 10.0,
        "ft_GHz": (ft / 1e9 * 100.0).round() / 100.0,
        "vov_mV": (vov * 1000.0).round(),
        "vth_V": vth,
    });

    if format == OutputFormat::Table {
        let obj = result.as_object().unwrap();
        println!("  Transistor Sizing Result:");
        println!("  ┌───────────────────────────────────┐");
        for key in &[
            "device", "pdk", "gmid", "l_nm", "w_um", "id_uA", "gain_VV", "gain_dB", "ft_GHz",
            "vov_mV", "vth_V",
        ] {
            if let Some(v) = obj.get(*key) {
                let display = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    other => other.to_string(),
                };
                println!("  │  {:<12} = {:<20} │", key, display);
            }
        }
        println!("  └───────────────────────────────────┘");
    }

    Ok(result)
}

pub fn explore(pdk: &str, device_type: &str, format: OutputFormat) -> Result<Value> {
    let lookup = load_lookup(pdk, device_type)?;

    if format == OutputFormat::Table {
        println!("  gm/Id Design Space: {pdk} / {device_type}");
        println!();

        for l_entry in lookup["data"].as_array().unwrap() {
            let l = l_entry["l"].as_f64().unwrap();
            let l_label = if l < 1e-6 {
                format!("{}n", l * 1e9)
            } else {
                format!("{}µ", l * 1e6)
            };
            println!("  L = {l_label}:");
            println!(
                "  {:>6} {:>7} {:>7} {:>10} {:>8} {:>8}",
                "gm/Id", "gain", "gain_dB", "Id(A)", "Vov(mV)", "fT(GHz)"
            );
            println!(
                "  {:>6} {:>7} {:>7} {:>10} {:>8} {:>8}",
                "─────", "─────", "──────", "────────", "──────", "──────"
            );

            for pt in l_entry["points"].as_array().unwrap() {
                let gmid = pt["gmid"].as_f64().unwrap();
                let gain = pt["gain"].as_f64().unwrap();
                let id = pt["id"].as_f64().unwrap();
                let vov = pt["vov"].as_f64().unwrap_or(0.0);
                let ft = pt["ft"].as_f64().unwrap_or(0.0);

                println!(
                    "  {:>6.1} {:>7.1} {:>7.1} {:>10.3e} {:>8.0} {:>8.2}",
                    gmid,
                    gain,
                    20.0 * gain.log10(),
                    id,
                    vov * 1000.0,
                    ft / 1e9
                );
            }
            println!();
        }
    }

    Ok(lookup)
}

fn load_lookup(pdk: &str, device_type: &str) -> Result<Value> {
    let path = format!("process_data/{pdk}/{device_type}_lookup.json");
    let content = std::fs::read_to_string(&path).map_err(|_| {
        VirtuosoError::NotFound(format!(
            "lookup table not found: {path}. Run: virtuoso process char"
        ))
    })?;
    serde_json::from_str(&content)
        .map_err(|e| VirtuosoError::Config(format!("invalid lookup JSON: {e}")))
}

fn find_closest_l(lookup: &Value, l_target: f64) -> Result<Vec<Value>> {
    let data = lookup["data"]
        .as_array()
        .ok_or_else(|| VirtuosoError::Config("lookup has no data array".into()))?;

    let entry = data
        .iter()
        .min_by(|a, b| {
            let la = (a["l"].as_f64().unwrap() - l_target).abs();
            let lb = (b["l"].as_f64().unwrap() - l_target).abs();
            la.partial_cmp(&lb).unwrap()
        })
        .ok_or_else(|| VirtuosoError::NotFound("no L data in lookup".into()))?;

    Ok(entry["points"].as_array().unwrap().clone())
}

fn interpolate_gmid(points: &[Value], gmid_target: f64) -> Result<Value> {
    // Find two closest points for linear interpolation
    let mut sorted: Vec<&Value> = points.iter().collect();
    sorted.sort_by(|a, b| {
        let da = (a["gmid"].as_f64().unwrap() - gmid_target).abs();
        let db = (b["gmid"].as_f64().unwrap() - gmid_target).abs();
        da.partial_cmp(&db).unwrap()
    });

    if sorted.is_empty() {
        return Err(VirtuosoError::NotFound("no points in lookup".into()));
    }

    // Return closest point (simple nearest-neighbor)
    Ok(sorted[0].clone())
}
