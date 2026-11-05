//! HTTP routes (docs/API.md).

use crate::db::{RunRow, StudyRow, new_id, now};
use crate::state::{App, Settings};
use crate::{jobs, models, system};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use simlab_core::{Material, Motor, StudyConfig};
use std::sync::Arc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

pub type S = State<Arc<App>>;

pub struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let msg = format!("{e:#}");
        let code = if msg.starts_with("no ") { StatusCode::NOT_FOUND } else { StatusCode::BAD_REQUEST };
        ApiError(code, msg)
    }
}
type R<T> = Result<T, ApiError>;

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/system/fix", post(system_fix))
        .route("/system/update", post(system_update))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/materials", get(materials).post(add_material))
        .route("/materials/{id}", axum::routing::delete(del_material))
        .route("/motors", get(motors).post(add_motor))
        .route("/motors/{id}", axum::routing::delete(del_motor))
        .route("/projects", get(projects).post(create_project))
        .route("/projects/{id}", get(project).patch(patch_project).delete(delete_project))
        .route("/projects/{id}/models", post(upload_model))
        .route("/projects/{id}/studies", post(create_study))
        .route("/models/{id}", get(model).patch(patch_model).delete(delete_model))
        .route("/models/{id}/surface/{buf}", get(model_surface))
        .route("/models/{id}/file", get(model_file))
        .route("/studies/{id}", get(study).patch(patch_study).delete(delete_study))
        .route("/studies/{id}/estimate", post(estimate))
        .route("/studies/{id}/runs", post(start_run))
        .route("/runs/{id}", get(run).delete(delete_run))
        .route("/runs/{id}/stop", post(stop_run))
        .route("/runs/{id}/mesh/{buf}", get(run_mesh))
        .route("/runs/{id}/field/{name}", get(run_field))
        .route("/runs/{id}/slice/{name}", get(run_slice))
        .route("/runs/{id}/slice/{name}/{field}", get(run_slice_field))
        .route("/runs/{id}/log", get(run_log))
        .route("/events", get(events))
        .layer(DefaultBodyLimit::max(models::MAX_UPLOAD + 1024 * 1024));
    let mut r = Router::new().nest("/api", api).route("/mcp", post(crate::mcp::http));
    if let Some(web) = app.resources.web.clone() {
        let index = web.join("index.html");
        r = r.fallback_service(tower_http::services::ServeDir::new(web).fallback(tower_http::services::ServeFile::new(index)));
    }
    r.layer(axum::middleware::from_fn_with_state(app.clone(), guard)).with_state(app)
}

/// Only this Mac may talk to the engine, and only pages served by it (or the
/// Tauri window / dev server): blocks DNS-rebinding and drive-by requests
/// from other websites open in the browser.
async fn guard(State(app): S, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let ok_host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            let host = h.rsplit_once(':').map(|(a, _)| a).unwrap_or(h);
            matches!(host, "127.0.0.1" | "localhost" | "[::1]")
        })
        .unwrap_or(false);
    let ok_origin = match req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        None => true,
        Some(o) => {
            o.starts_with("http://127.0.0.1:") || o.starts_with("http://localhost:") || o.starts_with("tauri://") || o == "http://tauri.localhost"
        }
    };
    if !ok_host || !ok_origin {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "SimLab only answers this Mac's own pages"}))).into_response();
    }
    app.touch();
    next.run(req).await
}

// ------------------------------------------------------------------ system

async fn health(State(app): S) -> R<Json<Value>> {
    let checks = app.checks.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        // "source": built here with make install (Update pulls and rebuilds);
        // "download": from a GitHub release (Update only checks for a newer one)
        "install": if system::source_dir(&app).is_some() { "source" } else { "download" },
        "data_dir": app.data,
        "machine": {
            "cpu": system::cpu_name(), "cores": system::cores(),
            "performance_cores": system::performance_cores(), "ram_bytes": system::ram_bytes(),
            "gpu": checks.iter().find(|c| c.id == "gpu").map(|c| c.detail.clone()),
        },
        "checks": checks,
    })))
}

#[derive(Deserialize)]
struct FixBody {
    id: String,
}
async fn system_fix(State(app): S, Json(b): Json<FixBody>) -> R<Json<Value>> {
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = system::fix(&app2, &b.id) {
            app2.emit(json!({"type": "check", "check": {"id": b.id, "label": "", "status": "error", "detail": format!("{e:#}")}}));
        }
    });
    Ok(Json(json!({"started": true})))
}

async fn system_update(State(app): S) -> R<Json<Value>> {
    let app2 = app.clone();
    let r = tokio::task::spawn_blocking(move || system::update(&app2)).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!({"message": r.message, "url": r.url, "latest": r.latest})))
}

async fn get_settings(State(app): S) -> Json<Settings> {
    Json(app.settings())
}
async fn put_settings(State(app): S, Json(s): Json<Settings>) -> R<Json<Settings>> {
    let ram = system::ram_bytes() as f64 / system::GIB;
    let s = Settings { memory_budget_gb: s.memory_budget_gb.clamp(1.0, (ram * 0.85).floor()), cpu_threads: s.cpu_threads.clamp(1, system::cores()), ..s };
    app.db().set_setting("settings", &serde_json::to_value(&s).unwrap())?;
    Ok(Json(s))
}

// ------------------------------------------------------------------ library

pub fn all_materials(app: &App) -> Vec<Value> {
    let mut v: Vec<Value> = simlab_core::builtin_materials().into_iter().map(|m| serde_json::to_value(m).unwrap()).collect();
    for mut c in app.db().custom("materials").unwrap_or_default() {
        c["custom"] = json!(true);
        v.push(c);
    }
    v
}
pub fn find_material(app: &App, id: &str) -> anyhow::Result<Material> {
    let v = all_materials(app).into_iter().find(|m| m["id"] == id).ok_or_else(|| anyhow::anyhow!("no material {id}"))?;
    Ok(serde_json::from_value(v)?)
}
async fn materials(State(app): S) -> Json<Vec<Value>> {
    Json(all_materials(&app))
}
async fn add_material(State(app): S, Json(mut m): Json<Material>) -> R<Json<Value>> {
    if m.e_xy_mpa <= 0.0 || m.e_z_mpa <= 0.0 || m.uts_xy_mpa <= 0.0 || m.uts_z_mpa <= 0.0 || m.density <= 0.0 || !(0.0..0.5).contains(&m.nu) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "stiffness, strength and density must be above zero, Poisson's ratio between 0 and 0.5".into()));
    }
    if m.id.is_empty() || simlab_core::builtin_materials().iter().any(|b| b.id == m.id) {
        m.id = format!("custom-{}", new_id());
    }
    let v = serde_json::to_value(&m).unwrap();
    app.db().put_custom("materials", &m.id, &v)?;
    Ok(Json(v))
}
async fn del_material(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    app.db().delete_custom("materials", &id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn all_motors(app: &App) -> Vec<Value> {
    let mut v: Vec<Value> = serde_json::from_str(include_str!("motors.json")).unwrap();
    for mut c in app.db().custom("motors").unwrap_or_default() {
        c["custom"] = json!(true);
        v.push(c);
    }
    v
}
async fn motors(State(app): S) -> Json<Vec<Value>> {
    Json(all_motors(&app))
}
async fn add_motor(State(app): S, Json(mut m): Json<Motor>) -> R<Json<Value>> {
    if m.id.is_empty() || !m.id.starts_with("custom-") {
        m.id = format!("custom-{}", new_id());
    }
    let v = serde_json::to_value(&m).unwrap();
    app.db().put_custom("motors", &m.id, &v)?;
    Ok(Json(v))
}
async fn del_motor(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    app.db().delete_custom("motors", &id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ projects

pub fn project_summaries(app: &App) -> anyhow::Result<Vec<Value>> {
    let db = app.db();
    let mut out = vec![];
    for p in db.projects()? {
        let models = db.models(&p.id)?;
        let studies = db.studies(&p.id)?;
        let runs = db.runs_for_project(&p.id)?;
        let last = runs.iter().rev().find(|r| r.status == "done").or(runs.last());
        out.push(json!({
            "id": p.id, "name": p.name, "notes": p.notes, "created_at": p.created_at, "updated_at": p.updated_at,
            "models": models.len(), "studies": studies.len(), "runs": runs.len(),
            "last_run": last.map(|r| {
                let mut b = r.brief();
                b["kind"] = json!(studies.iter().find(|s| s.id == r.study_id).map(|s| s.kind.clone()));
                b
            }),
            "thumbnail_model_id": models.iter().find(|m| m.status == "ready").map(|m| m.id.clone()),
        }));
    }
    Ok(out)
}
async fn projects(State(app): S) -> R<Json<Vec<Value>>> {
    Ok(Json(project_summaries(&app)?))
}

#[derive(Deserialize)]
struct NewProject {
    name: String,
    #[serde(default)]
    notes: String,
}
async fn create_project(State(app): S, Json(b): Json<NewProject>) -> R<Json<Value>> {
    let name = if b.name.trim().is_empty() { "Untitled part".to_string() } else { b.name.trim().to_string() };
    let p = app.db().create_project(&name, &b.notes)?;
    app.emit(json!({"type": "project", "project_id": p.id}));
    Ok(Json(serde_json::to_value(p).unwrap()))
}

pub fn study_json(app: &App, s: &StudyRow) -> anyhow::Result<Value> {
    let runs = app.db().runs(&s.id)?;
    let mut v = serde_json::to_value(s)?;
    v["runs"] = json!(runs.iter().map(|r| r.brief()).collect::<Vec<_>>());
    Ok(v)
}

pub fn project_detail(app: &App, id: &str) -> anyhow::Result<Value> {
    let (p, models, studies) = {
        let db = app.db();
        (db.project(id)?, db.models(id)?, db.studies(id)?)
    };
    Ok(json!({
        "project": p,
        "models": models.iter().map(|m| m.to_json()).collect::<Vec<_>>(),
        "studies": studies.iter().map(|s| study_json(app, s)).collect::<anyhow::Result<Vec<_>>>()?,
    }))
}
async fn project(State(app): S, Path(id): Path<String>) -> R<Json<Value>> {
    Ok(Json(project_detail(&app, &id)?))
}
#[derive(Deserialize)]
struct PatchProject {
    name: Option<String>,
    notes: Option<String>,
}
async fn patch_project(State(app): S, Path(id): Path<String>, Json(b): Json<PatchProject>) -> R<Json<Value>> {
    let p = app.db().update_project(&id, b.name.as_deref(), b.notes.as_deref())?;
    app.emit(json!({"type": "project", "project_id": id}));
    Ok(Json(serde_json::to_value(p).unwrap()))
}
async fn delete_project(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    let runs = app.db().runs_for_project(&id)?;
    for r in runs {
        jobs::stop(&app, &r.id);
        let _ = std::fs::remove_dir_all(app.run_dir(&r.id));
    }
    app.db().delete_project(&id)?;
    let _ = std::fs::remove_dir_all(app.data.join("projects").join(&id));
    app.emit(json!({"type": "project", "project_id": id}));
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ models

async fn upload_model(State(app): S, Path(pid): Path<String>, mut mp: Multipart) -> R<Json<Value>> {
    let (mut file, mut name, mut unit_mm) = (None, None, 1.0f64);
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))? {
        match field.name() {
            Some("file") => {
                let fname = field.file_name().unwrap_or("part.stl").to_string();
                let bytes = field.bytes().await.map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
                file = Some((fname, bytes));
            }
            Some("name") => name = Some(field.text().await.unwrap_or_default()),
            Some("unit_mm") => unit_mm = field.text().await.ok().and_then(|t| t.parse().ok()).unwrap_or(1.0),
            _ => {}
        }
    }
    let (fname, bytes) = file.ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "no file in the upload".into()))?;
    let m = models::create(&app, &pid, &fname, name, unit_mm, &bytes)?;
    app.emit(json!({"type": "model", "model": m.to_json()}));
    app.emit(json!({"type": "project", "project_id": pid}));
    Ok(Json(m.to_json()))
}

async fn model(State(app): S, Path(id): Path<String>) -> R<Json<Value>> {
    Ok(Json(app.db().model(&id)?.to_json()))
}

#[derive(Deserialize)]
struct PatchModel {
    name: Option<String>,
    unit_mm: Option<f64>,
    patch_labels: Option<serde_json::Map<String, Value>>,
}
async fn patch_model(State(app): S, Path(id): Path<String>, Json(b): Json<PatchModel>) -> R<Json<Value>> {
    let mut m = app.db().model(&id)?;
    if let Some(n) = b.name {
        m.name = n;
    }
    if let Some(l) = b.patch_labels {
        let mut labels = m.patch_labels.as_object().cloned().unwrap_or_default();
        labels.extend(l);
        m.patch_labels = Value::Object(labels);
    }
    let reprocess = matches!(b.unit_mm, Some(u) if u > 0.0 && (u - m.unit_mm).abs() > 1e-12);
    if let Some(u) = b.unit_mm.filter(|_| reprocess) {
        m.unit_mm = u;
        m.status = "processing".into();
        m.message = "Changing units".into();
    }
    app.db().update_model(&m)?;
    if reprocess {
        let app2 = app.clone();
        let mid = id.clone();
        tokio::task::spawn_blocking(move || {
            let res = models::process(&app2, &mid);
            let found = app2.db().model(&mid);
            if let Ok(mut m) = found {
                match res {
                    Ok(info) => {
                        m.status = "ready".into();
                        m.message.clear();
                        m.info = info;
                    }
                    Err(e) => {
                        m.status = "failed".into();
                        m.message = format!("{e:#}");
                    }
                }
                let _ = app2.db().update_model(&m);
                app2.emit(json!({"type": "model", "model": m.to_json()}));
            }
        });
    }
    app.emit(json!({"type": "model", "model": m.to_json()}));
    Ok(Json(m.to_json()))
}
async fn delete_model(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    let m = app.db().model(&id)?;
    app.db().delete_model(&id)?;
    let _ = std::fs::remove_dir_all(app.model_dir(&m.project_id, &id));
    app.emit(json!({"type": "project", "project_id": m.project_id}));
    Ok(StatusCode::NO_CONTENT)
}

fn binary(path: std::path::PathBuf) -> R<Response> {
    let bytes = std::fs::read(&path).map_err(|_| ApiError(StatusCode::NOT_FOUND, "no such buffer".into()))?;
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((h, Body::from(bytes)).into_response())
}
fn safe(name: &str) -> R<&str> {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') && !name.contains("..") {
        Ok(name)
    } else {
        Err(ApiError(StatusCode::BAD_REQUEST, "bad name".into()))
    }
}

async fn model_surface(State(app): S, Path((id, buf)): Path<(String, String)>) -> R<Response> {
    let m = app.db().model(&id)?;
    let buf = safe(&buf)?;
    if !["positions.f32", "normals.f32", "indices.u32", "patch.u32"].contains(&buf) {
        return Err(ApiError(StatusCode::NOT_FOUND, "no such buffer".into()));
    }
    binary(app.model_dir(&m.project_id, &id).join(format!("surface.{buf}")))
}
async fn model_file(State(app): S, Path(id): Path<String>) -> R<Response> {
    let m = app.db().model(&id)?;
    let dir = app.model_dir(&m.project_id, &id);
    let ext = if m.format == "iges" { "igs" } else { m.format.as_str() };
    let mut r = binary(dir.join(format!("source.{ext}")))?;
    let disp = format!("attachment; filename=\"{}\"", m.file_name.replace('"', ""));
    if let Ok(v) = HeaderValue::from_str(&disp) {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(r)
}

// ------------------------------------------------------------------ studies

#[derive(Deserialize)]
struct NewStudy {
    name: Option<String>,
    model_id: String,
    config: Value,
}
pub fn kind_of(c: &StudyConfig) -> &'static str {
    match c {
        StudyConfig::Strength(_) => "strength",
        StudyConfig::Wind(_) => "wind",
        StudyConfig::Prop(_) => "prop",
    }
}
pub fn parse_config(v: &Value) -> anyhow::Result<StudyConfig> {
    serde_json::from_value(v.clone()).map_err(|e| anyhow::anyhow!("the test settings are not valid: {e}"))
}
pub fn create_study_row(app: &App, pid: &str, model_id: &str, name: Option<String>, config: Value) -> anyhow::Result<StudyRow> {
    let cfg = parse_config(&config)?;
    let m = app.db().model(model_id)?;
    if m.project_id != pid {
        anyhow::bail!("that model belongs to another project");
    }
    let kind = kind_of(&cfg);
    let default_name = match kind {
        "strength" => "Strength test",
        "wind" => "Wind tunnel",
        _ => "Propeller test",
    };
    let s = StudyRow {
        id: new_id(),
        project_id: pid.into(),
        model_id: model_id.into(),
        name: name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| default_name.into()),
        kind: kind.into(),
        config: serde_json::to_value(&cfg)?,
        created_at: now(),
        updated_at: now(),
    };
    app.db().insert_study(&s)?;
    app.db().touch_project(pid)?;
    app.emit(json!({"type": "project", "project_id": pid}));
    Ok(s)
}
async fn create_study(State(app): S, Path(pid): Path<String>, Json(b): Json<NewStudy>) -> R<Json<Value>> {
    let s = create_study_row(&app, &pid, &b.model_id, b.name, b.config)?;
    Ok(Json(study_json(&app, &s)?))
}
async fn study(State(app): S, Path(id): Path<String>) -> R<Json<Value>> {
    let s = app.db().study(&id)?;
    Ok(Json(study_json(&app, &s)?))
}
#[derive(Deserialize)]
struct PatchStudy {
    name: Option<String>,
    config: Option<Value>,
}
async fn patch_study(State(app): S, Path(id): Path<String>, Json(b): Json<PatchStudy>) -> R<Json<Value>> {
    let mut s = app.db().study(&id)?;
    if let Some(n) = b.name {
        s.name = n;
    }
    if let Some(c) = b.config {
        let cfg = parse_config(&c)?;
        if kind_of(&cfg) != s.kind {
            return Err(ApiError(StatusCode::BAD_REQUEST, "a test cannot change its kind; make a new one".into()));
        }
        s.config = serde_json::to_value(cfg).unwrap();
    }
    app.db().update_study(&s)?;
    app.emit(json!({"type": "project", "project_id": s.project_id}));
    Ok(Json(study_json(&app, &s)?))
}
async fn delete_study(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    let s = app.db().study(&id)?;
    for r in app.db().runs(&id)? {
        jobs::stop(&app, &r.id);
        let _ = std::fs::remove_dir_all(app.run_dir(&r.id));
    }
    app.db().delete_study(&id)?;
    app.emit(json!({"type": "project", "project_id": s.project_id}));
    Ok(StatusCode::NO_CONTENT)
}
async fn estimate(State(app): S, Path(id): Path<String>) -> R<Json<Value>> {
    let app2 = app.clone();
    let v = tokio::task::spawn_blocking(move || jobs::estimate(&app2, &id)).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
struct StartRun {
    engine: Option<String>,
}
async fn start_run(State(app): S, Path(id): Path<String>, body: Option<Json<StartRun>>) -> R<Json<Value>> {
    let engine = body.and_then(|b| b.0.engine).unwrap_or_else(|| "native".into());
    let r = jobs::enqueue(&app, &id, &engine)?;
    Ok(Json(run_json(&app, &r)))
}

// ------------------------------------------------------------------ runs

pub fn run_json(app: &App, r: &RunRow) -> Value {
    let mut v = serde_json::to_value(r).unwrap();
    v["headline"] = json!(r.headline());
    v["metrics"] = std::fs::read(app.run_dir(&r.id).join("metrics.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
    let study = app.db().study(&r.study_id);
    if let Ok(s) = study {
        v["project_id"] = json!(s.project_id);
        v["kind"] = json!(s.kind);
    }
    v
}
async fn run(State(app): S, Path(id): Path<String>) -> R<Json<Value>> {
    let r = app.db().run(&id)?;
    Ok(Json(run_json(&app, &r)))
}
async fn stop_run(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    jobs::stop(&app, &id);
    Ok(StatusCode::NO_CONTENT)
}
async fn delete_run(State(app): S, Path(id): Path<String>) -> R<StatusCode> {
    let r = app.db().run(&id)?;
    jobs::stop(&app, &id);
    app.db().delete_run(&id)?;
    let _ = std::fs::remove_dir_all(app.run_dir(&id));
    let study = app.db().study(&r.study_id);
    if let Ok(s) = study {
        app.emit(json!({"type": "project", "project_id": s.project_id}));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn run_mesh(State(app): S, Path((id, buf)): Path<(String, String)>) -> R<Response> {
    let buf = safe(&buf)?;
    binary(app.run_dir(&id).join(format!("mesh.{buf}")))
}
async fn run_field(State(app): S, Path((id, name)): Path<(String, String)>) -> R<Response> {
    let name = safe(&name)?;
    let name = name.strip_suffix(".f32").unwrap_or(name);
    binary(app.run_dir(&id).join(format!("field.{name}.f32")))
}
async fn run_slice(State(app): S, Path((id, name)): Path<(String, String)>) -> R<Json<Value>> {
    let name = safe(&name)?;
    let b = std::fs::read(app.run_dir(&id).join(format!("slice.{name}.json"))).map_err(|_| ApiError(StatusCode::NOT_FOUND, "no such slice".into()))?;
    Ok(Json(serde_json::from_slice(&b).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?))
}
async fn run_slice_field(State(app): S, Path((id, name, field)): Path<(String, String, String)>) -> R<Response> {
    let (name, field) = (safe(&name)?, safe(&field)?);
    let field = field.strip_suffix(".f32").unwrap_or(field);
    binary(app.run_dir(&id).join(format!("slice.{name}.{field}.f32")))
}
async fn run_log(State(app): S, Path(id): Path<String>) -> R<Response> {
    let t = std::fs::read_to_string(app.run_dir(&id).join("log.txt")).unwrap_or_default();
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], t).into_response())
}

// ------------------------------------------------------------------ events

async fn events(State(app): S) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = app.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|m| m.ok()).map(|s| Ok(Event::default().data(s)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
