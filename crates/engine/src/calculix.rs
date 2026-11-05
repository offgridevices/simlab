//! The strength double-check: the same mesh, material, supports and loads as
//! SimLab's own run, solved by CalculiX 2.21 in the `simlab-reference:1`
//! Docker image. The deck goes in on stdin and the results come back on
//! stdout, so no folder has to be shared with Docker.
//!
//! A held part that spins is solved geometrically nonlinear (`NLGEOM`), so
//! it stiffens under its own stress the way SimLab's stress-stiffening
//! passes do; everything else is one linear step. The headline numbers come
//! from CalculiX's own nodal stresses, with the same rules as SimLab (the
//! rigidly held faces left out of the peak). The summary compares them with
//! the latest SimLab run of the same test.

use crate::db::{ModelRow, RunRow};
use crate::jobs::{Ctx, Outcome};
use crate::state::App;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use simlab_core::{Cancelled, Progress, StrengthConfig};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn run(ctx: &Ctx, r: &RunRow, model: &ModelRow, c: &StrengthConfig) -> Result<Outcome> {
    if !crate::system::reference_ready() {
        bail!("{}", crate::system::DOCKER_NEEDED);
    }
    let material = crate::solve::run_material(r)?;
    let (mesh, h, mesh_seconds) = crate::solve::strength_mesh(ctx, model, c)?;
    let tractions = crate::solve::prop_tractions(ctx, model, &mesh, c)?;

    ctx.phase("running", 0.25, 0.97);
    ctx.report(0.0, "Setting up loads and supports");
    let input = simlab_fea::StrengthInput { mesh: &mesh, material: &material, config: c, model_unit_m: 0.001, face_tractions: tractions.as_deref() };
    let prep = simlab_fea::prepare(&input)?;
    let opts = simlab_fea::SolveOptions { memory_budget_bytes: ctx.app.settings().budget_bytes(), ..Default::default() };
    let nlgeom = simlab_fea::stiffening_applies(&prep, &opts);
    let mut deck = Vec::with_capacity(mesh.nodes.len() * 120);
    let iopts = simlab_fea::calculix::InpOptions { heading: format!("SimLab double-check of run {}", r.id), nlgeom, ..Default::default() };
    simlab_fea::calculix::write_inp(&mut deck, &mesh, &prep, &iopts)?;
    ctx.log(&format!("CalculiX deck: {} nodes, {} elements, {:.1} MB{}", mesh.nodes.len(), mesh.tets.len(), deck.len() as f64 / 1e6, if nlgeom { ", geometrically nonlinear" } else { "" }));

    let t0 = Instant::now();
    let threads = ctx.app.settings().cpu_threads.max(1);
    let frd_text = run_ccx(ctx, &deck, threads)?;
    let seconds = t0.elapsed().as_secs_f64();
    ctx.report(0.9, "Reading CalculiX's results");
    let frd = simlab_fea::calculix::read_frd(&frd_text).context("could not read CalculiX's results")?;
    drop(frd_text);
    let n = mesh.nodes.len();
    let disp = frd.displacements(n)?;
    let stress = frd.stresses(n)?;
    let missing = disp.iter().filter(|d| d[0].is_nan()).count();
    if missing > 0 {
        ctx.log(&format!("CalculiX gave no displacement for {missing} nodes"));
    }
    let u: Vec<f64> = disp.iter().flat_map(|d| if d[0].is_nan() { [0.0; 3] } else { *d }).collect();
    let report = simlab_fea::SolverReport {
        method: "CalculiX 2.21".into(),
        detail: format!("SPOOLES direct solver{}", if nlgeom { ", geometrically nonlinear (stiffens under its own stress)" } else { ", linear" }),
        dofs: 3 * n,
        solve_seconds: seconds,
        ..Default::default()
    };
    let prestress = nlgeom.then(|| u.clone());
    let mut res = simlab_fea::postprocess_reference(&input, prep, u, report, prestress, stress)?;
    res.timings.total_s = seconds;
    let mut out = crate::solve::strength_outcome(ctx, &mesh, &res, h, mesh_seconds, "strength-reference")?;
    out.summary["engine"] = json!("reference");
    out.summary["method"] = json!("CalculiX check");
    out.summary["nonlinear"] = json!(nlgeom);
    // the comparison goes beside the verdict (the UI's check card, MCP's get_run)
    if let Some(cmp) = compare_with_native(ctx.app, r, &out.summary) {
        out.summary["check"] = cmp;
    }
    Ok(out)
}

/// The latest finished SimLab run of the same test with the same setup,
/// next to this CalculiX result.
fn compare_with_native(app: &App, r: &RunRow, ours: &Value) -> Option<Value> {
    let runs = app.db().runs(&r.study_id).ok()?;
    let native = runs.iter().rev().find(|x| x.engine == "native" && x.status == "done" && x.config == r.config && x.material == r.material)?;
    let s = native.summary.as_ref()?;
    let pct = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) if b.abs() > 0.0 => Some(100.0 * (a - b) / b),
        _ => None,
    };
    let get = |v: &Value, k: &str| v[k].as_f64();
    let d_vm = pct(get(ours, "max_von_mises_mpa"), get(s, "max_von_mises_mpa"));
    let d_sf = pct(get(ours, "safety_factor"), get(s, "safety_factor"));
    let d_u = pct(get(ours, "max_displacement_mm"), get(s, "max_displacement_mm"));
    let worst = [d_vm, d_sf, d_u].iter().flatten().fold(0.0f64, |m, d| m.max(d.abs()));
    let text = match d_vm {
        Some(d) if worst <= 2.0 => format!("CalculiX agrees with SimLab: peak stress {:+.1}%, safety factor {:+.1}%.", d, d_sf.unwrap_or(0.0)),
        Some(d) => format!("CalculiX differs from SimLab: peak stress {:+.1}%, safety factor {:+.1}%, movement {:+.1}%.", d, d_sf.unwrap_or(0.0), d_u.unwrap_or(0.0)),
        None => return None,
    };
    Some(json!({
        "native_run_id": native.id,
        "text": text,
        "agrees": worst <= 2.0,
        "native": {"max_von_mises_mpa": get(s, "max_von_mises_mpa"), "safety_factor": get(s, "safety_factor"), "max_displacement_mm": get(s, "max_displacement_mm"), "elements": get(s, "elements")},
        "calculix": {"max_von_mises_mpa": get(ours, "max_von_mises_mpa"), "safety_factor": get(ours, "safety_factor"), "max_displacement_mm": get(ours, "max_displacement_mm"), "elements": get(ours, "elements")},
        "diff_pct": {"max_von_mises": d_vm, "safety_factor": d_sf, "max_displacement": d_u},
    }))
}

/// Runs `ccx` in the reference image with the deck on stdin; returns the
/// `.frd` text. CalculiX's own output streams into the run log and the
/// progress line. Stopping the run kills the container.
fn run_ccx(ctx: &Ctx, deck: &[u8], threads: usize) -> Result<String> {
    let o = crate::system::reference_options();
    let name = format!("simlab-ccx-{}-{}", std::process::id(), ctx.run_id);
    let t = threads.to_string();
    let script = "cd /tmp && cat > job.inp && ccx -i job 2>&1 | tee ccx.log 1>&2; \
                  if [ ! -s job.frd ] || grep -q 'ERROR' ccx.log; then exit 3; fi; cat job.frd";
    let mut child = Command::new(&o.docker)
        .envs(o.env.iter().cloned())
        .args(["run", "--rm", "-i", "--init", "--name", &name, "--label", "simlab=reference"])
        .args(["-e", &format!("OMP_NUM_THREADS={t}"), "-e", &format!("CCX_NPROC_STIFFNESS={t}"), "-e", &format!("CCX_NPROC_EQUATION_SOLVER={t}")])
        .args(["--entrypoint", "sh", &o.image, "-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start docker")?;
    let mut guard = Kill { docker: o.docker.clone(), env: o.env.clone(), name, armed: true };

    let mut stdin = child.stdin.take().context("no stdin")?;
    let deck = deck.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&deck);
    });
    let mut stdout = child.stdout.take().context("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let (tx, rx) = mpsc::channel::<String>();
    let stderr = child.stderr.take().context("no stderr")?;
    let errs = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut tail: Vec<String> = vec![];
    let mut iteration = 0u32;
    ctx.report(0.02, "CalculiX: reading the model");
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                let l = line.trim();
                if l.is_empty() {
                    continue;
                }
                ctx.log(&format!("ccx: {l}"));
                tail.push(l.to_string());
                if tail.len() > 12 {
                    tail.remove(0);
                }
                let low = l.to_ascii_lowercase();
                if low.starts_with("iteration") {
                    iteration = l.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(iteration + 1);
                    ctx.report((0.15 + 0.12 * iteration as f32).min(0.85), &format!("CalculiX: nonlinear pass {iteration}"));
                } else if low.contains("factoring the system of equations") {
                    ctx.report((0.1 + 0.12 * iteration as f32).min(0.85), "CalculiX: solving the equations");
                } else if low.contains("calculating the stiffness") || low.contains("determining the structure") {
                    ctx.report(0.05, "CalculiX: building the equations");
                } else if low.contains("calculating the stress") || low.contains("calculating the results") {
                    ctx.report(0.85, "CalculiX: working out stresses");
                } else if low.contains("convergence") {
                    ctx.log(&format!("ccx: {l}"));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if ctx.cancelled() {
            guard.kill();
            let _ = child.wait();
            return Err(Cancelled.into());
        }
    }
    let status = child.wait()?;
    guard.armed = false;
    let _ = writer.join();
    let _ = errs.join();
    let frd = reader.join().unwrap_or_default();
    if ctx.cancelled() {
        return Err(Cancelled.into());
    }
    if !status.success() || frd.is_empty() {
        let why: Vec<&String> = tail.iter().filter(|l| l.contains("ERROR") || l.contains("rror")).collect();
        let why = if why.is_empty() { tail.iter().rev().take(4).rev().cloned().collect::<Vec<_>>().join(" | ") } else { why.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" | ") };
        bail!("CalculiX did not finish: {why}");
    }
    Ok(frd)
}

/// Kills the container when the run ends early (stop, error, panic).
struct Kill {
    docker: std::path::PathBuf,
    env: Vec<(String, String)>,
    name: String,
    armed: bool,
}
impl Kill {
    fn kill(&mut self) {
        if self.armed {
            let _ = Command::new(&self.docker).envs(self.env.iter().cloned()).args(["kill", &self.name]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            self.armed = false;
        }
    }
}
impl Drop for Kill {
    fn drop(&mut self) {
        self.kill();
    }
}

/// CalculiX's time and memory on top of the shared meshing, from SimLab's
/// estimate of the same mesh. Measured on the E-v5 propeller at Normal
/// (557 k equations, CalculiX 2.21 with SPOOLES, 12 cores in Colima,
/// November 2026): one factorization ~15 s; the linear solve took 19 s, the
/// nonlinear spinning one 152 s (11 Newton passes); peak memory 3.9 GB.
pub fn estimate(app: &App, _m: &ModelRow, c: &StrengthConfig, native: Value) -> Result<Value> {
    let mut e = native.clone();
    let dofs = native["size"]["dofs"].as_f64().unwrap_or(0.0).max(1000.0);
    let scale = dofs / 557_000.0;
    // a held part that spins is solved geometrically nonlinear: ~11 factorizations
    let nonlinear = matches!(c.support, simlab_core::Support::Fixed { .. }) && c.loads.iter().any(|l| matches!(l, simlab_core::Load::Spin { .. }));
    let passes = if nonlinear { 11.0 } else { 1.0 };
    let solve = 5.0 + passes * 15.0 * scale.powf(1.5);
    let mem = (0.3e9 + 3.6e9 * scale.powf(1.3)) as u64;
    let (_, vm_mem) = crate::system::docker_vm();
    let budget = vm_mem.unwrap_or_else(|| app.settings().budget_bytes());
    let mesh_s = native["mesh_seconds"].as_f64().unwrap_or(0.0);
    e["engine"] = json!("reference");
    e["label"] = json!("CalculiX check");
    e["seconds"] = json!(mesh_s.max(0.0) + solve);
    e["memory_bytes"] = json!(mem);
    e["budget_bytes"] = json!(budget);
    e["fits"] = json!(mem <= budget);
    e["docker_ready"] = json!(crate::system::reference_ready());
    e["note"] = json!(if mem <= budget {
        format!("CalculiX in Docker on the same mesh as SimLab's run{}", if nonlinear { ", solved nonlinear (it spins)" } else { "" })
    } else {
        format!("Needs about {:.1} GB, more than Docker's {:.0} GB", mem as f64 / 1e9, budget as f64 / 1e9)
    });
    Ok(e)
}
