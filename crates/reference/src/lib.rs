//! SimLab's reference wind tunnel: OpenFOAM v2606 (snappyHexMesh +
//! simpleFoam, MRF for spinning parts) run in the `simlab-reference:1`
//! Docker image. Slower than the built-in GPU solver, but a well-proven
//! answer to compare it against.
//!
//! **Which model** (plan::wind_method): spinning parts k-ω SST with MRF, as
//! in the user's propeller study. Still parts by Reynolds number U·L/ν:
//! laminar below 300, k-ω SST to 5000, and above that k-ω SST with the
//! Langtry–Menter transition model (kOmegaSSTLM), all steady. Above Re 300
//! the part gets 3 prism layers at every quality. A run length
//! (`WindConfig::duration`, flow passes) asks for a time-accurate run
//! instead: the same model solved steady for 300 iterations, then
//! pimpleFoam (unsteady RANS, Courant 4) with the forces averaged once the
//! start-up is over; it stops when the average over the first and second
//! half of the averaging time agree to 1 %.
//!
//! ```text
//! run_openfoam(surface_m, cfg, model_unit_m, case_dir, threads, progress)
//!   1. plan        domain, cell sizes, refinement, MRF zone, BCs   (plan.rs)
//!   2. write case  dictionaries + Allrun + sample points           (case.rs)
//!   3. docker run  surfaceFeatureExtract → blockMesh → snappyHexMesh
//!                  → checkMesh → decomposePar → simpleFoam (mpirun)
//!                  [time-accurate: → pimpleFoam from the steady result]
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
//! **Bluff bodies above Re 1000** (November 2026, Colima 12–16 cores; drag
//! coefficient on frontal area; "Draft" = the Draft mesh with its 3 prism
//! layers, about 400 k cells for a 20 mm sphere). References: sphere,
//! Clift–Grace–Weber 1978 (Re 10⁵: 0.47, the correlations spread 0.43–0.50);
//! square plate square to the wind 1.17 (Hoerner); cylinder 1.15–1.2,
//! Strouhal 0.20–0.21. The cylinder is quasi-2D (3.2 D span between slip
//! planes, 5 % blockage, which reads about 3 % high).
//!
//! | case | ref | k-ω SST (before) | SST + transition (now) | time-accurate LES / IDDES |
//! |---|---|---|---|---|
//! | sphere Re 1000, Draft | 0.471 | 0.478 (+1.5 %) | 0.488 (+3.6 %) | |
//! | sphere Re 10⁴, Draft | 0.405 | 0.398 (−1.8 %) | 0.408 (+0.7 %) | |
//! | sphere Re 10⁴, Normal (1.6 M) | 0.405 | | 0.374 (−8 %), 34 min on 8 cores | |
//! | sphere Re 2·10⁴, Draft | 0.441 | 0.380 (−14 %) | 0.402 (−9 %) | LES 0.48 (+9 %), IDDES ≈0.50 (+13 %) |
//! | sphere Re 2·10⁴, Normal (1.6 M) | 0.441 | 0.361 (−18 %) | 0.373 (−15 %), 28 min on 8 cores | |
//! | sphere Re 2·10⁴, Fine (10.3 M) | 0.441 | | not finished: ≈10 s per iteration on 8 cores, 4–8 h | |
//! | sphere Re 10⁵, Draft | 0.47 | 0.287 (−39 %) | 0.348 (−26 %) | |
//! | square plate Re 2·10⁴, Draft | 1.17 | 1.240 (+6 %) | 1.240 (+6 %) | LES 0.80 (−31 %) |
//! | cylinder Re 2·10⁴, Draft | 1.15–1.2 | 1.14 ±3 % | 1.21 ±2.5 % | unsteady RANS (transition): 1.25, St 0.220 |
//!
//! Before this, Draft had no prism layers above Re 300: SST read 0.305 at
//! Re 2·10⁴. Also measured at sphere Re 2·10⁴ and rejected: unsteady RANS (SST or
//! transition) settles to the steady answer (0.378 / 0.398, no shedding);
//! LES (WALE) with a finer wake (1.2 M cells) 0.50–0.53; prism layers
//! sized for y+ ≈ 1 moved the transition model to 0.409 at Re 2·10⁴ but to
//! 0.324 at 10⁵; a domain 6 L to the sides moved the plate by −0.6 %. LES
//! that would converge here needs several million cells and hours per run.
//! So rounded parts above Re ≈ 5000 read low, more so on finer meshes (Re
//! 10⁴: +0.7 % Draft but −8 % Normal; 2·10⁴: −9 % / −15 %; 10⁵: −26 %
//! Draft), and the engine's accuracy note says so. Sharp-edged and
//! cylindrical parts, where the edges or the shedding fix the separation,
//! stay within about 6 %. The spinning-part method is unchanged (E-v5,
//! 45,800 rpm, Normal: 345.6 g, 22.01 mN·m after the change, 345.8 g /
//! 22.06 before).
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
    /// solver iterations (steady) or time steps (time-accurate) in the run proper
    pub iterations: u32,
    /// how many of them the forces are averaged over
    pub averaged_over: u32,
    /// "steady (simpleFoam)" or "time-accurate (pimpleFoam), forces averaged over time"
    pub solver: String,
    /// time-accurate runs: the averaging time in flow passes over the part (L / U)
    pub averaged_flow_passes: Option<f64>,
    /// time-accurate runs: standard deviation of the drag over the averaging time (N),
    /// how much it swings as the wake sheds
    pub drag_fluctuation: Option<f64>,
    pub seconds: f64,
    pub converged: bool,
    pub turbulence: String,
    /// y+ on the part (min, average, max), from OpenFOAM's yPlus function object
    pub y_plus: Option<[f64; 3]>,
    /// still parts: the thinner size across the wind (mm) and the surface
    /// mesh cells across it (spinning parts: 0)
    #[serde(default)]
    pub thin_mm: f64,
    #[serde(default)]
    pub cells_thin: f64,
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
    /// which engine started the container: its containers carry the label
    /// `simlab.owner=<owner>` as well as `simlab=reference`, so an engine
    /// cleans up only its own (see [`kill_stale_containers`] and [`owner_id`])
    pub owner: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            docker: find_docker().unwrap_or_else(|| "docker".into()),
            image: IMAGE.into(),
            env: docker_env(),
            early_stop: true,
            average_last: 200,
            owner: None,
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
    std::fs::write(case_dir.join("system/sampleDict"), case::sample_dict_for(&names, plan.transient.is_some()))?;
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
        .args(["run", "--rm", "--init", "--name", &name])
        .args(container_labels(opts))
        .args(["--shm-size", "1g", "--entrypoint", "bash", "-v"])
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
                            let (f, msg) = stage_words(&s, p.transient.is_some());
                            progress.report(f, msg);
                        }
                        Event::Meshing(m) => {
                            snappy_steps += 1;
                            let f = 0.05 + 0.25 * (1.0 - (-(snappy_steps as f32) / 25.0).exp());
                            progress.report(f, &format!("Meshing (snappyHexMesh): {m}"));
                        }
                        Event::Iteration { it, time, force, moment, residuals } => {
                            // the steady start of a time-accurate run only shows progress
                            let init = st.stage == "init";
                            if !init {
                                mon.push(time, force, moment);
                            }
                            let step = mon.forces.len().max(1) as u64;
                            let mut ru: f64 = 0.0;
                            for (k, r) in &residuals {
                                if k.starts_with('U') {
                                    ru = ru.max(*r);
                                } else if !init {
                                    progress.metric(&format!("residual_{k}"), step, *r);
                                }
                            }
                            if ru > 0.0 && !init {
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
                                if !init {
                                    progress.metric("drag_n", step, d);
                                    progress.metric("lift_n", step, dot(force, p.lift_dir));
                                    progress.metric("side_n", step, dot(force, p.side_dir));
                                }
                                format!("drag {:.4e} N", d)
                            };
                            let (f, msg) = match &p.transient {
                                None => (
                                    0.35 + 0.6 * (it as f32 / p.max_iterations as f32).min(1.0),
                                    format!("Solving: iteration {it} of at most {} · {headline}", p.max_iterations),
                                ),
                                Some(t) if init => (
                                    0.35 + 0.05 * (it as f32 / t.init_iterations.max(1) as f32).min(1.0),
                                    format!("Steady start: iteration {it} of {} · {headline}", t.init_iterations),
                                ),
                                Some(t) => (
                                    0.40 + 0.55 * (time / t.end_time).min(1.0) as f32,
                                    format!(
                                        "Solving in time: {:.1} of at most {:.0} flow passes over the part · {headline}",
                                        time / t.flow_time,
                                        t.end_time / t.flow_time
                                    ),
                                ),
                            };
                            progress.report(f, &msg);
                            let settled = match &p.transient {
                                None => it >= p.min_iterations && mon.settled(it).is_some(),
                                Some(t) => !init && time >= t.average_from + t.min_average && mon.average_settled(t.average_from).is_some(),
                            };
                            if opts.early_stop && !stop_sent && settled {
                                // ask the solver to write and stop (runTimeModifiable)
                                std::fs::write(case_dir.join("system/controlDict"), case::control_dict_file(p, true))?;
                                stop_sent = true;
                                let when = match &p.transient {
                                    None => format!("iteration {it}"),
                                    Some(t) => format!("{:.1} flow passes", time / t.flow_time),
                                };
                                progress.report(f, &format!("Forces settled at {when}; finishing"));
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
    let mut warnings = early_warnings;
    warnings.extend(p.warnings.iter().cloned());
    let n = mon.forces.len();
    let (force, moment, window, converged, averaged_time) = match &p.transient {
        None => {
            let mut window = mon.window.min(n);
            let spread = mon.spread(window);
            let converged = st.residuals_converged || stop_sent || spread <= mon.tol;
            if !converged {
                // a steady solve that keeps swinging (an unsteady wake): the
                // last half of the iterations averages over several swings,
                // where the last 200 may catch one side of one
                window = window.max(n / 2);
                warnings.push(format!(
                    "The forces had not settled after {} iterations: they still swing by ±{:.0} % (the wake is unsteady). The numbers are averages over the last {window} iterations; treat them as ±{:.0} %.",
                    st.iteration,
                    spread * 100.0,
                    (0.5 * spread * 100.0).max(1.0).ceil()
                ));
            }
            let force = post::mean3(&mon.forces[n - window..]);
            let moment = post::mean3(&mon.moments[n - window..]);
            (force, moment, window, converged, None)
        }
        Some(t) => {
            // a run cut short before its averaging time: average its second half
            let last = mon.times.last().copied().unwrap_or(0.0);
            let from = if last > t.average_from + 0.25 * t.min_average { t.average_from } else { 0.5 * last };
            let (force, moment, window) = mon.time_mean(from);
            let drift = mon.average_drift(from);
            let converged = stop_sent || drift <= mon.tol_average;
            if !converged {
                warnings.push(format!(
                    "The averaged forces had not settled after {:.0} flow passes over the part: the first and second half of the averaging time differ by {:.1} %. Treat the drag as ±{:.0} %.",
                    last / t.flow_time,
                    drift * 100.0,
                    (drift * 100.0).max(1.0).ceil()
                ));
            }
            (force, moment, window, converged, Some(last - from))
        }
    };
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
        iterations: n as u32,
        averaged_over: window as u32,
        solver: p.solver_name().into(),
        averaged_flow_passes: averaged_time.zip(p.transient.as_ref()).map(|(a, t)| a / t.flow_time),
        drag_fluctuation: averaged_time.map(|a| mon.drag_std(p.drag_dir, a)),
        seconds: started.elapsed().as_secs_f64(),
        converged,
        turbulence: p.turbulence_name().into(),
        y_plus,
        thin_mm: p.thin * 1e3,
        cells_thin: p.cells_thin,
        warnings,
        surface_pressure,
        slices,
        case_dir: case_dir.clone(),
        log_path,
    })
}

/// Every SimLab reference container (OpenFOAM, CalculiX) carries this label.
pub const LABEL: &str = "simlab=reference";
/// ...and this one, naming the engine that started it (see [`owner_id`]).
pub const OWNER_LABEL: &str = "simlab.owner";

/// A short, stable id for an engine's data folder: two engines with
/// different data folders (sharing one Colima) never touch each other's
/// containers, and an engine restarted on the same folder finds the ones
/// it left behind when it crashed.
pub fn owner_id(data_dir: &Path) -> String {
    let p = data_dir.canonicalize().unwrap_or_else(|_| data_dir.to_path_buf());
    // FNV-1a, 64 bit: stable across builds (unlike std's hasher)
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `docker run` arguments that label a container as SimLab's (and its owner's).
pub fn container_labels(opts: &RunOptions) -> Vec<String> {
    let mut a = vec!["--label".to_string(), LABEL.to_string()];
    if let Some(o) = &opts.owner {
        a.push("--label".into());
        a.push(format!("{OWNER_LABEL}={o}"));
    }
    a
}

/// `docker ps` filters for the containers [`kill_stale_containers`] stops:
/// with an owner, only that owner's; without, every SimLab reference container.
pub fn stale_filters(opts: &RunOptions) -> Vec<String> {
    let mut f = vec!["--filter".to_string(), format!("label={LABEL}")];
    if let Some(o) = &opts.owner {
        f.push("--filter".into());
        f.push(format!("label={OWNER_LABEL}={o}"));
    }
    f
}

/// Kills reference containers left over from an engine that quit without
/// stopping its run (call at engine start and exit). With `opts.owner` set
/// only that engine's containers go: another engine sharing Colima keeps
/// its runs. Returns how many.
pub fn kill_stale_containers(opts: &RunOptions) -> usize {
    let Ok(out) = Command::new(&opts.docker).envs(opts.env.iter().cloned()).arg("ps").arg("-q").args(stale_filters(opts)).output() else {
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

fn stage_words(s: &str, transient: bool) -> (f32, &'static str) {
    match s {
        "init" => (0.35, "Steady start (simpleFoam)"),
        "solve" if transient => (0.40, "Following the airflow in time (pimpleFoam)"),
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

/// Watches the force history and decides when it has settled: steady runs
/// by the spread over the last iterations, time-accurate runs by whether
/// the running average has stopped moving.
struct Monitor {
    times: Vec<f64>,
    forces: Vec<Vec3>,
    moments: Vec<Vec3>,
    window: usize,
    tol: f64,
    /// time-accurate: the two halves of the averaging time agree to this
    tol_average: f64,
    use_moment: bool,
}
impl Monitor {
    fn new(p: &Plan) -> Self {
        Monitor { times: vec![], forces: vec![], moments: vec![], window: 200, tol: 0.002, tol_average: TRANSIENT_TOL, use_moment: p.rotor.is_some() }
    }
    fn push(&mut self, t: f64, f: Vec3, m: Vec3) {
        self.times.push(t);
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
    /// time-weighted mean force and moment over [from, now] (each sample
    /// stands for the step that ended at it); also how many samples
    fn time_mean(&self, from: f64) -> (Vec3, Vec3, usize) {
        time_mean(&self.times, &self.forces, &self.moments, from, f64::INFINITY)
    }
    /// relative difference between the mean force over the first and the
    /// second half of [from, now]: small once the average has settled
    fn average_drift(&self, from: f64) -> f64 {
        let Some(&end) = self.times.last() else { return f64::INFINITY };
        if end <= from {
            return f64::INFINITY;
        }
        let mid = 0.5 * (from + end);
        let (a, _, na) = time_mean(&self.times, &self.forces, &self.moments, from, mid);
        let (b, _, nb) = time_mean(&self.times, &self.forces, &self.moments, mid, end);
        if na < 10 || nb < 10 {
            return f64::INFINITY;
        }
        let (all, _, _) = self.time_mean(from);
        norm(sub(a, b)) / norm(all).max(1e-30)
    }
    fn average_settled(&self, from: f64) -> Option<f64> {
        let d = self.average_drift(from);
        (d <= self.tol_average).then_some(d)
    }
    /// standard deviation of the drag (N) over the last `span` seconds
    fn drag_std(&self, dir: Vec3, span: f64) -> f64 {
        let Some(&end) = self.times.last() else { return 0.0 };
        let d: Vec<f64> = self.times.iter().zip(&self.forces).filter(|(t, _)| **t >= end - span).map(|(_, f)| dot(*f, dir)).collect();
        if d.len() < 2 {
            return 0.0;
        }
        let m = d.iter().sum::<f64>() / d.len() as f64;
        (d.iter().map(|x| (x - m).powi(2)).sum::<f64>() / d.len() as f64).sqrt()
    }
}

/// Time-accurate runs stop once the mean force over the first and second
/// half of the averaging time agree to 1 %.
pub const TRANSIENT_TOL: f64 = 0.01;

/// Mean of forces and moments over the samples with from < t <= to,
/// weighted by each sample's time step.
fn time_mean(times: &[f64], forces: &[Vec3], moments: &[Vec3], from: f64, to: f64) -> (Vec3, Vec3, usize) {
    let (mut f, mut m, mut w, mut n) = ([0.0; 3], [0.0; 3], 0.0, 0usize);
    for i in 0..times.len() {
        let t = times[i];
        if t <= from || t > to {
            continue;
        }
        let t0 = if i > 0 { times[i - 1].max(from) } else { from };
        let dt = (t - t0).max(0.0);
        f = simlab_core::add(f, simlab_core::scale(forces[i], dt));
        m = simlab_core::add(m, simlab_core::scale(moments[i], dt));
        w += dt;
        n += 1;
    }
    if w <= 0.0 {
        return (forces.last().copied().unwrap_or([0.0; 3]), moments.last().copied().unwrap_or([0.0; 3]), 0);
    }
    (simlab_core::scale(f, 1.0 / w), simlab_core::scale(m, 1.0 / w), n)
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_are_labelled_with_their_engine() {
        let a = owner_id(Path::new("/tmp/simlab-a"));
        let b = owner_id(Path::new("/tmp/simlab-b"));
        assert_ne!(a, b, "two data folders, two owners");
        assert_eq!(a, owner_id(Path::new("/tmp/simlab-a")), "stable for the same folder");
        assert_eq!(a.len(), 16);
        let mut o = RunOptions { owner: Some(a.clone()), ..RunOptions::default() };
        assert_eq!(container_labels(&o), ["--label", LABEL, "--label", &format!("simlab.owner={a}")]);
        // cleanup only matches this owner's containers
        assert_eq!(stale_filters(&o), ["--filter", "label=simlab=reference", "--filter", &format!("label=simlab.owner={a}")]);
        o.owner = None;
        assert_eq!(container_labels(&o), ["--label", LABEL]);
        assert_eq!(stale_filters(&o), ["--filter", "label=simlab=reference"]);
    }

    #[test]
    fn time_average_weights_each_step_by_its_length() {
        let t = [1.0, 2.0, 4.0, 5.0];
        let f: Vec<Vec3> = [1.0, 2.0, 3.0, 5.0].iter().map(|x| [*x, 0.0, 0.0]).collect();
        let m = vec![[0.0; 3]; 4];
        // from 2: the step to 4 (2 s long, force 3) and to 5 (1 s, force 5)
        let (fm, _, n) = time_mean(&t, &f, &m, 2.0, f64::INFINITY);
        assert_eq!(n, 2);
        assert!((fm[0] - (3.0 * 2.0 + 5.0) / 3.0).abs() < 1e-12);
        // a window starting inside a step counts only its part
        let (fm, _, _) = time_mean(&t, &f, &m, 3.0, 5.0);
        assert!((fm[0] - (3.0 + 5.0) / 2.0).abs() < 1e-12);
    }

    #[test]
    fn a_settled_average_lets_a_time_accurate_run_stop() {
        let p = plan::plan(&geom::icosphere([0.0; 3], 0.01, 2), &simlab_core::WindConfig {
            wind_dir: [1.0, 0.0, 0.0],
            speed_m_s: 15.0,
            air: Default::default(),
            spin: None,
            quality: simlab_core::GridQuality::Draft,
            duration: None,
        }, 0.001)
        .unwrap();
        let mut mon = Monitor::new(&p);
        // a drag that swings ±10 % about 1 N, period 1 s, for 40 s
        for i in 1..=4000 {
            let t = i as f64 * 0.01;
            mon.push(t, [1.0 + 0.1 * (std::f64::consts::TAU * t).sin(), 0.0, 0.0], [0.0; 3]);
        }
        assert!(mon.average_drift(10.0) < 0.005);
        assert!(mon.average_settled(10.0).is_some());
        let (f, _, _) = mon.time_mean(10.0);
        assert!((f[0] - 1.0).abs() < 1e-3);
        assert!((mon.drag_std([1.0, 0.0, 0.0], 30.0) - 0.1 / 2f64.sqrt()).abs() < 2e-3);
        // a drag still climbing has not settled
        let mut mon = Monitor::new(&p);
        for i in 1..=4000 {
            let t = i as f64 * 0.01;
            mon.push(t, [1.0 + 0.01 * t, 0.0, 0.0], [0.0; 3]);
        }
        assert!(mon.average_settled(10.0).is_none());
    }
}
