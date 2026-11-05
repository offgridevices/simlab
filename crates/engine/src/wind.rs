//! Wind tunnel runs. Two engines:
//!
//! * **Accurate** (`engine: "reference"`, the default): OpenFOAM v2606
//!   (snappyHexMesh + simpleFoam, k-ω SST, MRF for spinning parts) in the
//!   `simlab-reference:1` Docker image, through simlab-reference. Checked
//!   within 0.6 % of the user's earlier OpenFOAM propeller runs and 0.2–1.3 %
//!   of sphere drag at Re 100–1000.
//! * **Quick preview** (`engine: "native"`): the GPU lattice-Boltzmann
//!   tunnel (simlab-cfd). Minutes, but only a rough picture for spinning
//!   parts, and every result says so in its headline and accuracy note.
//!
//! Both write the same result files: the model surface as `mesh.*`, surface
//! pressure as `field.pressure.f32` (per surface vertex), and each flow
//! slice as `slice.<name>.json` + `slice.<name>.<field>.f32` (positions in
//! model mm, `NaN` inside the part).

use crate::db::{ModelRow, RunRow};
use crate::jobs::{Ctx, Outcome};
use crate::state::App;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use simlab_core::{GridQuality, Progress, TriMesh, Vec3, WindConfig, dot, norm, normalize, scale, sub};
use std::path::PathBuf;

pub const G: f64 = 9.80665;

/// The one-line accuracy notes. They lead the headline of every quick
/// preview, so nobody reads a preview's number as the answer.
pub const PREVIEW_NOTE_STILL: &str = "Drag within about 3% on simple shapes at low Reynolds number, less certain above.";
pub const PREVIEW_NOTE_SPIN: &str = "Rough picture only: thrust can be 30–60% off, use Accurate for numbers.";

/// What the run settles on before it starts: where the part spins about.
/// A config without `spin.center_mm` gets the hub the model analysis found
/// (`suggestions.prop.center_mm`), never the bounding-box centre: on a
/// 3-blade prop the box centre is several mm off the hub.
pub fn fill_spin_center(model: &ModelRow, config: &mut Value) {
    if config["kind"] != "wind" || !config["spin"].is_object() {
        return;
    }
    let given = config["spin"]["center_mm"].as_array().map(|a| a.len() == 3 && a.iter().all(|x| x.is_number())).unwrap_or(false);
    if given {
        return;
    }
    let hub = model.info["suggestions"]["prop"]["center_mm"].clone();
    let c = if hub.is_array() {
        hub
    } else {
        // no propeller found: the middle of the part is the only guess left
        let b = &model.info["bbox_mm"];
        let mid = |k: usize| (b["min"][k].as_f64().unwrap_or(0.0) + b["max"][k].as_f64().unwrap_or(0.0)) / 2.0;
        json!([mid(0), mid(1), mid(2)])
    };
    config["spin"]["center_mm"] = c;
    if !config["spin"]["axis"].is_array() {
        config["spin"]["axis"] = model.info["suggestions"]["prop"]["axis"].as_array().map(|a| json!(a)).unwrap_or(json!([0, 0, 1]));
    }
}

/// How far (mm, across the spin axis) the spin centre is from the hub the
/// model analysis found, when it found one.
fn hub_offset_mm(model: &ModelRow, cfg: &WindConfig) -> Option<f64> {
    let s = cfg.spin?;
    let h = model.info["suggestions"]["prop"]["center_mm"].as_array()?;
    let hub: Vec3 = [h.first()?.as_f64()?, h.get(1)?.as_f64()?, h.get(2)?.as_f64()?];
    let a = normalize(s.axis);
    let d = sub(s.center_mm, hub);
    Some(norm(sub(d, scale(a, dot(d, a)))))
}

fn spinning(cfg: &WindConfig) -> bool {
    cfg.spin.map(|s| s.rpm != 0.0 && norm(s.axis) > 0.0).unwrap_or(false)
}

/// Where OpenFOAM's case goes. Docker (Colima) only sees the home folder,
/// so the run's own folder when the data folder is under it; otherwise
/// SimLab's cache folder in ~/Library/Caches.
pub fn case_dir(app: &App, run_id: &str) -> PathBuf {
    let run = app.run_dir(run_id);
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let base = canon(&app.data);
    if !home.as_os_str().is_empty() && base.starts_with(canon(&home)) {
        run.join("openfoam")
    } else {
        home.join("Library/Caches/com.offgridevices.simlab/reference").join(run_id)
    }
}

pub fn accuracy(engine: &str, spin: bool) -> Value {
    match (engine, spin) {
        ("reference", true) => json!({
            "method": "OpenFOAM v2606: snappyHexMesh mesh around the part, steady RANS with the k-ω SST turbulence model, the spin as a moving reference frame (MRF)",
            "checked_against": "Your 2026 OpenFOAM propeller study: E-v5 at 45,800 rpm 345.8 g vs 346.0 g, P-v2 380.0 g vs 378.4 g, E-v5 at 17,000 rpm 45.7 g vs 45.8 g",
            "typical_error": "Thrust within about 3% and torque within about 7% (that is how much they move between mesh sizes); within 0.6% of your earlier OpenFOAM runs of the same props.",
        }),
        ("reference", false) => json!({
            "method": "OpenFOAM v2606: snappyHexMesh mesh around the part, steady RANS (k-ω SST, or laminar at low Reynolds number)",
            "checked_against": "Sphere drag at Reynolds number 100 and 1000 (Clift, Schiller–Naumann)",
            "typical_error": "Drag within about 1% on simple shapes (sphere 0.2–1.3%); sharp-edged parts in turbulent flow are less certain, a few percent.",
        }),
        (_, true) => json!({
            "method": "SimLab's GPU wind tunnel: lattice Boltzmann (D3Q19, LES), the part re-drawn on the grid as it turns",
            "checked_against": "OpenFOAM runs of E-v5 and P-v2 at 17,000–45,800 rpm (Normal and Fine: thrust 29–63% low); E-v5 at Draft: 86% low",
            "typical_error": format!("{PREVIEW_NOTE_SPIN} At Draft it can be far worse: E-v5 at 45,800 rpm read 50 g of thrust against OpenFOAM's 345 g. Torque can be 20–55% high, or double at Draft. Good for seeing the flow, not for the numbers."),
        }),
        (_, false) => json!({
            "method": "SimLab's GPU wind tunnel: lattice Boltzmann (D3Q19, LES) with interpolated walls",
            "checked_against": "Sphere drag from Reynolds number 20 to 10,000",
            "typical_error": format!("{PREVIEW_NOTE_STILL} Sphere: within about 3% up to Re 300, 8.5% low at Re 1000, 3% high at Re 10,000. Use Accurate for numbers you rely on."),
        }),
    }
}

// ------------------------------------------------------------------ runs

pub fn run(ctx: &Ctx, r: &RunRow, model: &ModelRow, cfg: &WindConfig) -> Result<Outcome> {
    let surf_mm = crate::models::load_surface_mm(ctx.app, model)?;
    let surf_m = surf_mm.scaled(0.001);
    let spin = spinning(cfg);
    let mut warnings: Vec<String> = vec![];
    if let Some(d) = hub_offset_mm(model, cfg).filter(|d| *d > 1.0) {
        warnings.push(format!(
            "The spin centre is {d:.1} mm from the hub SimLab found in the model. If the part really turns about its hub, set the spin centre there: a few mm off changes the result a lot."
        ));
    }
    let out = if r.engine == "reference" { openfoam(ctx, r, cfg, &surf_m, spin, warnings)? } else { gpu(ctx, cfg, &surf_m, spin, warnings)? };
    simlab_core::write_display_mesh(&ctx.dir, "mesh", &surf_mm)?;
    Ok(out)
}

fn openfoam(ctx: &Ctx, r: &RunRow, cfg: &WindConfig, surf_m: &TriMesh, spin: bool, mut warnings: Vec<String>) -> Result<Outcome> {
    if !crate::system::docker_running() || !crate::system::reference_image_present() {
        bail!("{}", crate::system::DOCKER_NEEDED);
    }
    let dir = case_dir(ctx.app, &r.id);
    let threads = ctx.app.settings().cpu_threads.max(1);
    ctx.log(&format!("OpenFOAM case in {} with up to {threads} cores", dir.display()));
    ctx.phase("running", 0.0, 0.98);
    let res = simlab_reference::run_openfoam(surf_m, cfg, 0.001, &dir, threads, ctx)?;
    ctx.phase("running", 0.98, 1.0);
    ctx.report(0.0, "Saving the results");
    ctx.log(&format!("OpenFOAM log: {}", res.log_path.display()));
    // the split-up copies of the case are as big as the case itself
    if let Ok(rd) = std::fs::read_dir(&res.case_dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("processor") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    let (fields, slices) = write_pressure_and_slices(
        ctx,
        &res.surface_pressure,
        None,
        res.slices.iter().map(|s| SliceOut {
            name: s.name.clone(),
            label: s.label.clone(),
            origin_mm: scale(s.origin_m, 1e3),
            u_mm: scale(s.u_m, 1e3),
            v_mm: scale(s.v_m, 1e3),
            nu: s.nu as u32,
            nv: s.nv as u32,
            fields: s.fields.iter().map(|f| (f.name.clone(), f.label.clone(), f.unit.clone(), f.values.as_slice())).collect(),
        }),
    )?;

    warnings.extend(res.warnings.iter().cloned());
    let rpm = cfg.spin.map(|s| s.rpm.abs()).unwrap_or(0.0);
    let (headline, short) = match (res.thrust_n, res.torque_nm, res.power_w) {
        (Some(t), Some(q), Some(p)) if spin => (
            format!("Pushes {} g ({} N) at {} rpm and needs {} W at the shaft (torque {} mN·m)", sig(t / G * 1e3), sig(t), crate::solve::group(rpm.round() as usize), sig(p), sig(q * 1e3)),
            format!("{} g at {} rpm", sig(t / G * 1e3), crate::solve::group(rpm.round() as usize)),
        ),
        _ => {
            let cd = res.cd.map(|c| format!(", drag coefficient {c:.3}")).unwrap_or_default();
            (format!("Drag {} N ({} g) at {} m/s{cd}", sig(res.drag_n), sig(res.drag_n / G * 1e3), sig(cfg.speed_m_s)), format!("Drag {} N", sig(res.drag_n)))
        }
    };
    let summary = json!({
        "kind": "wind",
        "engine": "reference",
        "method": "Accurate (OpenFOAM)",
        "headline": headline,
        "short": short,
        "force_n": res.force_n,
        "moment_nm": res.moment_nm,
        "moment_center_mm": scale(res.moment_center_m, 1e3),
        "drag_n": res.drag_n, "lift_n": res.lift_n, "side_n": res.side_n,
        "cd": res.cd,
        "ref_area_mm2": res.ref_area_m2 * 1e6,
        "thrust_n": res.thrust_n,
        "thrust_g": res.thrust_n.map(|t| t / G * 1e3),
        "torque_nm": res.torque_nm,
        "power_w": res.power_w,
        "rpm": cfg.spin.map(|s| s.rpm),
        "wake_dir": res.wake_dir,
        "reynolds": res.reynolds,
        "cells": res.cells,
        "steps": res.iterations,
        "iterations": res.iterations,
        "averaged_over": res.averaged_over,
        "seconds": res.seconds,
        "converged": res.converged,
        "turbulence": res.turbulence,
        "y_plus": res.y_plus,
        "warnings": warnings,
        "accuracy": accuracy("reference", spin),
        "case_dir": res.case_dir,
    });
    Ok(Outcome { summary, fields: json!(fields), slices: json!(slices) })
}

fn gpu(ctx: &Ctx, cfg: &WindConfig, surf_m: &TriMesh, spin: bool, mut warnings: Vec<String>) -> Result<Outcome> {
    ctx.phase("running", 0.0, 0.98);
    let opt = simlab_cfd::SolverOptions { memory_budget: Some(ctx.app.settings().budget_bytes()), ..Default::default() };
    let res = simlab_cfd::run_with(cfg, surf_m, 0.001, &opt, ctx)?;
    ctx.phase("running", 0.98, 1.0);
    ctx.report(0.0, "Saving the results");
    let (fields, slices) = write_pressure_and_slices(
        ctx,
        &res.surface_pressure_pa,
        Some(&res.surface_cp),
        res.slices.iter().map(|s| SliceOut {
            name: s.name.clone(),
            label: s.label.clone(),
            origin_mm: s.origin_mm,
            u_mm: s.u_mm,
            v_mm: s.v_mm,
            nu: s.nu,
            nv: s.nv,
            fields: s.fields.iter().map(|f| (f.name.clone(), f.label.clone(), f.unit.clone(), f.data.as_slice())).collect(),
        }),
    )?;
    let mut summary = serde_json::to_value(&res.summary)?;
    let note = if spin { PREVIEW_NOTE_SPIN } else { PREVIEW_NOTE_STILL };
    // the note leads the headline and the warnings: a preview's number must
    // never be read as the answer
    let r = &res.summary;
    let headline = match (r.thrust_n, r.torque_nm, r.power_w) {
        (Some(t), Some(q), Some(p)) if spin => format!(
            "Rough preview: about {} g of thrust at {} rpm, {} W, torque {} mN·m. Thrust can be 30–60% off.",
            sig(t.abs() / G * 1e3),
            crate::solve::group(cfg.spin.map(|s| s.rpm.abs()).unwrap_or(0.0).round() as usize),
            sig(p.abs()),
            sig(q.abs() * 1e3)
        ),
        _ => format!("Quick preview: drag about {} N at {} m/s{}", sig(r.drag_n), sig(cfg.speed_m_s), r.cd.map(|c| format!(", drag coefficient {c:.2}")).unwrap_or_default()),
    };
    summary["headline"] = json!(headline);
    summary["preview_note"] = json!(note);
    summary["short"] = json!(match r.thrust_n {
        Some(t) if spin => format!("Preview ~{} g", sig(t.abs() / G * 1e3)),
        _ => format!("Preview: drag {} N", sig(r.drag_n)),
    });
    summary["thrust_g"] = json!(res.summary.thrust_n.map(|t| t / G * 1e3));
    // the tunnel reports the air's moment on the part about +axis; the
    // summary's torque, like OpenFOAM's, is what the motor must supply
    // (positive for a driven propeller)
    if let (Some(q), Some(s)) = (r.torque_nm, cfg.spin) {
        summary["aero_moment_nm"] = json!(q);
        summary["torque_nm"] = json!(-q * s.rpm.signum());
    }
    summary["rpm"] = json!(cfg.spin.map(|s| s.rpm));
    summary["engine"] = json!("native");
    summary["method"] = json!("Quick preview (GPU)");
    warnings.insert(0, note.to_string());
    warnings.extend(res.summary.warnings.iter().cloned());
    summary["warnings"] = json!(warnings);
    summary["accuracy"] = accuracy("native", spin);
    Ok(Outcome { summary, fields: json!(fields), slices: json!(slices) })
}

/// A number with three significant digits, the way people read them:
/// 0.00000118 → "1.18e-6", 0.0758 → "0.0758", 345.3 → "345", 3.386 → "3.39".
pub fn sig(x: f64) -> String {
    if !x.is_finite() {
        return "–".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    let a = x.abs();
    if !(1e-3..1e5).contains(&a) {
        return format!("{x:.2e}");
    }
    let digits = (2 - a.log10().floor() as i32).max(0) as usize;
    format!("{x:.digits$}")
}

struct SliceOut<'a> {
    name: String,
    label: String,
    origin_mm: Vec3,
    u_mm: Vec3,
    v_mm: Vec3,
    nu: u32,
    nv: u32,
    /// (name, label, unit, values row-major in v then u)
    fields: Vec<(String, String, String, &'a [f32])>,
}

fn range(v: &[f32]) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &x in v {
        if x.is_finite() {
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    if lo > hi { (0.0, 0.0) } else { (lo, hi) }
}

/// Writes the surface pressure (and Cp) per model-surface vertex and every
/// flow slice; returns the run's `fields` and `slices` lists.
fn write_pressure_and_slices<'a>(ctx: &Ctx, pressure: &[f32], cp: Option<&[f32]>, slices: impl Iterator<Item = SliceOut<'a>>) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut fields = vec![];
    if pressure.iter().any(|x| x.is_finite()) {
        let (lo, hi) = range(pressure);
        simlab_core::write_le(&ctx.dir.join("field.pressure.f32"), pressure.iter().copied())?;
        // symmetric about zero, so white is ambient
        let m = lo.abs().max(hi.abs());
        fields.push(json!({"name": "pressure", "label": "Air pressure", "unit": "Pa", "min": -m, "max": m, "data_min": lo, "data_max": hi, "scale": "linear", "diverging": true}));
    }
    if let Some(cp) = cp.filter(|c| c.iter().any(|x| x.is_finite())) {
        let (lo, hi) = range(cp);
        simlab_core::write_le(&ctx.dir.join("field.cp.f32"), cp.iter().copied())?;
        fields.push(json!({"name": "cp", "label": "Pressure coefficient", "unit": "", "min": lo, "max": hi, "scale": "linear", "diverging": true}));
    }
    let mut list = vec![];
    for s in slices {
        if s.fields.is_empty() {
            continue;
        }
        let mut infos = vec![];
        for (name, label, unit, vals) in &s.fields {
            if !crate::api::safe_name(name) {
                continue;
            }
            let (lo, hi) = range(vals);
            simlab_core::write_le(&ctx.dir.join(format!("slice.{}.{name}.f32", s.name)), vals.iter().copied())?;
            infos.push(json!({"name": name, "label": label, "unit": unit, "min": lo, "max": hi}));
        }
        let meta = json!({"name": s.name, "label": s.label, "origin_mm": s.origin_mm, "u_mm": s.u_mm, "v_mm": s.v_mm, "nu": s.nu, "nv": s.nv, "fields": infos});
        std::fs::write(ctx.dir.join(format!("slice.{}.json", s.name)), serde_json::to_vec(&meta)?)?;
        list.push(json!({"name": s.name, "label": s.label}));
    }
    Ok((fields, list))
}

// ------------------------------------------------------------------ estimates

/// Time and memory for both engines, in the API's estimate shape. The
/// top level is `engine`'s; `engines` holds both so the UI can show the
/// time next to each choice.
pub fn estimate(app: &App, m: &ModelRow, cfg: &WindConfig, engine: &str) -> Result<Value> {
    let surf_m = crate::models::load_surface_mm(app, m)?.scaled(0.001);
    let of = openfoam_estimate(app, &surf_m, cfg).unwrap_or_else(|e| unavailable(&format!("{e:#}")));
    let gp = gpu_estimate(app, &surf_m, cfg).unwrap_or_else(|e| unavailable(&format!("{e:#}")));
    let mut top = if engine == "native" { gp.clone() } else { of.clone() };
    top["engines"] = json!({"reference": of, "native": gp});
    Ok(top)
}

fn unavailable(why: &str) -> Value {
    json!({"fits": false, "memory_bytes": 0, "budget_bytes": 0, "size": {}, "seconds": 0.0, "note": why, "available": false})
}

fn gpu_estimate(app: &App, surf_m: &TriMesh, cfg: &WindConfig) -> Result<Value> {
    let budget = app.settings().budget_bytes();
    let e = simlab_cfd::estimate(cfg, surf_m, 0.001, budget)?;
    Ok(json!({
        "engine": "native", "label": "Quick preview (GPU)",
        "fits": e.fits, "memory_bytes": e.memory_bytes, "budget_bytes": e.budget_bytes,
        "size": {"cells": e.cells}, "seconds": e.seconds,
        "note": if e.fits { format!("About {} air cells on the graphics chip", crate::solve::group(e.cells as usize)) } else { e.note },
        "available": true,
    }))
}

/// OpenFOAM's time and memory, calibrated on validation runs (OpenFOAM
/// v2606, 8 MPI ranks in Colima on an M4 Max, November 2026). The case
/// plan's cell count misses part of what snappyHexMesh adds, by a factor
/// that depends on the quality; seconds per final cell cover meshing and
/// the iterations a run of that kind needed:
///
/// | kind | quality | planned → real cells | run time | s per real cell |
/// |---|---|---|---|---|
/// | spinning (E-v5) | Draft | 404 k → 589 k | 252 s | 3.8e-4 |
/// | spinning (E-v5) | Normal | 1.62 M → 1.86 M | 595 s | 3.0e-4 |
/// | spinning (E-v5) | Fine | 1.62 M → 2.31 M | 1559 s | 6.6e-4 |
/// | still (sphere Re 1000) | Draft | 312 k → 341 k | 150 s | 3.5e-4 |
/// | still (sphere Re 100/1000) | Normal | 1.05 M → 1.59 M | 683–900 s | 4.1–5.5e-4 |
///
/// Still Fine was not run; it is extrapolated from Normal. Memory: 3.5 GB
/// for the 1.86 M-cell Normal propeller run. More than 8 cores did not
/// help: the same Normal propeller run took 582 s on 12 (Colima 12 cores),
/// so fewer than 8 slow it down as (8 / cores)^0.8 and more change nothing.
fn openfoam_estimate(app: &App, surf_m: &TriMesh, cfg: &WindConfig) -> Result<Value> {
    let plan = simlab_reference::plan::plan(surf_m, cfg, 0.001)?;
    let planned = simlab_reference::plan::estimate_cells(&plan, surf_m) as f64;
    let spin = plan.rotor.is_some();
    let (grow, s_per_cell) = match (&cfg.quality, spin) {
        (GridQuality::Draft, true) => (1.46, 3.8e-4),
        (GridQuality::Fine, true) => (1.43, 6.6e-4),
        (_, true) => (1.15, 3.0e-4),
        (GridQuality::Draft, false) => (1.10, 3.5e-4),
        (GridQuality::Fine, false) => (1.5, 6.5e-4),
        (_, false) => (1.52, 5.0e-4),
    };
    let cells = planned * grow;
    let (vm_cpus, vm_mem) = crate::system::docker_vm();
    let threads = app.settings().cpu_threads.max(1).min(vm_cpus.unwrap_or(usize::MAX)) as f64;
    let seconds = 30.0 + cells * s_per_cell * (8.0 / threads.min(8.0)).powf(0.8);
    let memory = (0.4e9 + 1700.0 * cells) as u64;
    let budget = vm_mem.unwrap_or_else(|| app.settings().budget_bytes());
    let fits = memory <= budget;
    let ready = crate::system::docker_running() && crate::system::reference_image_present();
    let note = if !fits {
        format!("Needs about {:.1} GB, more than Docker's {:.0} GB: pick a coarser quality, or give Colima more memory", memory as f64 / 1e9, budget as f64 / 1e9)
    } else if !ready {
        "Needs Docker (Colima) running with SimLab's reference image; SimLab asks before starting it".to_string()
    } else {
        format!("About {} cells, {} cores in Docker", crate::solve::group(cells as usize), threads as usize)
    };
    Ok(json!({
        "engine": "reference", "label": "Accurate (OpenFOAM)",
        "fits": fits, "memory_bytes": memory, "budget_bytes": budget,
        "size": {"cells": cells as u64}, "seconds": seconds, "note": note,
        "available": true, "docker_ready": ready, "threads": threads as usize,
        "budget_is": if vm_mem.is_some() { "Docker's memory" } else { "your memory budget" },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_digits() {
        assert_eq!(sig(345.297), "345");
        assert_eq!(sig(3.3862), "3.39");
        assert_eq!(sig(0.0758), "0.0758");
        assert_eq!(sig(21.98), "22.0");
        assert_eq!(sig(1.18e-6), "1.18e-6");
        assert_eq!(sig(-0.5), "-0.500");
        assert_eq!(sig(0.0), "0");
    }

    #[test]
    fn spin_centre_defaults_to_the_hub() {
        let m = ModelRow {
            id: "m".into(),
            project_id: "p".into(),
            name: "prop".into(),
            file_name: "prop.stl".into(),
            format: "stl".into(),
            unit_mm: 1.0,
            status: "ready".into(),
            message: String::new(),
            info: json!({"bbox_mm": {"min": [-30, -38, -3], "max": [47, 38, 3]}, "suggestions": {"prop": {"center_mm": [0.01, 0.0, 0.0], "axis": [0, 0, 1]}}}),
            patch_labels: json!({}),
            created_at: String::new(),
        };
        // no centre given: the detected hub, not the box centre (8.5 mm away)
        let mut c = json!({"kind": "wind", "wind_dir": [0, 0, -1], "speed_m_s": 0, "spin": {"rpm": 45800, "axis": [0, 0, 1]}});
        fill_spin_center(&m, &mut c);
        assert_eq!(c["spin"]["center_mm"], json!([0.01, 0.0, 0.0]));
        // a centre the user gave is kept
        let mut c = json!({"kind": "wind", "wind_dir": [0, 0, -1], "speed_m_s": 0, "spin": {"rpm": 45800, "axis": [0, 0, 1], "center_mm": [1, 2, 3]}});
        fill_spin_center(&m, &mut c);
        assert_eq!(c["spin"]["center_mm"], json!([1, 2, 3]));
        // no propeller found: the middle of the part
        let mut m2 = m.clone();
        m2.info = json!({"bbox_mm": {"min": [0, 0, 0], "max": [10, 20, 30]}});
        let mut c = json!({"kind": "wind", "wind_dir": [1, 0, 0], "speed_m_s": 5, "spin": {"rpm": 1000}});
        fill_spin_center(&m2, &mut c);
        assert_eq!(c["spin"]["center_mm"], json!([5.0, 10.0, 15.0]));
        assert_eq!(c["spin"]["axis"], json!([0, 0, 1]));
    }
}
