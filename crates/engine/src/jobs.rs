//! The job runner: two queues (the GPU runs one wind tunnel at a time, the
//! CPU one stress/propeller/reference job at a time), live progress, stop.

use crate::api::{find_material, parse_config};
use crate::db::{RunRow, new_id, now};
use crate::state::App;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use simlab_core::{Progress, StudyConfig};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;

pub fn enqueue(app: &Arc<App>, study_id: &str, engine: &str) -> Result<RunRow> {
    let s = app.db().study(study_id)?;
    let cfg = parse_config(&s.config)?;
    let m = app.db().model(&s.model_id)?;
    if m.status != "ready" {
        bail!("the model is not ready yet ({})", if m.message.is_empty() { &m.status } else { &m.message });
    }
    if !matches!(engine, "native" | "reference") {
        bail!("engine must be native or reference");
    }
    if engine == "reference" && matches!(cfg, StudyConfig::Prop(_)) {
        bail!("the propeller test has no reference engine; use a wind tunnel test with spin for a reference run");
    }
    let material = match &cfg {
        StudyConfig::Strength(c) => Some(serde_json::to_value(find_material(app, &c.material_id)?)?),
        _ => None,
    };
    let r = RunRow {
        id: new_id(),
        study_id: study_id.into(),
        status: "queued".into(),
        engine: engine.into(),
        progress: 0.0,
        message: "Waiting its turn".into(),
        config: s.config.clone(),
        material,
        summary: None,
        fields: json!([]),
        slices: json!([]),
        error: None,
        created_at: now(),
        started_at: None,
        finished_at: None,
    };
    app.db().insert_run(&r)?;
    app.db().touch_project(&s.project_id)?;
    std::fs::create_dir_all(app.run_dir(&r.id))?;
    app.stops.lock().unwrap().insert(r.id.clone(), Arc::new(AtomicBool::new(false)));
    let gpu = matches!(cfg, StudyConfig::Wind(_)) && engine == "native";
    let q = if gpu { &app.gpu_queue } else { &app.cpu_queue };
    q.send(r.id.clone())?;
    emit_run(app, &r, &s.project_id);
    Ok(r)
}

pub fn stop(app: &App, run_id: &str) {
    if let Some(f) = app.stops.lock().unwrap().get(run_id) {
        f.store(true, Ordering::Relaxed);
    }
    // a queued run stops right away
    let found = app.db().run(run_id);
    if let Ok(mut r) = found {
        if r.status == "queued" {
            r.status = "stopped".into();
            r.message = "Stopped before it started".into();
            r.finished_at = Some(now());
            let _ = app.db().update_run(&r);
            let study = app.db().study(&r.study_id);
            if let Ok(s) = study {
                emit_run(app, &r, &s.project_id);
            }
        }
    }
}

fn emit_run(app: &App, r: &RunRow, project_id: &str) {
    let mut b = r.brief();
    b["study_id"] = json!(r.study_id);
    b["project_id"] = json!(project_id);
    b["message"] = json!(r.message);
    app.emit(json!({"type": "run", "run": b}));
}

pub async fn worker(app: Arc<App>, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(id) = rx.recv().await {
        let app2 = app.clone();
        let _ = tokio::task::spawn_blocking(move || execute(&app2, &id)).await;
    }
}

/// What a solver hands back.
#[derive(Default)]
pub struct Outcome {
    pub summary: Value,
    pub fields: Value,
    pub slices: Value,
}

/// Progress for one run: throttled database/UI updates, metric history,
/// a log file, and the stop switch.
pub struct Ctx<'a> {
    pub app: &'a App,
    pub run_id: String,
    pub project_id: String,
    pub study_id: String,
    pub engine: String,
    pub created_at: String,
    pub dir: PathBuf,
    pub stop: Arc<AtomicBool>,
    pub status: Mutex<String>,
    last: Mutex<(Instant, f32)>,
    metrics: Mutex<BTreeMap<String, Vec<(u64, f64)>>>,
    log: Mutex<Option<std::fs::File>>,
    /// this phase covers [lo, hi] of the overall progress bar
    pub span: Mutex<(f32, f32)>,
}

impl<'a> Ctx<'a> {
    pub fn log(&self, line: &str) {
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
    pub fn phase(&self, status: &str, lo: f32, hi: f32) {
        *self.status.lock().unwrap() = status.into();
        *self.span.lock().unwrap() = (lo, hi);
    }
    fn save_metrics(&self) {
        let m = self.metrics.lock().unwrap();
        let v: BTreeMap<&String, Vec<[f64; 2]>> = m.iter().map(|(k, v)| (k, v.iter().map(|(s, x)| [*s as f64, *x]).collect())).collect();
        let _ = std::fs::write(self.dir.join("metrics.json"), serde_json::to_vec(&v).unwrap_or_default());
    }
}

impl Progress for Ctx<'_> {
    fn report(&self, fraction: f32, message: &str) {
        let (lo, hi) = *self.span.lock().unwrap();
        let overall = lo + (hi - lo) * fraction.clamp(0.0, 1.0);
        // at most ~7 updates a second; the last one of a phase always lands
        let mut last = self.last.lock().unwrap();
        if last.0.elapsed().as_millis() < 150 && fraction < 1.0 {
            return;
        }
        *last = (Instant::now(), overall);
        drop(last);
        let status = self.status.lock().unwrap().clone();
        let _ = self.app.db().set_run_progress(&self.run_id, &status, overall as f64, message);
        self.app.emit(json!({"type": "run", "run": {
            "id": self.run_id, "status": status, "progress": overall, "message": message, "project_id": self.project_id,
            "study_id": self.study_id, "engine": self.engine, "created_at": self.created_at,
        }}));
        self.log(&format!("[{:5.1}%] {message}", overall * 100.0));
    }
    fn metric(&self, name: &str, step: u64, value: f64) {
        if !value.is_finite() {
            return;
        }
        let n = {
            let mut m = self.metrics.lock().unwrap();
            let v = m.entry(name.to_string()).or_default();
            v.push((step, value));
            // keep the history small: drop every other point past 4000
            if v.len() > 4000 {
                let thinned: Vec<_> = v.iter().enumerate().filter(|(i, _)| i % 2 == 0).map(|(_, x)| *x).collect();
                *v = thinned;
            }
            v.len()
        };
        self.app.emit(json!({"type": "metric", "run_id": self.run_id, "name": name, "step": step, "value": value}));
        if n % 50 == 0 {
            self.save_metrics();
        }
    }
    fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

fn execute(app: &Arc<App>, run_id: &str) {
    let Ok(mut r) = app.db().run(run_id) else { return };
    if r.status != "queued" {
        return;
    }
    let stop = app.stops.lock().unwrap().get(run_id).cloned().unwrap_or_default();
    let project_id = app.db().study(&r.study_id).map(|s| s.project_id).unwrap_or_default();
    let dir = app.run_dir(run_id);
    let _ = std::fs::create_dir_all(&dir);
    r.status = "running".into();
    r.started_at = Some(now());
    r.message = "Starting".into();
    let _ = app.db().update_run(&r);
    emit_run(app, &r, &project_id);

    let ctx = Ctx {
        app,
        run_id: run_id.into(),
        project_id: project_id.clone(),
        study_id: r.study_id.clone(),
        engine: r.engine.clone(),
        created_at: r.created_at.clone(),
        dir: dir.clone(),
        stop,
        status: Mutex::new("running".into()),
        last: Mutex::new((Instant::now() - std::time::Duration::from_secs(1), 0.0)),
        metrics: Mutex::new(BTreeMap::new()),
        log: Mutex::new(std::fs::File::create(dir.join("log.txt")).ok()),
        span: Mutex::new((0.0, 1.0)),
    };
    let t0 = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::solve::run(&ctx, &r)));
    ctx.save_metrics();
    let mut r = app.db().run(run_id).unwrap_or(r);
    r.finished_at = Some(now());
    match result {
        Ok(Ok(out)) => {
            r.status = "done".into();
            r.progress = 1.0;
            r.message = format!("Finished in {}", human_secs(t0.elapsed().as_secs_f64()));
            r.summary = Some(out.summary);
            r.fields = out.fields;
            r.slices = out.slices;
        }
        Ok(Err(e)) if e.downcast_ref::<simlab_core::Cancelled>().is_some() || ctx.cancelled() => {
            r.status = "stopped".into();
            r.message = "Stopped".into();
        }
        Ok(Err(e)) => {
            r.status = "failed".into();
            r.message = "Failed".into();
            r.error = Some(format!("{e:#}"));
            ctx.log(&format!("ERROR: {e:#}"));
        }
        Err(p) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            r.status = "failed".into();
            r.message = "Failed".into();
            r.error = Some(format!("the solver crashed: {msg}"));
            ctx.log(&format!("PANIC: {msg}"));
        }
    }
    let _ = app.db().update_run(&r);
    app.stops.lock().unwrap().remove(run_id);
    emit_run(app, &r, &project_id);
    app.emit(json!({"type": "project", "project_id": project_id}));
}

pub fn human_secs(s: f64) -> String {
    if s < 60.0 {
        format!("{s:.0} s")
    } else if s < 3600.0 {
        format!("{} min {} s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{:.1} h", s / 3600.0)
    }
}

pub fn estimate(app: &App, study_id: &str) -> Result<Value> {
    let s = app.db().study(study_id)?;
    let m = app.db().model(&s.model_id)?;
    if m.status != "ready" {
        bail!("the model is not ready yet");
    }
    crate::solve::estimate(app, &m, &parse_config(&s.config)?)
}
