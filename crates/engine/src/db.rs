//! The SQLite store. One connection behind a mutex: the app is single-user
//! and every query is small; heavy data lives in files next to the database.

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub struct Db {
    pub conn: Connection,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS models (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name TEXT NOT NULL, file_name TEXT NOT NULL, format TEXT NOT NULL,
  unit_mm REAL NOT NULL DEFAULT 1, status TEXT NOT NULL, message TEXT NOT NULL DEFAULT '',
  info TEXT NOT NULL DEFAULT '{}', patch_labels TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS studies (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  name TEXT NOT NULL, kind TEXT NOT NULL, config TEXT NOT NULL,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runs (
  id TEXT PRIMARY KEY, study_id TEXT NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
  status TEXT NOT NULL, engine TEXT NOT NULL, progress REAL NOT NULL DEFAULT 0,
  message TEXT NOT NULL DEFAULT '', config TEXT NOT NULL, material TEXT,
  summary TEXT, fields TEXT NOT NULL DEFAULT '[]', slices TEXT NOT NULL DEFAULT '[]',
  error TEXT, created_at TEXT NOT NULL, started_at TEXT, finished_at TEXT
);
CREATE TABLE IF NOT EXISTS materials (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS motors (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS runs_study ON runs(study_id);
CREATE INDEX IF NOT EXISTS studies_project ON studies(project_id);
CREATE INDEX IF NOT EXISTS models_project ON models(project_id);
"#;

pub fn now() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    iso(d.as_secs() as i64, d.subsec_millis())
}

/// Unix seconds → "2026-11-04T16:20:00.123Z" (civil-from-days, Howard Hinnant).
fn iso(secs: i64, ms: u32) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ModelRow {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub file_name: String,
    pub format: String,
    pub unit_mm: f64,
    pub status: String,
    pub message: String,
    pub info: Value,
    pub patch_labels: Value,
    pub created_at: String,
}

impl ModelRow {
    /// The API shape: the row's fields with `info` spread in and patch labels applied.
    pub fn to_json(&self) -> Value {
        let mut v = serde_json::json!({
            "id": self.id, "project_id": self.project_id, "name": self.name,
            "file_name": self.file_name, "format": self.format, "unit_mm": self.unit_mm,
            "status": self.status, "message": self.message, "created_at": self.created_at,
        });
        if let (Some(obj), Some(info)) = (v.as_object_mut(), self.info.as_object()) {
            for (k, val) in info {
                obj.insert(k.clone(), val.clone());
            }
            if let (Some(patches), Some(labels)) = (obj.get_mut("patches").and_then(|p| p.as_array_mut()), self.patch_labels.as_object()) {
                for p in patches {
                    let id = p["id"].as_u64().map(|i| i.to_string()).unwrap_or_default();
                    if let Some(l) = labels.get(&id) {
                        p["label"] = l.clone();
                    }
                }
            }
        }
        v
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct StudyRow {
    pub id: String,
    pub project_id: String,
    pub model_id: String,
    pub name: String,
    pub kind: String,
    pub config: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct RunRow {
    pub id: String,
    pub study_id: String,
    pub status: String,
    pub engine: String,
    pub progress: f64,
    pub message: String,
    pub config: Value,
    pub material: Option<Value>,
    pub summary: Option<Value>,
    pub fields: Value,
    pub slices: Value,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl RunRow {
    pub fn headline(&self) -> Option<String> {
        self.summary.as_ref().and_then(|s| s["headline"].as_str().map(String::from))
    }
    pub fn brief(&self) -> Value {
        serde_json::json!({
            "id": self.id, "status": self.status, "engine": self.engine, "progress": self.progress,
            "created_at": self.created_at, "finished_at": self.finished_at, "headline": self.headline(),
            "verdict": self.summary.as_ref().and_then(|s| s.get("verdict").cloned()),
            "short": self.summary.as_ref().and_then(|s| s.get("short").cloned()),
        })
    }
}

fn js(s: String) -> Value {
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        conn.execute("INSERT OR IGNORE INTO meta(key, value) VALUES ('schema', '1')", [])?;
        // runs that were going when the app last closed did not finish
        conn.execute(
            "UPDATE runs SET status='stopped', message='SimLab was closed while this ran', finished_at=?1 WHERE status IN ('queued','meshing','running')",
            [now()],
        )?;
        conn.execute("UPDATE models SET status='failed', message='SimLab was closed while this was being read; upload it again' WHERE status='processing'", [])?;
        Ok(Db { conn })
    }

    // ---- settings
    pub fn setting(&self, key: &str) -> Result<Option<Value>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get::<_, String>(0))
            .optional()?
            .map(js))
    }
    pub fn set_setting(&self, key: &str, v: &Value) -> Result<()> {
        self.conn.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, v.to_string()])?;
        Ok(())
    }

    // ---- projects
    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut st = self.conn.prepare("SELECT id,name,notes,created_at,updated_at FROM projects ORDER BY updated_at DESC")?;
        let rows = st.query_map([], |r| Ok(Project { id: r.get(0)?, name: r.get(1)?, notes: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn project(&self, id: &str) -> Result<Project> {
        self.conn
            .query_row("SELECT id,name,notes,created_at,updated_at FROM projects WHERE id=?1", [id], |r| {
                Ok(Project { id: r.get(0)?, name: r.get(1)?, notes: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)? })
            })
            .optional()?
            .ok_or_else(|| anyhow!("no project {id}"))
    }
    pub fn create_project(&self, name: &str, notes: &str) -> Result<Project> {
        let t = now();
        let p = Project { id: new_id(), name: name.into(), notes: notes.into(), created_at: t.clone(), updated_at: t };
        self.conn.execute("INSERT INTO projects VALUES(?1,?2,?3,?4,?5)", params![p.id, p.name, p.notes, p.created_at, p.updated_at])?;
        Ok(p)
    }
    pub fn update_project(&self, id: &str, name: Option<&str>, notes: Option<&str>) -> Result<Project> {
        let p = self.project(id)?;
        self.conn.execute(
            "UPDATE projects SET name=?2, notes=?3, updated_at=?4 WHERE id=?1",
            params![id, name.unwrap_or(&p.name), notes.unwrap_or(&p.notes), now()],
        )?;
        self.project(id)
    }
    pub fn touch_project(&self, id: &str) -> Result<()> {
        self.conn.execute("UPDATE projects SET updated_at=?2 WHERE id=?1", params![id, now()])?;
        Ok(())
    }
    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM projects WHERE id=?1", [id])?;
        Ok(())
    }

    // ---- models
    fn model_from(r: &rusqlite::Row) -> rusqlite::Result<ModelRow> {
        Ok(ModelRow {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            file_name: r.get(3)?,
            format: r.get(4)?,
            unit_mm: r.get(5)?,
            status: r.get(6)?,
            message: r.get(7)?,
            info: js(r.get(8)?),
            patch_labels: js(r.get(9)?),
            created_at: r.get(10)?,
        })
    }
    const MODEL_COLS: &str = "id,project_id,name,file_name,format,unit_mm,status,message,info,patch_labels,created_at";
    pub fn models(&self, project_id: &str) -> Result<Vec<ModelRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {} FROM models WHERE project_id=?1 ORDER BY created_at", Self::MODEL_COLS))?;
        let rows = st.query_map([project_id], Self::model_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn model(&self, id: &str) -> Result<ModelRow> {
        self.conn
            .query_row(&format!("SELECT {} FROM models WHERE id=?1", Self::MODEL_COLS), [id], Self::model_from)
            .optional()?
            .ok_or_else(|| anyhow!("no model {id}"))
    }
    pub fn insert_model(&self, m: &ModelRow) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO models({}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", Self::MODEL_COLS),
            params![m.id, m.project_id, m.name, m.file_name, m.format, m.unit_mm, m.status, m.message, m.info.to_string(), m.patch_labels.to_string(), m.created_at],
        )?;
        Ok(())
    }
    pub fn update_model(&self, m: &ModelRow) -> Result<()> {
        self.conn.execute(
            "UPDATE models SET name=?2, unit_mm=?3, status=?4, message=?5, info=?6, patch_labels=?7 WHERE id=?1",
            params![m.id, m.name, m.unit_mm, m.status, m.message, m.info.to_string(), m.patch_labels.to_string()],
        )?;
        Ok(())
    }
    pub fn delete_model(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM models WHERE id=?1", [id])?;
        Ok(())
    }

    // ---- studies
    const STUDY_COLS: &str = "id,project_id,model_id,name,kind,config,created_at,updated_at";
    fn study_from(r: &rusqlite::Row) -> rusqlite::Result<StudyRow> {
        Ok(StudyRow {
            id: r.get(0)?,
            project_id: r.get(1)?,
            model_id: r.get(2)?,
            name: r.get(3)?,
            kind: r.get(4)?,
            config: js(r.get(5)?),
            created_at: r.get(6)?,
            updated_at: r.get(7)?,
        })
    }
    pub fn studies(&self, project_id: &str) -> Result<Vec<StudyRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {} FROM studies WHERE project_id=?1 ORDER BY created_at", Self::STUDY_COLS))?;
        let rows = st.query_map([project_id], Self::study_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn study(&self, id: &str) -> Result<StudyRow> {
        self.conn
            .query_row(&format!("SELECT {} FROM studies WHERE id=?1", Self::STUDY_COLS), [id], Self::study_from)
            .optional()?
            .ok_or_else(|| anyhow!("no test {id}"))
    }
    pub fn insert_study(&self, s: &StudyRow) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO studies({}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", Self::STUDY_COLS),
            params![s.id, s.project_id, s.model_id, s.name, s.kind, s.config.to_string(), s.created_at, s.updated_at],
        )?;
        Ok(())
    }
    pub fn update_study(&self, s: &StudyRow) -> Result<()> {
        self.conn.execute(
            "UPDATE studies SET name=?2, kind=?3, config=?4, updated_at=?5 WHERE id=?1",
            params![s.id, s.name, s.kind, s.config.to_string(), now()],
        )?;
        Ok(())
    }
    pub fn delete_study(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM studies WHERE id=?1", [id])?;
        Ok(())
    }

    // ---- runs
    const RUN_COLS: &str = "id,study_id,status,engine,progress,message,config,material,summary,fields,slices,error,created_at,started_at,finished_at";
    fn run_from(r: &rusqlite::Row) -> rusqlite::Result<RunRow> {
        Ok(RunRow {
            id: r.get(0)?,
            study_id: r.get(1)?,
            status: r.get(2)?,
            engine: r.get(3)?,
            progress: r.get(4)?,
            message: r.get(5)?,
            config: js(r.get(6)?),
            material: r.get::<_, Option<String>>(7)?.map(js),
            summary: r.get::<_, Option<String>>(8)?.map(js),
            fields: js(r.get(9)?),
            slices: js(r.get(10)?),
            error: r.get(11)?,
            created_at: r.get(12)?,
            started_at: r.get(13)?,
            finished_at: r.get(14)?,
        })
    }
    pub fn runs(&self, study_id: &str) -> Result<Vec<RunRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {} FROM runs WHERE study_id=?1 ORDER BY created_at", Self::RUN_COLS))?;
        let rows = st.query_map([study_id], Self::run_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn runs_for_project(&self, project_id: &str) -> Result<Vec<RunRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM runs WHERE study_id IN (SELECT id FROM studies WHERE project_id=?1) ORDER BY created_at",
            Self::RUN_COLS.split(',').map(|c| format!("runs.{c}")).collect::<Vec<_>>().join(",")
        ))?;
        let rows = st.query_map([project_id], Self::run_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn run(&self, id: &str) -> Result<RunRow> {
        self.conn
            .query_row(&format!("SELECT {} FROM runs WHERE id=?1", Self::RUN_COLS), [id], Self::run_from)
            .optional()?
            .ok_or_else(|| anyhow!("no run {id}"))
    }
    pub fn insert_run(&self, r: &RunRow) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO runs({}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)", Self::RUN_COLS),
            params![
                r.id,
                r.study_id,
                r.status,
                r.engine,
                r.progress,
                r.message,
                r.config.to_string(),
                r.material.as_ref().map(|v| v.to_string()),
                r.summary.as_ref().map(|v| v.to_string()),
                r.fields.to_string(),
                r.slices.to_string(),
                r.error,
                r.created_at,
                r.started_at,
                r.finished_at
            ],
        )?;
        Ok(())
    }
    pub fn update_run(&self, r: &RunRow) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status=?2, progress=?3, message=?4, material=?5, summary=?6, fields=?7, slices=?8, error=?9, started_at=?10, finished_at=?11 WHERE id=?1",
            params![
                r.id,
                r.status,
                r.progress,
                r.message,
                r.material.as_ref().map(|v| v.to_string()),
                r.summary.as_ref().map(|v| v.to_string()),
                r.fields.to_string(),
                r.slices.to_string(),
                r.error,
                r.started_at,
                r.finished_at
            ],
        )?;
        Ok(())
    }
    pub fn set_run_progress(&self, id: &str, status: &str, progress: f64, message: &str) -> Result<()> {
        self.conn.execute("UPDATE runs SET status=?2, progress=?3, message=?4 WHERE id=?1", params![id, status, progress, message])?;
        Ok(())
    }
    pub fn delete_run(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM runs WHERE id=?1", [id])?;
        Ok(())
    }

    // ---- custom library entries
    pub fn custom(&self, table: &str) -> Result<Vec<Value>> {
        let mut st = self.conn.prepare(&format!("SELECT json FROM {table} ORDER BY id"))?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).map(js).collect())
    }
    pub fn put_custom(&self, table: &str, id: &str, v: &Value) -> Result<()> {
        self.conn.execute(&format!("INSERT INTO {table}(id,json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json"), params![id, v.to_string()])?;
        Ok(())
    }
    pub fn delete_custom(&self, table: &str, id: &str) -> Result<()> {
        self.conn.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_dates() {
        assert_eq!(super::iso(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(super::iso(1_793_826_937, 5), "2026-11-04T21:15:37.005Z");
    }
}
