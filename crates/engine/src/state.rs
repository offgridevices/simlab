//! Shared engine state: the database, the data folder, live events, the job
//! queues and the stop switches of running jobs.

use crate::db::Db;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{broadcast, mpsc};

pub struct App {
    db: Mutex<Db>,
    pub data: PathBuf,
    pub resources: Resources,
    pub events: broadcast::Sender<String>,
    pub gpu_queue: mpsc::UnboundedSender<String>,
    pub cpu_queue: mpsc::UnboundedSender<String>,
    pub stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub checks: Mutex<Vec<crate::system::Check>>,
    pub started: std::time::Instant,
    pub last_activity: Mutex<std::time::Instant>,
}

/// Files that ship with the app: the mesher project and the built web UI.
#[derive(Clone, Debug)]
pub struct Resources {
    pub mesher: PathBuf,
    pub web: Option<PathBuf>,
    pub docker: PathBuf,
}

impl Resources {
    /// Inside SimLab.app: Contents/Resources/{mesher,web,docker}. In the repo
    /// (cargo run): the repo's own folders.
    pub fn locate() -> Resources {
        if let Ok(dir) = std::env::var("SIMLAB_RESOURCES") {
            return Self::from(Path::new(&dir));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(contents) = exe.parent().and_then(|p| p.parent()) {
                let res = contents.join("Resources");
                if res.join("mesher").exists() {
                    return Self::from(&res);
                }
            }
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let repo = repo.canonicalize().unwrap_or(repo);
        Resources { mesher: repo.join("mesher"), web: Some(repo.join("web/dist")).filter(|p| p.exists()), docker: repo.join("docker") }
    }
    fn from(dir: &Path) -> Resources {
        Resources { mesher: dir.join("mesher"), web: Some(dir.join("web")).filter(|p| p.exists()), docker: dir.join("docker") }
    }
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("SIMLAB_DATA") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Application Support/SimLab")
}

impl App {
    pub fn new(data: PathBuf, gpu: mpsc::UnboundedSender<String>, cpu: mpsc::UnboundedSender<String>) -> Result<Arc<App>> {
        std::fs::create_dir_all(&data)?;
        for d in ["projects", "runs", "runtime"] {
            std::fs::create_dir_all(data.join(d))?;
        }
        let db = Db::open(&data.join("simlab.db"))?;
        let (events, _) = broadcast::channel(4096);
        Ok(Arc::new(App {
            db: Mutex::new(db),
            data,
            resources: Resources::locate(),
            events,
            gpu_queue: gpu,
            cpu_queue: cpu,
            stops: Mutex::new(HashMap::new()),
            checks: Mutex::new(vec![]),
            started: std::time::Instant::now(),
            last_activity: Mutex::new(std::time::Instant::now()),
        }))
    }

    /// The database. Never call this inside `if let ... = app.db()...` or a
    /// `match` on it and then again in the body: the guard lives for the whole
    /// statement and the second call would wait forever. Bind the result first.
    /// As a safety net, a lock held for over 20 s panics with a clear message
    /// instead of freezing the app.
    pub fn db(&self) -> MutexGuard<'_, Db> {
        let t0 = std::time::Instant::now();
        loop {
            match self.db.try_lock() {
                Ok(g) => return g,
                Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    if t0.elapsed().as_secs() > 20 {
                        panic!("database lock held for over 20 s (a nested app.db() call?)");
                    }
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
            }
        }
    }

    pub fn emit(&self, v: Value) {
        let _ = self.events.send(v.to_string());
    }

    pub fn touch(&self) {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = std::time::Instant::now();
    }

    pub fn model_dir(&self, project_id: &str, model_id: &str) -> PathBuf {
        self.data.join("projects").join(project_id).join("models").join(model_id)
    }
    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.data.join("runs").join(run_id)
    }
    pub fn venv(&self) -> PathBuf {
        self.data.join("runtime/venv")
    }
    pub fn python(&self) -> PathBuf {
        self.venv().join("bin/python")
    }

    pub fn settings(&self) -> Settings {
        let db = self.db();
        let v = db.setting("settings").ok().flatten().unwrap_or(json!({}));
        serde_json::from_value(v).unwrap_or_default()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub memory_budget_gb: f64,
    pub cpu_threads: usize,
    pub default_units_mm: f64,
    pub reference_engine_enabled: bool,
}
impl Default for Settings {
    fn default() -> Self {
        let ram = crate::system::ram_bytes() as f64 / crate::system::GIB;
        Settings {
            memory_budget_gb: (ram * 0.5).round().max(2.0),
            cpu_threads: crate::system::performance_cores(),
            default_units_mm: 1.0,
            reference_engine_enabled: true,
        }
    }
}
impl Settings {
    pub fn budget_bytes(&self) -> u64 {
        (self.memory_budget_gb * crate::system::GIB) as u64
    }
}
