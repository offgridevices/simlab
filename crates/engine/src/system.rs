//! The system check: everything SimLab needs, checked every time it starts.
//! Missing pieces are reported with a plain-words question; nothing is
//! installed or started without the user saying yes (POST /api/system/fix).

use crate::state::App;
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

#[derive(Serialize, Clone, Debug)]
pub struct Check {
    pub id: String,
    pub label: String,
    /// ok | missing | error | working | optional
    pub status: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Fix {
    pub label: String,
    pub asks: String,
}

fn check(id: &str, label: &str, status: &str, detail: impl Into<String>, fix: Option<Fix>) -> Check {
    Check { id: id.into(), label: label.into(), status: status.into(), detail: detail.into(), fix }
}

fn sysctl(name: &str) -> Option<String> {
    let out = Command::new("/usr/sbin/sysctl").arg("-n").arg(name).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
pub const GIB: f64 = 1073741824.0;
pub fn ram_bytes() -> u64 {
    sysctl("hw.memsize").and_then(|s| s.parse().ok()).unwrap_or(8_000_000_000)
}
pub fn cpu_name() -> String {
    sysctl("machdep.cpu.brand_string").unwrap_or_else(|| "Unknown CPU".into())
}
pub fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}
pub fn performance_cores() -> usize {
    sysctl("hw.perflevel0.physicalcpu").and_then(|s| s.parse().ok()).unwrap_or_else(cores)
}

/// Finds a program even when the app was opened from Finder (which gives it
/// almost no PATH).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var("PATH").unwrap_or_default().split(':').map(PathBuf::from).collect();
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(d.into());
    }
    for d in [".local/bin", ".cargo/bin", ".colima/bin"] {
        dirs.push(PathBuf::from(&home).join(d));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn docker_env() -> Vec<(String, String)> {
    // Colima's socket, when Docker's default context is not set up for it
    let home = std::env::var("HOME").unwrap_or_default();
    let sock = PathBuf::from(&home).join(".colima/default/docker.sock");
    if std::env::var("DOCKER_HOST").is_err() && sock.exists() {
        vec![("DOCKER_HOST".into(), format!("unix://{}", sock.display()))]
    } else {
        vec![]
    }
}

pub fn docker_running() -> bool {
    let Some(docker) = find_program("docker") else { return false };
    Command::new(docker).envs(docker_env()).args(["info", "--format", "{{.ServerVersion}}"]).output().map(|o| o.status.success()).unwrap_or(false)
}

pub const REFERENCE_IMAGE: &str = "simlab-reference:1";

pub fn reference_image_present() -> bool {
    let Some(docker) = find_program("docker") else { return false };
    Command::new(docker).envs(docker_env()).args(["image", "inspect", REFERENCE_IMAGE]).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Runs every check. Quick (well under a second) except for the Gmsh import.
pub fn run_checks(app: &App) -> Vec<Check> {
    let mut out = vec![];
    let ram = ram_bytes();
    out.push(check("machine", "This Mac", "ok", format!("{} · {} cores · {:.0} GB memory", cpu_name(), cores(), ram as f64 / GIB), None));

    match crate::solve::gpu_info() {
        Ok(g) => out.push(check("gpu", "Graphics chip (wind tunnel)", "ok", g, None)),
        Err(e) => out.push(check("gpu", "Graphics chip (wind tunnel)", "error", format!("No usable GPU: {e}. The wind tunnel cannot run."), None)),
    }

    // the mesher: uv + a hash-locked Gmsh in SimLab's own folder
    let uv = find_program("uv");
    let py = app.python();
    let gmsh_ok = py.exists()
        && Command::new(&py)
            .args(["-I", "-c", "import gmsh, numpy; print(gmsh.__version__)"])
            .output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "4.15.2")
            .unwrap_or(false);
    if gmsh_ok {
        out.push(check("mesher", "Gmsh mesher 4.15.2", "ok", "Reads STEP and STL, builds the stress-test mesh", None));
    } else if uv.is_none() {
        out.push(check(
            "mesher",
            "Gmsh mesher",
            "missing",
            "Needs uv (Python package manager by Astral) to install Gmsh safely. Install it with: brew install uv",
            None,
        ));
    } else {
        out.push(check(
            "mesher",
            "Gmsh mesher",
            "missing",
            "Not set up yet. Needed to read STEP files and for every stress test.",
            Some(Fix {
                label: "Set up Gmsh".into(),
                asks: "Download Gmsh 4.15.2 and NumPy 2.5.4 (about 60 MB, checked against locked hashes) into SimLab's own folder? Nothing is installed system-wide.".into(),
            }),
        ));
    }

    // reference engines in Docker (optional)
    let docker = find_program("docker");
    let colima = find_program("colima");
    if docker.is_none() {
        out.push(check(
            "docker",
            "Docker (optional, for reference runs)",
            "optional",
            "Not installed. SimLab works without it; reference runs with CalculiX and OpenFOAM need it. To add it: brew install colima docker",
            None,
        ));
    } else if !docker_running() {
        out.push(check(
            "docker",
            "Docker (optional, for reference runs)",
            "optional",
            if colima.is_some() { "Installed (Colima) but not running. SimLab starts it only for a reference run." } else { "Installed but not running." },
            colima.map(|_| Fix {
                label: "Start Colima".into(),
                asks: {
                    let (cpu, mem) = colima_profile().unwrap_or((COLIMA_NEW_CPU, COLIMA_NEW_MEMORY_GB));
                    format!("Start Colima now? It runs a small Linux machine using up to {mem} GB of memory and {cpu} CPU cores (your Colima settings) until you stop it or quit SimLab.")
                },
            }),
        ));
    } else if !reference_image_present() {
        out.push(check(
            "docker",
            "Reference engines (CalculiX + OpenFOAM)",
            "optional",
            "Docker is running; the reference image is not built yet.",
            Some(Fix {
                label: "Build reference image".into(),
                asks: "Build SimLab's reference image? It downloads OpenFOAM v2606 (OpenCFD's official image) and CalculiX 2.21 (Ubuntu's signed package), about 1.2 GB, and takes a few minutes.".into(),
            }),
        ));
    } else {
        out.push(check("docker", "Reference engines (CalculiX + OpenFOAM)", "ok", "Ready in Docker", None));
    }

    let free = disk_free(&app.data);
    out.push(check(
        "disk",
        "Storage",
        if free > 5_000_000_000 { "ok" } else { "error" },
        format!("{:.0} GB free · data in {}", free as f64 / 1e9, app.data.display()),
        None,
    ));
    out
}

fn disk_free(p: &std::path::Path) -> u64 {
    let out = Command::new("/bin/df").arg("-k").arg(p).output();
    out.ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().nth(1).and_then(|l| l.split_whitespace().nth(3).and_then(|s| s.parse::<u64>().ok())))
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

pub fn refresh(app: &Arc<App>) {
    let checks = run_checks(app);
    for c in &checks {
        app.emit(json!({"type": "check", "check": c}));
    }
    *app.checks.lock().unwrap_or_else(|e| e.into_inner()) = checks;
}

fn set_working(app: &Arc<App>, id: &str, detail: &str) {
    let mut checks = app.checks.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = checks.iter_mut().find(|c| c.id == id) {
        c.status = "working".into();
        c.detail = detail.into();
        c.fix = None;
        app.emit(json!({"type": "check", "check": c}));
    }
}

/// Sizes for a brand-new Colima (only when none exists yet).
const COLIMA_NEW_CPU: u32 = 8;
const COLIMA_NEW_MEMORY_GB: u32 = 8;

/// CPU cores and memory (GB) of the existing default Colima, if there is one.
fn colima_profile() -> Option<(u32, u32)> {
    let home = std::env::var("HOME").ok()?;
    let text = std::fs::read_to_string(format!("{home}/.colima/default/colima.yaml")).ok()?;
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v.round() as u32)
    };
    Some((field("cpu:")?, field("memory:")?))
}

/// Runs a fix the user agreed to. Blocking; call from spawn_blocking.
pub fn fix(app: &Arc<App>, id: &str) -> anyhow::Result<()> {
    let result = match id {
        "mesher" => {
            set_working(app, id, "Downloading Gmsh and NumPy…");
            setup_mesher(app)
        }
        "docker" if !docker_running() => {
            set_working(app, id, "Starting Colima…");
            let colima = find_program("colima").ok_or_else(|| anyhow::anyhow!("Colima is not installed"))?;
            // An existing Colima keeps its own CPU and memory settings: passing
            // --cpu/--memory would overwrite them for everything else you run.
            let mut cmd = Command::new(colima);
            cmd.arg("start");
            if colima_profile().is_none() {
                cmd.args(["--cpu", &COLIMA_NEW_CPU.to_string(), "--memory", &COLIMA_NEW_MEMORY_GB.to_string(), "--vm-type", "vz"]);
            }
            run_ok(&mut cmd)
        }
        "docker" => {
            set_working(app, id, "Building the reference image (a few minutes)…");
            build_reference_image(app)
        }
        _ => Err(anyhow::anyhow!("nothing to fix for {id}")),
    };
    refresh(app);
    result
}

pub fn setup_mesher(app: &App) -> anyhow::Result<()> {
    let uv = find_program("uv").ok_or_else(|| anyhow::anyhow!("uv is not installed (brew install uv)"))?;
    let mut cmd = Command::new(uv);
    cmd.args(["sync", "--frozen", "--no-install-project", "--project"])
        .arg(&app.resources.mesher)
        .env("UV_PROJECT_ENVIRONMENT", app.venv())
        .env("UV_PYTHON_PREFERENCE", "managed")
        .env_remove("VIRTUAL_ENV");
    run_ok(&mut cmd)
}

fn build_reference_image(app: &App) -> anyhow::Result<()> {
    let docker = find_program("docker").ok_or_else(|| anyhow::anyhow!("Docker is not installed"))?;
    run_ok(Command::new(docker).envs(docker_env()).args(["build", "-t", REFERENCE_IMAGE]).arg(&app.resources.docker))
}

fn run_ok(cmd: &mut Command) -> anyhow::Result<()> {
    let out = cmd.output()?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(6).collect();
        Err(anyhow::anyhow!("{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
    }
}

/// Settings → Update: pulls the newest SimLab from GitHub into the folder it
/// was installed from and rebuilds the app. The new version is used after a
/// restart. Blocking; call from spawn_blocking.
pub const RELEASES_URL: &str = "https://github.com/offgridevices/simlab/releases/latest";

/// How this copy was installed: built from a git checkout (`make install`
/// writes `source_dir`) or downloaded from a GitHub release.
pub fn source_dir(app: &App) -> Option<PathBuf> {
    let src = PathBuf::from(std::fs::read_to_string(app.data.join("source_dir")).ok()?.trim());
    src.join(".git").exists().then_some(src)
}

pub struct UpdateResult {
    pub message: String,
    /// where to download the newer version (downloaded copies only)
    pub url: Option<String>,
    pub latest: Option<String>,
}

/// Built from source: pull and rebuild. Downloaded: ask GitHub for the newest
/// release and say where to get it (nothing is downloaded or replaced).
pub fn update(app: &Arc<App>) -> anyhow::Result<UpdateResult> {
    let Some(src) = source_dir(app) else {
        return check_release(env!("CARGO_PKG_VERSION"));
    };
    let git = find_program("git").ok_or_else(|| anyhow::anyhow!("git is missing"))?;
    let before = Command::new(&git).arg("-C").arg(&src).args(["rev-parse", "HEAD"]).output()?;
    run_ok(Command::new(&git).arg("-C").arg(&src).args(["pull", "--ff-only"]))?;
    let after = Command::new(&git).arg("-C").arg(&src).args(["rev-parse", "HEAD"]).output()?;
    let done = |m: &str| UpdateResult { message: m.into(), url: None, latest: None };
    if before.stdout == after.stdout {
        return Ok(done("SimLab is up to date."));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let _ = std::fs::create_dir_all(format!("{home}/Library/Logs/SimLab"));
    let log = std::fs::File::create(format!("{home}/Library/Logs/SimLab/update.log"))?;
    let status = Command::new("/bin/bash")
        .arg(src.join("install.sh"))
        .args(["--yes", "--no-open"])
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()?;
    if !status.success() {
        anyhow::bail!("the update did not build; details in ~/Library/Logs/SimLab/update.log");
    }
    Ok(done("Updated. Quit and reopen SimLab to use the new version."))
}

fn check_release(current: &str) -> anyhow::Result<UpdateResult> {
    // macOS's own curl: no extra network library in the engine
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "15", "-H", "Accept: application/vnd.github+json", "-A", "SimLab-update-check"])
        .arg("https://api.github.com/repos/offgridevices/simlab/releases/latest")
        .output()?;
    if !out.status.success() {
        anyhow::bail!("could not reach GitHub to check for a newer SimLab (no internet, or no release yet)");
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let tag = v["tag_name"].as_str().unwrap_or_default().trim_start_matches('v').to_string();
    let url = v["html_url"].as_str().unwrap_or(RELEASES_URL).to_string();
    Ok(if newer(&tag, current) {
        UpdateResult {
            message: format!("SimLab {tag} is out (you have {current}). Download it, then drag it into Applications to replace this one. Your projects stay."),
            url: Some(url),
            latest: Some(tag),
        }
    } else {
        UpdateResult { message: format!("SimLab is up to date ({current})."), url: None, latest: Some(tag) }
    })
}

/// "0.10.0" > "0.9.2"; anything unparsable is never newer
fn newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<Vec<u64>> { s.split(['.', '-']).take(3).map(|p| p.parse().ok()).collect() };
    match (parse(latest), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod update_tests {
    #[test]
    fn version_order() {
        assert!(super::newer("0.10.0", "0.9.2"));
        assert!(super::newer("1.0.0", "0.99.99"));
        assert!(!super::newer("0.1.0", "0.1.0"));
        assert!(!super::newer("0.0.9", "0.1.0"));
        assert!(!super::newer("", "0.1.0"));
    }
}
