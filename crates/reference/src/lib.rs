//! SimLab's reference wind tunnel: OpenFOAM v2606 (snappyHexMesh +
//! simpleFoam, steady RANS k-ω SST, MRF for spinning parts) run in the
//! `simlab-reference:1` Docker image. Slower than the built-in GPU solver,
//! but a well-proven answer to compare it against.
//!
//! ```text
//! run_openfoam(surface_m, cfg, model_unit_m, case_dir, threads, progress)
//!   1. plan        domain, cell sizes, refinement, MRF zone, BCs   (plan.rs)
//!   2. write case  dictionaries + Allrun + sample points           (case.rs)
//!   3. docker run  surfaceFeatureExtract → blockMesh → snappyHexMesh
//!                  → checkMesh → decomposePar → simpleFoam (mpirun)
//!                  → postProcess (samples) → reconstructPar
//!                  output streamed and parsed live                  (logparse.rs)
//!   4. read back   forces (averaged), surface pressure, slices      (post.rs)
//! ```
//!
//! **Stopping early.** While simpleFoam runs, the engine watches the force
//! and moment on the part. Once they have stayed within ±0.2 % of their mean
//! for 200 iterations (after at least 300–400), it rewrites
//! `system/controlDict` with `stopAt writeNow` and OpenFOAM finishes
//! cleanly. Otherwise it runs to the quality's iteration cap, and
//! `converged` is false with a warning giving the spread.
//!
//! **Cancelling.** `Progress::cancelled()` is polled twice a second; when it
//! turns true the container is killed (`docker kill`) and the function
//! returns an error wrapping [`simlab_core::Cancelled`]. The container is
//! also killed if this function's thread unwinds.
//!
//! **Validation** (OpenFOAM v2606, 8 MPI ranks in Colima, 8 cores / 12 GB,
//! on an M4 Max, November 2026). Propeller references are the user's 2026
//! study (OpenFOAM v2506, same method, 12 ranks): T002, T021, T003.
//!
//! | case | quality | cells | result | reference | diff | time |
//! |---|---|---|---|---|---|---|
//! | sphere Re 100 (laminar) | Draft | 347 k | Cd 1.093 | 1.087 Clift / 1.092 Schiller–Naumann | +0.6 % / +0.1 % | 1 min |
//! | sphere Re 100 (laminar) | Normal | 1.59 M | Cd 1.101 | same | +1.3 % / +0.8 % | 11 min |
//! | sphere Re 1000 (SST) | Draft | 341 k | Cd 0.483 | 0.471 Clift | +2.4 % | 2.5 min |
//! | sphere Re 1000 (SST) | Normal | 1.59 M | Cd 0.472 | 0.471 Clift | +0.2 % | 15 min |
//! | E-v5 prop, 45,800 rpm hover | Normal | 1.86 M | 345.8 g, 22.06 mN·m | 346.0 g, 22.09 (T002) | −0.06 %, −0.16 % | 10 min |
//! | P-v2 prop, 45,800 rpm hover | Normal | 1.86 M | 380.0 g, 24.82 mN·m | 378.4 g, 24.66 (T021) | +0.4 %, +0.6 % | 10 min |
//! | E-v5 prop, 17,000 rpm hover | Normal | 1.86 M | 45.7 g, 3.115 mN·m | 45.8 g, 3.122 (T003) | −0.1 %, −0.2 % | 10 min |
//! | E-v5 prop, 45,800 rpm | Draft | 589 k | 347.0 g, 23.17 mN·m | | +0.3 %, +4.9 % | 4 min |
//! | E-v5 prop, 45,800 rpm | Fine | 2.31 M | 335.1 g, 20.50 mN·m | | −3.1 %, −7.2 % | 26 min |
//!
//! The archived T002 case itself, re-run unchanged in v2606, reproduces its
//! v2506 numbers to 0.01 %; where the background grid falls relative to the
//! part moves results by about 0.6 % (hence the grid node at the centre).
//! The propeller's torque depends on how finely the blade edges are meshed
//! (Draft → Normal → Fine: 23.2 → 22.0 → 20.5 mN·m) much more than its
//! thrust does: treat ±3 % thrust and ±7 % torque as the method's own spread.
//!
//! **Logs.** Everything the container prints goes to `<case_dir>/simlab.log`;
//! each OpenFOAM step also leaves `<case_dir>/log.<step>`. The case stays on
//! disk (mesh and final fields reconstructed) and can be opened in ParaView
//! or re-run with `<case_dir>/Allrun`.

pub mod case;
pub mod geom;
pub mod logparse;
pub mod plan;
pub mod post;

use anyhow::{Context, Result, bail};
use logparse::{Event, LogState};
use plan::Plan;
pub use post::{RefSlice, SliceField};
use serde::{Deserialize, Serialize};
use simlab_core::{Cancelled, Progress, TriMesh, Vec3, WindConfig, dot, norm, sub};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The Docker image `docker/Dockerfile` builds.
pub const IMAGE: &str = "simlab-reference:1";

/// What a reference run found. Forces in newtons on the part, moments in
/// N·m about `moment_center_m`, model axes. Values are the mean over the
/// last iterations (see `averaged_over`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WindReference {
    pub force_n: Vec3,
    pub moment_nm: Vec3,
    /// the spin centre, or the centre of the part's bounding box
    pub moment_center_m: Vec3,
    /// force along the wind direction (`cfg.wind_dir`)
    pub drag_n: f64,
    /// force along "up" (model +z made square to the wind; +y if the wind is vertical)
    pub lift_n: f64,
    /// force along lift × wind (to the left, looking downstream)
    pub side_n: f64,
    /// drag / (½ ρ U² A), A = the part's frontal (projected) area; None in still air
    pub cd: Option<f64>,
    pub ref_area_m2: f64,
    /// U·L/ν with L the part's largest size (spinning: tip speed and diameter)
    pub reynolds: f64,
    /// spinning parts: force along the thrust direction (opposite the wake), N
    pub thrust_n: Option<f64>,
    /// spinning parts: torque needed to keep it turning (positive = it resists), N·m
    pub torque_nm: Option<f64>,
    /// torque × angular speed, W
    pub power_w: Option<f64>,
    /// spinning parts: the direction the wake leaves along
    pub wake_dir: Option<Vec3>,
    pub cells: u64,
    pub iterations: u32,
    pub averaged_over: u32,
    pub seconds: f64,
    pub converged: bool,
    pub turbulence: String,
    /// y+ on the part (min, average, max), from OpenFOAM's yPlus function object
    pub y_plus: Option<[f64; 3]>,
    pub warnings: Vec<String>,
    /// pressure above ambient (Pa) at each vertex of the input surface
    pub surface_pressure: Vec<f32>,
    pub slices: Vec<RefSlice>,
    pub case_dir: PathBuf,
    pub log_path: PathBuf,
}

/// Where to find Docker and which image to run.
#[derive(Clone, Debug)]
pub struct RunOptions {
    pub docker: PathBuf,
    pub image: String,
    /// extra environment for the docker CLI (DOCKER_HOST for Colima)
    pub env: Vec<(String, String)>,
    /// stop simpleFoam once the forces have settled (default); false runs
    /// to the quality's iteration cap, as the propeller study did
    pub early_stop: bool,
    /// results are the mean over this many final iterations (default 200;
    /// the propeller study used 300)
    pub average_last: usize,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            docker: find_docker().unwrap_or_else(|| "docker".into()),
            image: IMAGE.into(),
            env: docker_env(),
            early_stop: true,
            average_last: 200,
        }
    }
}

pub fn find_docker() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var("PATH").unwrap_or_default().split(':').map(PathBuf::from).collect();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    dirs.push(PathBuf::from(&home).join(".colima/bin"));
    dirs.into_iter().map(|d| d.join("docker")).find(|p| p.is_file())
}

/// Colima's socket, when DOCKER_HOST is not set (same rule as the engine).
pub fn docker_env() -> Vec<(String, String)> {
    let home = std::env::var("HOME").unwrap_or_default();
    let sock = PathBuf::from(&home).join(".colima/default/docker.sock");
    if std::env::var("DOCKER_HOST").is_err() && sock.exists() {
        vec![("DOCKER_HOST".into(), format!("unix://{}", sock.display()))]
    } else {
        vec![]
    }
}

/// A case written to disk and ready to run.
pub struct Prepared {
    pub plan: Plan,
    pub slices: Vec<RefSlice>,
    pub probe_points: Vec<Vec3>,
    pub slice_points: Vec<Vec<Vec3>>,
    pub estimated_cells: u64,
}

/// Plans and writes a complete case (no Docker needed).
pub fn prepare_case(surface: &TriMesh, cfg: &WindConfig, model_unit_m: f64, case_dir: &Path, threads: usize) -> Result<Prepared> {
    let plan = plan::plan(surface, cfg, model_unit_m)?;
    case::write_case(case_dir, surface, &plan, threads)?;
    let slices = post::slice_layout(&plan, surface);
    let probe_points = post::vertex_probe_points(surface, 0.5 * plan.h0 / f64::powi(2.0, plan.surface_level.1 as i32));
    std::fs::write(case_dir.join("system/vertices.points"), case::points_file(&probe_points))?;
    let mut slice_points = vec![];
    for s in &slices {
        let pts = post::slice_points(s);
        std::fs::write(case_dir.join(format!("system/{}.points", s.name)), case::points_file(&pts))?;
        slice_points.push(pts);
    }
    let names: Vec<String> = slices.iter().map(|s| s.name.clone()).collect();
    std::fs::write(case_dir.join("system/sampleDict"), case::sample_dict(&names))?;
    let estimated_cells = plan::estimate_cells(&plan, surface);
    std::fs::write(
        case_dir.join("case.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "kind": format!("{:?}", plan.kind),
            "center_m": plan.center,
            "flow_dir": plan.frame.e1,
            "domain_lo_m": plan.lo, "domain_hi_m": plan.hi,
            "background_cells": plan.cells,
            "background_cell_m": plan.h0,
            "surface_level": [plan.surface_level.0, plan.surface_level.1],
            "surface_cell_m": plan.surface_cell(),
            "layers": plan.layers,
            "turbulence": plan.turbulence_name(),
            "reynolds": plan.reynolds,
            "ref_area_m2": plan.ref_area,
            "ref_length_m": plan.ref_length,
            "max_iterations": plan.max_iterations,
            "estimated_cells": estimated_cells,
            "threads": threads,
            "rotor": plan.rotor.as_ref().map(|r| serde_json::json!({
                "axis": r.axis, "omega_rad_s": r.omega, "rpm": r.omega * 60.0 / std::f64::consts::TAU,
                "radius_m": r.radius, "zone_radius_m": r.zone_radius, "wake_dir": r.jet_dir, "wake_vote": r.jet_vote,
            })),
            "warnings": plan.warnings,
        }))?,
    )?;
    Ok(Prepared { plan, slices, probe_points, slice_points, estimated_cells })
}

/// Runs a reference wind-tunnel case in Docker. `surface` is in metres;
/// `model_unit_m` converts the config's model coordinates (`center_mm`) to
/// metres (0.001 for a model in millimetres). `case_dir` is created (or
/// reused and cleaned) and kept afterwards. `threads` MPI ranks.
pub fn run_openfoam(
    surface: &TriMesh,
    cfg: &WindConfig,
    model_unit_m: f64,
    case_dir: &Path,
    threads: usize,
    progress: &dyn Progress,
) -> Result<WindReference> {
    run_openfoam_with(&RunOptions::default(), surface, cfg, model_unit_m, case_dir, threads, progress)
}

pub fn run_openfoam_with(
    opts: &RunOptions,
    surface: &TriMesh,
    cfg: &WindConfig,
    model_unit_m: f64,
    case_dir: &Path,
    threads: usize,
    progress: &dyn Progress,
) -> Result<WindReference> {
    let started = Instant::now();
    progress.report(0.0, "Writing the OpenFOAM case");

    // Docker and the image are there?
    let ok = Command::new(&opts.docker)
        .envs(opts.env.iter().cloned())
        .args(["image", "inspect", &opts.image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!("Docker is not running or the reference image ({}) is not built. Start Colima and build it from Settings → System check.", opts.image);
    }
    // more MPI ranks than the Docker VM has cores only slows the run down
    let vm_cpus = docker_cpus(opts);
    let mut threads = threads.max(1);
    let mut early_warnings = vec![];
    if let Some(n) = vm_cpus
        && threads > n {
            early_warnings.push(format!("Asked for {threads} threads but Docker has {n} cores; used {n}."));
            threads = n;
        }
    std::fs::create_dir_all(case_dir)?;
    let case_dir = case_dir.canonicalize()?;
    let prep = prepare_case(surface, cfg, model_unit_m, &case_dir, threads)?;
    let p = &prep.plan;

    let name = format!("simlab-ref-{}-{}", std::process::id(), started.elapsed().as_nanos() ^ now_nanos());
    let mut child = Command::new(&opts.docker)
        .envs(opts.env.iter().cloned())
        .args(["run", "--rm", "--init", "--name", &name, "--label", "simlab=reference", "--shm-size", "1g", "--entrypoint", "bash", "-v"])
        .arg(format!("{}:/case", case_dir.display()))
        .args([&opts.image, "/case/Allrun"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start docker")?;
    let mut guard = Kill { docker: opts.docker.clone(), env: opts.env.clone(), name: name.clone(), armed: true };

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = vec![];
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
    {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let log_path = case_dir.join("simlab.log");
    let mut log = std::io::BufWriter::new(std::fs::File::create(&log_path)?);
    let mut st = LogState::default();
    let mut mon = Monitor::new(p);
    mon.window = opts.average_last.max(10);
    let mut stop_sent = false;
    let mut snappy_steps = 0u32;
    let spin = p.rotor.clone();
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                let _ = writeln!(log, "{line}");
                for ev in st.feed(&line) {
                    match ev {
                        Event::Stage(s) => {
                            let (f, msg) = stage_words(&s);
                            progress.report(f, msg);
                        }
                        Event::Meshing(m) => {
                            snappy_steps += 1;
                            let f = 0.05 + 0.25 * (1.0 - (-(snappy_steps as f32) / 25.0).exp());
                            progress.report(f, &format!("Meshing (snappyHexMesh): {m}"));
                        }
                        Event::Iteration { it, force, moment, residuals } => {
                            mon.push(force, moment);
                            let step = it as u64;
                            let mut ru: f64 = 0.0;
                            for (k, r) in &residuals {
                                if k.starts_with('U') {
                                    ru = ru.max(*r);
                                } else {
                                    progress.metric(&format!("residual_{k}"), step, *r);
                                }
                            }
                            if ru > 0.0 {
                                progress.metric("residual_U", step, ru);
                            }
                            let headline = if let Some(r) = &spin {
                                let thrust = -dot(force, r.jet_dir);
                                let torque = -dot(moment, r.axis);
                                progress.metric("thrust_n", step, thrust);
                                progress.metric("torque_nm", step, torque);
                                format!("thrust {:.3} N ({:.1} g), torque {:.2} mN·m", thrust, thrust / 9.80665e-3, torque * 1e3)
                            } else {
                                let d = dot(force, p.drag_dir);
                                progress.metric("drag_n", step, d);
                                progress.metric("lift_n", step, dot(force, p.lift_dir));
                                progress.metric("side_n", step, dot(force, p.side_dir));
                                format!("drag {:.4e} N", d)
                            };
                            let f = 0.35 + 0.6 * (it as f32 / p.max_iterations as f32).min(1.0);
                            progress.report(f, &format!("Solving: iteration {it} of at most {} · {headline}", p.max_iterations));
                            if opts.early_stop && !stop_sent && it >= p.min_iterations && mon.settled(it).is_some() {
                                // ask simpleFoam to write and stop (runTimeModifiable)
                                std::fs::write(case_dir.join("system/controlDict"), case::control_dict_file(p, true))?;
                                stop_sent = true;
                                progress.report(f, &format!("Forces settled at iteration {it}; finishing"));
                            }
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if progress.cancelled() {
            let _ = log.flush();
            guard.kill();
            let _ = child.wait();
            return Err(anyhow::Error::new(Cancelled));
        }
    }
    let _ = log.flush();
    let status = child.wait()?;
    guard.armed = false;
    for r in readers {
        let _ = r.join();
    }
    if progress.cancelled() {
        return Err(anyhow::Error::new(Cancelled));
    }
    if !status.success() || st.stage != "done" {
        let mut why = st.fatal.clone().unwrap_or_else(|| st.tail.iter().rev().take(6).rev().cloned().collect::<Vec<_>>().join(" | "));
        if st.stage.is_empty() && why.contains("/case/Allrun") {
            why = format!(
                "Docker cannot see the case folder {} (Colima shares only your home folder by default)",
                case_dir.display()
            );
        }
        bail!("OpenFOAM stopped during '{}': {} (full log: {})", st.stage, why, log_path.display());
    }
    progress.report(0.97, "Reading the results");

    // ---- forces
    if mon.forces.is_empty() {
        bail!("OpenFOAM finished but printed no forces (log: {})", log_path.display());
    }
    let window = mon.window.min(mon.forces.len());
    let n = mon.forces.len();
    let force = post::mean3(&mon.forces[n - window..]);
    let moment = post::mean3(&mon.moments[n - window..]);
    let spread = mon.spread(window);
    let converged = st.residuals_converged || stop_sent || spread <= mon.tol;
    let mut warnings = early_warnings;
    warnings.extend(p.warnings.iter().cloned());
    if !converged {
        warnings.push(format!(
            "The forces had not settled after {} iterations: they still move by ±{:.1} % over the last {window}. The numbers are averages over those iterations.",
            st.iteration,
            spread * 100.0
        ));
    }
    if let Some(nf) = st.mesh_failed_checks {
        warnings.push(format!(
            "OpenFOAM's mesh check flagged {nf} issue(s) ({}). Usually harmless in a few cells near sharp edges.",
            if st.mesh_notes.is_empty() { "see log.checkMesh".to_string() } else { st.mesh_notes.join("; ") }
        ));
    }
    if p.layers > 0
        && let Some(c) = st.layer_coverage
            && c < 50.0 {
                warnings.push(format!("Only {c:.0} % of the planned boundary layer cells could be added on the part; drag from skin friction is less certain."));
            }

    let (thrust_n, torque_nm, power_w, wake_dir) = if let Some(r) = &spin {
        let thrust = -dot(force, r.jet_dir);
        let torque = -dot(moment, r.axis);
        if thrust < 0.0 {
            warnings.push("The part pushed air the opposite way from what its shape suggested, so the wake's fine cells were on the wrong side; treat thrust as rough.".into());
        }
        (Some(thrust), Some(torque), Some(torque * r.omega), Some(r.jet_dir))
    } else {
        (None, None, None, None)
    };
    let drag = dot(force, p.drag_dir);
    let cd = (p.speed > 0.0 && p.ref_area > 0.0).then(|| drag / (0.5 * p.rho * p.speed * p.speed * p.ref_area));

    // ---- samples
    let mut surface_pressure = vec![f32::NAN; surface.positions.len()];
    let mut slices = prep.slices.clone();
    match post::latest_time_dir(&case_dir, "probes") {
        Some(dir) => {
            let faces = post::read_part_faces(&case_dir);
            match post::read_cloud(&dir, "vertices", &prep.probe_points) {
                Ok((pv, _)) => {
                    let found = pv.iter().filter(|x| x.is_some()).count();
                    let (sp, filled) = post::surface_pressure(surface, &pv, &faces, p.rho);
                    surface_pressure = sp;
                    if found < pv.len() * 9 / 10 {
                        warnings.push(format!(
                            "Surface pressure: {} of {} points were filled in from nearby values.",
                            filled,
                            pv.len()
                        ));
                    }
                }
                Err(e) => warnings.push(format!("No surface pressure: {e}")),
            }
            for (s, pts) in slices.iter_mut().zip(&prep.slice_points) {
                match post::read_cloud(&dir, &s.name, pts) {
                    Ok((pv, uv)) => post::fill_slice(s, &pv, &uv, p.rho),
                    Err(e) => warnings.push(format!("No {} slice: {e}", s.name)),
                }
            }
        }
        None => warnings.push("OpenFOAM wrote no samples, so there is no surface pressure or flow slices.".into()),
    }
    slices.retain(|s| !s.fields.is_empty());

    let y_plus = st.y_plus.map(|(a, b, c)| [a, c, b]);
    progress.report(1.0, "Done");
    Ok(WindReference {
        force_n: force,
        moment_nm: moment,
        moment_center_m: p.cof_r,
        drag_n: drag,
        lift_n: dot(force, p.lift_dir),
        side_n: dot(force, p.side_dir),
        cd,
        ref_area_m2: p.ref_area,
        reynolds: p.reynolds,
        thrust_n,
        torque_nm,
        power_w,
        wake_dir,
        cells: st.cells.unwrap_or(0),
        iterations: st.iteration,
        averaged_over: window as u32,
        seconds: started.elapsed().as_secs_f64(),
        converged,
        turbulence: p.turbulence_name().into(),
        y_plus,
        warnings,
        surface_pressure,
        slices,
        case_dir: case_dir.clone(),
        log_path,
    })
}

/// Kills reference containers left over from an engine that quit without
/// stopping its run (call once at engine start). Returns how many.
pub fn kill_stale_containers(opts: &RunOptions) -> usize {
    let Ok(out) = Command::new(&opts.docker)
        .envs(opts.env.iter().cloned())
        .args(["ps", "-q", "--filter", "label=simlab=reference"])
        .output()
    else {
        return 0;
    };
    let ids: Vec<String> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from).collect();
    if !ids.is_empty() {
        let _ = Command::new(&opts.docker).envs(opts.env.iter().cloned()).arg("kill").args(&ids).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    ids.len()
}

/// Cores the Docker VM has (Colima: `--cpu`).
pub fn docker_cpus(opts: &RunOptions) -> Option<usize> {
    let out = Command::new(&opts.docker).envs(opts.env.iter().cloned()).args(["info", "--format", "{{.NCPU}}"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn stage_words(s: &str) -> (f32, &'static str) {
    match s {
        "feature" => (0.02, "Finding the part's sharp edges"),
        "blockMesh" => (0.03, "Building the background grid"),
        "decomposeMesh" => (0.04, "Splitting the grid across cores"),
        "snappy" => (0.05, "Meshing around the part (snappyHexMesh)"),
        "reconstructMesh" => (0.30, "Joining the mesh"),
        "checkMesh" => (0.31, "Checking the mesh"),
        "decompose" => (0.33, "Splitting the case across cores"),
        "solve" => (0.35, "Solving the airflow (simpleFoam)"),
        "sample" => (0.95, "Sampling pressure and flow slices"),
        "reconstruct" => (0.96, "Gathering the results"),
        "done" => (0.97, "OpenFOAM finished"),
        _ => (0.0, "Working"),
    }
}

/// Watches the force history and decides when it has settled.
struct Monitor {
    forces: Vec<Vec3>,
    moments: Vec<Vec3>,
    window: usize,
    tol: f64,
    use_moment: bool,
}
impl Monitor {
    fn new(p: &Plan) -> Self {
        Monitor { forces: vec![], moments: vec![], window: 200, tol: 0.002, use_moment: p.rotor.is_some() }
    }
    fn push(&mut self, f: Vec3, m: Vec3) {
        self.forces.push(f);
        self.moments.push(m);
    }
    /// largest deviation from the mean over the last `w` samples, relative to
    /// the mean's size (force, and moment for spinning parts)
    fn spread(&self, w: usize) -> f64 {
        let n = self.forces.len();
        if n == 0 {
            return f64::INFINITY;
        }
        let w = w.min(n).max(1);
        let rel = |v: &[Vec3]| {
            let m = post::mean3(v);
            let s = norm(m).max(1e-30);
            v.iter().map(|x| norm(sub(*x, m)) / s).fold(0.0, f64::max)
        };
        let mut s = rel(&self.forces[n - w..]);
        if self.use_moment {
            s = s.max(rel(&self.moments[n - w..]));
        }
        s
    }
    fn settled(&self, _it: u32) -> Option<f64> {
        if self.forces.len() < self.window {
            return None;
        }
        let s = self.spread(self.window);
        (s <= self.tol).then_some(s)
    }
}

/// Kills the container if the run ends without it exiting by itself.
struct Kill {
    docker: PathBuf,
    env: Vec<(String, String)>,
    name: String,
    armed: bool,
}
impl Kill {
    fn kill(&mut self) {
        if self.armed {
            let _ = Command::new(&self.docker)
                .envs(self.env.iter().cloned())
                .args(["kill", &self.name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            self.armed = false;
        }
    }
}
impl Drop for Kill {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Reads an STL, for the example and tests: millimetres → metres.
pub fn read_stl_mm(path: &Path) -> Result<TriMesh> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(simlab_core::stl::read_stl(&bytes)?.scaled(0.001))
}

