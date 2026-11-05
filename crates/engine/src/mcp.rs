//! MCP server, so Claude (Code or Desktop) can run SimLab: list and create
//! projects, import parts from a file path, set up and run tests, wait for
//! them and read the results. Everything it does shows up live in the app.
//!
//! Transports: Streamable HTTP at POST /mcp (JSON responses), and stdio via
//! `simlab-engine mcp`, which forwards to the running engine (starting one
//! in the background if the app is closed).

use crate::api::{self, find_material, project_detail, project_summaries};
use crate::state::App;
use crate::{jobs, models};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::sync::Arc;

const PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "SimLab tests 3D-printed drone parts on this Mac: Strength (will it break, in a chosen filament or resin and print direction), Wind tunnel (forces, drag, flow, optionally spinning: thrust, torque and power of a propeller), Propeller (thrust/torque/power vs RPM from blade element theory, under a second). Wind tunnel runs default to the Accurate engine (OpenFOAM in Docker: ~4 min Draft, ~10 min Normal, ~26 min Fine for a propeller; within ~3% thrust, ~7% torque); engine 'native' is a Quick preview on the GPU, fine for pictures of the flow but rough for numbers (still parts: drag within ~3% on simple shapes at low Reynolds number; spinning parts: thrust can be 30-60% off). Use Accurate for any number the user will rely on, and say which engine a number came from. Workflow: create_project → import_model (a STEP/STL path on this Mac) → get_model (wait until status is ready; read its patches: flat faces and holes with ids, and suggestions such as motor mounts or propeller axis) → list_materials / list_motors → create_study with a config → estimate_study → run_study → wait_for_run → get_run. Positions are model millimetres; forces in newtons; torques in N·m; speeds in m/s. Patches are the clickable surface groups; supports and loads refer to them by id. The user watches the app live; leave clear study names. Say plainly what the numbers mean and how sure they are.";

fn tools() -> Value {
    json!([
        {"name": "list_projects", "description": "All projects with their latest result.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "create_project", "description": "Make a new project (one part or assembly you want to test).", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "notes": {"type": "string"}}, "required": ["name"]}},
        {"name": "get_project", "description": "A project's models, tests (studies) and runs.", "inputSchema": {"type": "object", "properties": {"project_id": {"type": "string"}}, "required": ["project_id"]}},
        {"name": "import_model", "description": "Import a part from a file on this Mac (STL, STEP/STP, IGES, OBJ, 3MF). STL has no units: unit_mm is millimetres per file unit (1 for mm, 25.4 for inches, 1000 for metres). Processing takes seconds; poll get_model until status is 'ready'.", "inputSchema": {"type": "object", "properties": {"project_id": {"type": "string"}, "path": {"type": "string", "description": "absolute path"}, "name": {"type": "string"}, "unit_mm": {"type": "number", "default": 1}}, "required": ["project_id", "path"]}},
        {"name": "get_model", "description": "A model's size, status and surface patches (id, kind plane/cylinder/other, area, centre, normal or axis and radius; cylinders with hole=true are holes), plus suggestions (looks_like, motor_mounts with their hole patches, propeller axis/centre/blades). Use patch ids for supports and loads. Large patch lists are trimmed to the biggest ones unless all=true.", "inputSchema": {"type": "object", "properties": {"model_id": {"type": "string"}, "all": {"type": "boolean"}}, "required": ["model_id"]}},
        {"name": "list_materials", "description": "Printing materials (FDM filaments and resins) with stiffness and strength along (xy) and across (z) layers, density, sources. Strength tests take material_id.", "inputSchema": {"type": "object", "properties": {"process": {"type": "string", "enum": ["fdm", "resin", "other"]}}}},
        {"name": "list_motors", "description": "Motor presets with full-throttle thrust (g), mass, KV, mount pattern.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "create_study", "description": "Set up a test on a model. config is one of:\n- Strength: {\"kind\":\"strength\",\"material_id\":\"fdm-petg\",\"print_up\":[0,0,1],\"support\":{\"type\":\"fixed\",\"patches\":[3]} or {\"type\":\"free_flying\"},\"loads\":[...],\"quality\":{\"type\":\"normal\"}} where loads are {\"type\":\"force\",\"patches\":[..],\"force_n\":[x,y,z]}, {\"type\":\"pressure\",\"patches\":[..],\"pressure_pa\":p}, {\"type\":\"motor\",\"patches\":[..],\"thrust_n\":t,\"torque_nm\":q,\"axis\":[0,0,1],\"motor_mass_g\":m}, {\"type\":\"acceleration\",\"g\":[0,0,-1]}, {\"type\":\"spin\",\"rpm\":r,\"axis\":[..],\"center_mm\":[..]}, {\"type\":\"prop_aero\",\"rpm\":r,\"airspeed_m_s\":0,\"axis\":[..],\"center_mm\":[..]}. Use free_flying for a frame in the air (loads balance by acceleration). When the area to hold or load is not its own patch (smooth organic parts, e.g. a propeller hub), add \"regions\":[{\"id\":1000000,\"label\":\"hub\",\"shape\":{\"type\":\"cylinder\",\"axis\":[0,0,1],\"center_mm\":[0,0,0],\"radius_mm\":4}}] (or {\"type\":\"sphere\",\"center_mm\":..,\"radius_mm\":..} / {\"type\":\"box\",\"min_mm\":..,\"max_mm\":..}) and list the region id (≥ 1000000) in patches like any patch. Quality: draft, normal, fine, or {\"type\":\"custom\",\"size_mm\":s}.\n- Wind tunnel: {\"kind\":\"wind\",\"wind_dir\":[1,0,0],\"speed_m_s\":15,\"spin\":null or {\"rpm\":r,\"axis\":[..],\"center_mm\":[..]},\"quality\":{\"type\":\"normal\"}}. speed 0 with spin = hovering propeller in still air (wind_dir then points the way the wake leaves, e.g. [0,0,-1]). Leave out spin.center_mm to spin about the hub SimLab found in the model (get_model suggestions.prop.center_mm), which is what you want for a propeller: the bounding-box centre is often several mm off the hub. Runs use the Accurate engine (OpenFOAM) unless run_study says engine 'native'.\n- Propeller: {\"kind\":\"prop\",\"axis\":[0,0,1] (thrust direction),\"center_mm\":[..],\"rpm_min\":5000,\"rpm_max\":30000,\"rpm_steps\":12,\"airspeeds_m_s\":[0]}.", "inputSchema": {"type": "object", "properties": {"project_id": {"type": "string"}, "model_id": {"type": "string"}, "name": {"type": "string"}, "config": {"type": "object"}}, "required": ["project_id", "model_id", "config"]}},
        {"name": "update_study", "description": "Change a test's name or config (same kind).", "inputSchema": {"type": "object", "properties": {"study_id": {"type": "string"}, "name": {"type": "string"}, "config": {"type": "object"}}, "required": ["study_id"]}},
        {"name": "estimate_study", "description": "Memory, size and time a run will need, and whether it fits (the memory budget; for OpenFOAM, Docker's memory). Wind tunnel tests also list both engines under 'engines' (reference = Accurate, native = Quick preview).", "inputSchema": {"type": "object", "properties": {"study_id": {"type": "string"}, "engine": {"type": "string", "enum": ["reference", "native"]}}, "required": ["study_id"]}},
        {"name": "run_study", "description": "Start a run of a test. Leave engine out for the default: wind tunnel → 'reference' = Accurate (OpenFOAM in Docker; minutes to half an hour; the one to trust for numbers); strength and propeller → 'native' (SimLab's own solver). engine 'native' on a wind tunnel test is a Quick preview on the GPU: a rough picture, its numbers can be far off for spinning parts (thrust 30-60%). engine 'reference' on a strength test double-checks it in CalculiX (the summary compares it with the latest SimLab run of the same test). Accurate runs need Docker (Colima) running: if it is not, this fails with what to do (Settings → System check); ask the user before starting Colima.", "inputSchema": {"type": "object", "properties": {"study_id": {"type": "string"}, "engine": {"type": "string", "enum": ["reference", "native"]}}, "required": ["study_id"]}},
        {"name": "wait_for_run", "description": "Wait until a run finishes (or timeout_s passes, default 120, max 600; call again for long OpenFOAM runs). Returns its status, progress message and summary.", "inputSchema": {"type": "object", "properties": {"run_id": {"type": "string"}, "timeout_s": {"type": "number"}}, "required": ["run_id"]}},
        {"name": "get_run", "description": "A run's status, summary (headline, verdict, numbers) and result fields with their ranges.", "inputSchema": {"type": "object", "properties": {"run_id": {"type": "string"}}, "required": ["run_id"]}},
        {"name": "stop_run", "description": "Stop a running or queued run.", "inputSchema": {"type": "object", "properties": {"run_id": {"type": "string"}}, "required": ["run_id"]}},
        {"name": "compare_runs", "description": "Every finished run in a project side by side: test, material, key numbers, verdict.", "inputSchema": {"type": "object", "properties": {"project_id": {"type": "string"}}, "required": ["project_id"]}}
    ])
}

fn arg<'a>(a: &'a Value, k: &str) -> anyhow::Result<&'a str> {
    a[k].as_str().ok_or_else(|| anyhow::anyhow!("missing '{k}'"))
}

async fn call(app: &Arc<App>, name: &str, a: &Value) -> anyhow::Result<Value> {
    Ok(match name {
        "list_projects" => json!(project_summaries(app)?),
        "create_project" => {
            let p = app.db().create_project(arg(a, "name")?, a["notes"].as_str().unwrap_or(""))?;
            app.emit(json!({"type": "project", "project_id": p.id}));
            serde_json::to_value(p)?
        }
        "get_project" => {
            let mut d = project_detail(app, arg(a, "project_id")?)?;
            // keep it small: models without their patch lists
            if let Some(ms) = d["models"].as_array_mut() {
                for m in ms {
                    let n = m["patches"].as_array().map(|p| p.len()).unwrap_or(0);
                    if let Some(o) = m.as_object_mut() {
                        o.remove("patches");
                    }
                    m["patch_count"] = json!(n);
                }
            }
            d
        }
        "import_model" => {
            let path = std::path::PathBuf::from(arg(a, "path")?);
            let meta = std::fs::metadata(&path).map_err(|_| anyhow::anyhow!("no file at {}", path.display()))?;
            if !meta.is_file() || meta.len() as usize > models::MAX_UPLOAD {
                anyhow::bail!("not a file, or over 512 MB");
            }
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("part.stl").to_string();
            models::format_of(&fname).ok_or_else(|| anyhow::anyhow!("SimLab reads STL, STEP, IGES, OBJ and 3MF"))?;
            let bytes = std::fs::read(&path)?;
            let pid = arg(a, "project_id")?.to_string();
            let m = models::create(app, &pid, &fname, a["name"].as_str().map(String::from), a["unit_mm"].as_f64().unwrap_or(1.0), &bytes)?;
            app.emit(json!({"type": "model", "model": m.to_json()}));
            m.to_json()
        }
        "get_model" => {
            let mut m = app.db().model(arg(a, "model_id")?)?.to_json();
            if !a["all"].as_bool().unwrap_or(false) {
                if let Some(p) = m["patches"].as_array_mut() {
                    let total = p.len();
                    if total > 80 {
                        p.sort_by(|x, y| y["area_mm2"].as_f64().partial_cmp(&x["area_mm2"].as_f64()).unwrap());
                        p.truncate(80);
                        m["patches_note"] = json!(format!("showing the 80 largest of {total} patches; holes used by motor_mounts are listed in suggestions; pass all=true for every patch"));
                    }
                }
            }
            m
        }
        "list_materials" => {
            let all = api::all_materials(app);
            let filtered: Vec<Value> = all
                .into_iter()
                .filter(|m| a["process"].as_str().map(|p| m["process"] == p).unwrap_or(true))
                .map(|mut m| {
                    if let Some(o) = m.as_object_mut() {
                        o.remove("sources");
                    }
                    m
                })
                .collect();
            json!(filtered)
        }
        "list_motors" => json!(api::all_motors(app)),
        "create_study" => {
            let s = api::create_study_row(app, arg(a, "project_id")?, arg(a, "model_id")?, a["name"].as_str().map(String::from), a["config"].clone())?;
            if let simlab_core::StudyConfig::Strength(c) = api::parse_config(&s.config)? {
                find_material(app, &c.material_id)?;
            }
            api::study_json(app, &s)?
        }
        "update_study" => {
            let mut s = app.db().study(arg(a, "study_id")?)?;
            if let Some(n) = a["name"].as_str() {
                s.name = n.into();
            }
            if a["config"].is_object() {
                let cfg = api::complete_config(app, &s.model_id, a["config"].clone())?;
                if api::kind_of(&cfg) != s.kind {
                    anyhow::bail!("a test cannot change its kind; create a new one");
                }
                s.config = serde_json::to_value(cfg)?;
            }
            app.db().update_study(&s)?;
            app.emit(json!({"type": "project", "project_id": s.project_id}));
            api::study_json(app, &s)?
        }
        "estimate_study" => {
            let id = arg(a, "study_id")?.to_string();
            let engine = a["engine"].as_str().map(String::from);
            let app2 = app.clone();
            tokio::task::spawn_blocking(move || jobs::estimate(&app2, &id, engine.as_deref())).await??
        }
        "run_study" => {
            let id = arg(a, "study_id")?.to_string();
            let engine = a["engine"].as_str().map(String::from);
            let app2 = app.clone();
            let r = tokio::task::spawn_blocking(move || jobs::enqueue(&app2, &id, engine.as_deref())).await??;
            json!({"run_id": r.id, "status": r.status, "engine": r.engine})
        }
        "wait_for_run" => {
            let id = arg(a, "run_id")?;
            let timeout = a["timeout_s"].as_f64().unwrap_or(120.0).clamp(1.0, 600.0);
            let t0 = std::time::Instant::now();
            loop {
                let r = app.db().run(id)?;
                if !matches!(r.status.as_str(), "queued" | "running" | "meshing") || t0.elapsed().as_secs_f64() > timeout {
                    break run_brief(app, &r);
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
        "get_run" => {
            let r = app.db().run(arg(a, "run_id")?)?;
            run_brief(app, &r)
        }
        "stop_run" => {
            jobs::stop(app, arg(a, "run_id")?);
            json!({"stopped": true})
        }
        "compare_runs" => {
            let pid = arg(a, "project_id")?;
            let studies = app.db().studies(pid)?;
            let runs = app.db().runs_for_project(pid)?;
            json!(runs
                .iter()
                .filter(|r| r.status == "done")
                .map(|r| {
                    let s = studies.iter().find(|s| s.id == r.study_id);
                    json!({"run_id": r.id, "test": s.map(|s| s.name.clone()), "kind": s.map(|s| s.kind.clone()), "engine": r.engine,
                        "material": r.material.as_ref().map(|m| m["name"].clone()), "finished_at": r.finished_at, "summary": r.summary})
                })
                .collect::<Vec<_>>())
        }
        _ => anyhow::bail!("unknown tool {name}"),
    })
}

fn run_brief(app: &App, r: &crate::db::RunRow) -> Value {
    let mut v = api::run_json(app, r);
    if let Some(o) = v.as_object_mut() {
        o.remove("metrics");
        o.remove("config");
        if let Some(m) = o.get_mut("material").and_then(|m| m.as_object_mut()) {
            m.remove("sources");
            m.remove("notes");
        }
    }
    v
}

/// Handles one JSON-RPC message; None for notifications.
pub async fn handle(app: &Arc<App>, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or("");
    let id = id?; // notifications get no reply
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let asked = msg["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOLS[0]);
            let v = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[0] };
            Ok(json!({
                "protocolVersion": v,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "simlab", "title": "SimLab by OffGrid", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            match call(app, name, &args).await {
                Ok(v) => Ok(json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap()}], "structuredContent": if v.is_object() { v.clone() } else { json!({"items": v}) }, "isError": false})),
                Err(e) => Ok(json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true})),
            }
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ => Err((-32601, format!("unknown method {method}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, m)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}}),
    })
}

pub async fn http(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Response {
    if let Some(batch) = body.as_array() {
        let mut out = vec![];
        for m in batch {
            if let Some(r) = handle(&app, m).await {
                out.push(r);
            }
        }
        return if out.is_empty() { StatusCode::ACCEPTED.into_response() } else { Json(Value::Array(out)).into_response() };
    }
    match handle(&app, &body).await {
        Some(r) => Json(r).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

// ------------------------------------------------------------------ stdio bridge

/// `simlab-engine mcp`: reads JSON-RPC lines on stdin, forwards each to the
/// engine's /mcp, writes replies on stdout. Starts a background engine if
/// none is running (it exits after 10 idle minutes).
pub fn stdio(port: u16) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    ensure_engine(port)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match post_local(port, "/mcp", &line) {
            Ok((202, _)) => continue,
            Ok((_, body)) => body,
            Err(_) => {
                // the engine went away (app quit): start one and retry once
                ensure_engine(port)?;
                match post_local(port, "/mcp", &line)? {
                    (202, _) => continue,
                    (_, body) => body,
                }
            }
        };
        writeln!(stdout, "{}", reply.trim())?;
        stdout.flush()?;
    }
    Ok(())
}

fn ensure_engine(port: u16) -> anyhow::Result<()> {
    if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .args(["serve", "--port", &port.to_string(), "--idle-exit", "600"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
    }
    anyhow::bail!("the SimLab engine did not start")
}

/// A tiny HTTP/1.1 POST to the local engine (no external client needed).
fn post_local(port: u16, path: &str, body: &str) -> anyhow::Result<(u16, String)> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(700)))?;
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let (head, rest) = text.split_once("\r\n\r\n").ok_or_else(|| anyhow::anyhow!("bad reply from the engine"))?;
    let code: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(500);
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(rest) } else { rest.to_string() };
    Ok((code, body))
}

fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || after.len() < n {
            break;
        }
        out.push_str(&after[..n]);
        rest = after[n..].trim_start_matches("\r\n");
    }
    out
}
