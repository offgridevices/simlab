//! The bridge between runs and the solver crates: this file, wind.rs (air
//! tests: OpenFOAM and the GPU tunnel) and calculix.rs (the strength
//! double-check) are the only ones that call simlab-fea, simlab-cfd,
//! simlab-prop and simlab-reference.

use crate::jobs::{Ctx, Outcome};
use crate::db::{ModelRow, RunRow};
use crate::state::App;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use simlab_core::{Load, MeshQuality, Progress, PropConfig, StrengthConfig, StudyConfig, TetMesh, TriMesh, Vec3};

/// The graphics chip the quick air previews run on.
pub fn gpu_info() -> Result<String> {
    simlab_cfd::gpu_info()
}

/// A propeller guess for the UI, from a model surface in mm.
pub fn detect_prop(m: &TriMesh) -> Option<Value> {
    let g = simlab_prop::detect_prop(m)?;
    if g.confidence < 0.5 {
        return None;
    }
    Some(json!({
        "axis": g.axis, "center_mm": g.center, "radius_mm": g.radius, "hub_radius_mm": g.hub_radius,
        "blades": g.blades, "rotation": g.rotation, "confidence": g.confidence,
    }))
}

pub fn run(ctx: &Ctx, r: &RunRow) -> Result<Outcome> {
    let cfg: StudyConfig = serde_json::from_value(r.config.clone())?;
    let study = ctx.app.db().study(&r.study_id)?;
    let model = ctx.app.db().model(&study.model_id)?;
    match (&cfg, r.engine.as_str()) {
        (StudyConfig::Prop(c), _) => prop(ctx, &model, c),
        (StudyConfig::Strength(c), "native") => strength(ctx, r, &model, c),
        (StudyConfig::Strength(c), "reference") => crate::calculix::run(ctx, r, &model, c),
        (StudyConfig::Wind(c), "native" | "reference") => crate::wind::run(ctx, r, &model, c),
        (_, e) => bail!("unknown engine {e}"),
    }
}

/// How close each solver came to known answers, shown with every result.
fn accuracy(kind: &str) -> Value {
    match kind {
        "prop" => json!({
            "method": "Blade element momentum theory with low-Reynolds airfoil data",
            "checked_against": "University of Illinois measured propeller data (7 props); your earlier OpenFOAM study (9 props, 17,000–45,800 rpm)",
            "typical_error": "Thrust within about 10% on low-pitch props like E-v5 and on measured props with a known blade section, but it reads high on steeper blades: 15–19% on P-v2 and 23–35% on A-v2 and P-v1 in your OpenFOAM study. Torque within about 12% (power read 24% low on one measured prop). Use the wind tunnel for the precise number.",
        }),
        "strength" => json!({
            "method": "Finite elements (quadratic tetrahedra) with print-direction-aware material and Tsai-Hill failure check",
            "checked_against": "CalculiX 2.21 on the same mesh; textbook and NAFEMS benchmarks",
            "typical_error": "Matches CalculiX within 0.004%; benchmarks within 0.03–0.8%. The real-world limit is how strong your print really is (layer bonding, infill, moisture), so treat a safety factor under 2 as risky.",
        }),
        "strength-reference" => json!({
            "method": "CalculiX 2.21 (in Docker) on the same mesh, loads and supports as SimLab's run; a geometrically nonlinear solve when the part spins and is held, so it stiffens under its own stress as SimLab's solver does",
            "checked_against": "An independent, widely used finite-element code: this run is the check",
            "typical_error": "A second opinion on the same model: when it agrees with SimLab's run, the solver is not the problem. Both share the same limit: how strong your print really is (layer bonding, infill, moisture).",
        }),
        _ => Value::Null,
    }
}

// ------------------------------------------------------------------ strength

/// Element size (model mm) for a quality: fine enough along the part and
/// through its thickness (t ≈ 2·volume/area for a plate-like part).
pub fn element_size_mm(info: &Value, q: &MeshQuality) -> f64 {
    if let MeshQuality::Custom { size_mm } = q {
        return size_mm.max(0.01);
    }
    let size = info["size_mm"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).fold(0.0, f64::max)).unwrap_or(100.0);
    let vol = info["volume_mm3"].as_f64().unwrap_or(0.0);
    let area = info["area_mm2"].as_f64().unwrap_or(1.0);
    let t = if vol > 0.0 { 2.0 * vol / area } else { size / 20.0 };
    let (k, f) = match q {
        MeshQuality::Draft => (30.0, 1.2),
        MeshQuality::Fine => (100.0, 0.45),
        _ => (60.0, 0.7),
    };
    (size / k).min(f * t).max(size / 2000.0)
}

fn strength_estimate(app: &App, m: &ModelRow, c: &StrengthConfig) -> Result<Value> {
    let h = element_size_mm(&m.info, &c.quality);
    let vol = m.info["volume_mm3"].as_f64().unwrap_or(0.0);
    // a regular tetrahedron of edge h fills h³/(6√2); Gmsh grades finer at
    // curves and edges, so allow 1.6× that count
    // Gmsh grades finer at curves, edges and thin spots: measured 2.5× the
    // even-fill count on a 3-inch prop, 1.6× on bulky parts
    let size = m.info["size_mm"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).fold(0.0, f64::max)).unwrap_or(100.0);
    let thin = 2.0 * vol / m.info["area_mm2"].as_f64().unwrap_or(1.0) < 0.05 * size;
    let grading = if thin { 4.0 } else { 1.6 };
    let tets = (grading * vol * 6.0 * std::f64::consts::SQRT_2 / h.powi(3)).max(1000.0) as usize;
    let nodes = (tets as f64 * 1.8) as usize;
    let est = simlab_fea::estimate_memory(nodes, tets);
    let budget = app.settings().budget_bytes();
    // thin parts go to the direct solver (measured: 579k unknowns, 97 s)
    let mem = if thin { est.direct_bytes } else { est.iterative_bytes.min(est.direct_bytes) };
    let solve_s = if thin { 97.0 * (est.dofs as f64 / 579_000.0).powf(1.6) } else { est.seconds };
    let tris = m.info["triangles"].as_u64().unwrap_or(0) as f64;
    let mesh_s = tris / 28_000.0 + tets as f64 / 5_000.0;
    let fits = mem <= budget;
    Ok(json!({
        "fits": fits, "memory_bytes": mem, "budget_bytes": budget,
        "size": {"elements": tets, "nodes": nodes, "dofs": est.dofs, "element_mm": h},
        "seconds": solve_s + mesh_s + 1.0,
        "mesh_seconds": mesh_s,
        "note": if fits { format!("About {} elements of {:.2} mm", group(tets), h) } else { "Too big for the memory budget: pick a coarser quality or raise the budget in Settings".into() },
    }))
}

pub fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn run_material(r: &RunRow) -> Result<simlab_core::Material> {
    Ok(serde_json::from_value(r.material.clone().ok_or_else(|| anyhow::anyhow!("no material saved with this run"))?)?)
}

/// A strength mesh for a run: (mesh, element size mm, meshing seconds).
/// Shared by SimLab's solver and the CalculiX check, so both see the same model.
pub fn strength_mesh(ctx: &Ctx, model: &ModelRow, c: &StrengthConfig) -> Result<(TetMesh, f64, f64)> {
    let dir = ctx.app.model_dir(&model.project_id, &model.id);
    let h = element_size_mm(&model.info, &c.quality);
    let unit_mm = model.unit_mm;

    // 1. mesh the part
    ctx.phase("running", 0.0, 0.25); // the API has one active status: "running" (the message says it is meshing)
    let t0 = std::time::Instant::now();
    let mesh_dir = ctx.dir.join("mesh");
    // CAD files mesh from their exact faces; triangle files (STL/OBJ/3MF)
    // are first simplified to about the element size (keeping their shape
    // within h/150), then filled with elements without remeshing the surface
    let cad = matches!(model.format.as_str(), "step" | "iges");
    let (input, size_arg, unit_arg) = if cad {
        (crate::models::mesher_input(&dir, &model.format), format!("{}", h / unit_mm), format!("{}", unit_mm * 1e-3))
    } else {
        ctx.report(0.02, "Simplifying the surface to the element size");
        let surf = crate::models::load_surface_mm(ctx.app, model)?;
        let area: f64 = (0..surf.triangles.len()).map(|t| surf.triangle_area(t)).sum();
        let opts = simlab_core::decimate::DecimateOptions {
            target_tris: simlab_core::decimate::target_tris_for(area, h),
            max_error: h / 150.0,
            max_edge: h,
        };
        let simple = if surf.triangles.len() > opts.target_tris { simlab_core::decimate::decimate_with(&surf, &opts) } else { surf };
        ctx.log(&format!("surface simplified to {} triangles", simple.triangles.len()));
        let sdir = ctx.dir.join("surface-in");
        simlab_core::decimate::write_mesher_surface(&sdir, &simple)?;
        (sdir, format!("{h}"), "0.001".to_string())
    };
    if ctx.cancelled() {
        return Err(simlab_core::Cancelled.into());
    }
    crate::models::run_mesher(
        ctx.app,
        &["volume".as_ref(), input.as_os_str(), mesh_dir.as_os_str(), "--size".as_ref(), size_arg.as_ref(), "--unit".as_ref(), unit_arg.as_ref()],
        |p, m| ctx.report(0.1 + 0.9 * p, m),
    )
    .map_err(|e| anyhow::anyhow!("could not fill the part with elements: {e:#}. If the file came from a scan or a mesh tool, repair it (for example PrusaSlicer: right-click → Fix by Netfabb) and upload it again."))?;
    if ctx.cancelled() {
        return Err(simlab_core::Cancelled.into());
    }
    let mut mesh = TetMesh::read_dir(&mesh_dir)?;
    // areas picked by shape: their faces take the region's id
    for reg in &c.regions {
        let mut n = 0;
        for (f, p) in mesh.faces.iter().zip(mesh.face_patch.iter_mut()) {
            let cen = [0, 1, 2].map(|k| (mesh.nodes[f[0] as usize][k] + mesh.nodes[f[1] as usize][k] + mesh.nodes[f[2] as usize][k]) / 3.0 * 1e3);
            if reg.shape.contains(cen) {
                *p = reg.id;
                n += 1;
            }
        }
        if n == 0 {
            anyhow::bail!("the area \"{}\" does not touch the part's surface; make it bigger or move it", if reg.label.is_empty() { "picked by shape" } else { &reg.label });
        }
        ctx.log(&format!("region {} ({}) covers {n} faces", reg.id, reg.label));
    }
    let mesh_seconds = t0.elapsed().as_secs_f64();
    ctx.log(&format!("mesh: {} elements, {} nodes, element size {h:.3} mm, {mesh_seconds:.1} s", mesh.tets.len(), mesh.nodes.len()));
    let _ = std::fs::remove_dir_all(&mesh_dir); // big; the run can be repeated
    let _ = std::fs::remove_dir_all(ctx.dir.join("surface-in"));
    Ok((mesh, h, mesh_seconds))
}

fn strength(ctx: &Ctx, r: &RunRow, model: &ModelRow, c: &StrengthConfig) -> Result<Outcome> {
    let material = run_material(r)?;
    let (mesh, h, mesh_seconds) = strength_mesh(ctx, model, c)?;

    // 2. propeller air loads, from the blade element model
    let tractions = prop_tractions(ctx, model, &mesh, c)?;

    // 3. solve
    ctx.phase("running", 0.25, 0.97);
    let opts = simlab_fea::SolveOptions { memory_budget_bytes: ctx.app.settings().budget_bytes(), ..Default::default() };
    let input = simlab_fea::StrengthInput { mesh: &mesh, material: &material, config: c, model_unit_m: 0.001, face_tractions: tractions.as_deref() };
    let res = simlab_fea::solve_strength(&input, &opts, ctx)?;
    strength_outcome(ctx, &mesh, &res, h, mesh_seconds, "strength")
}

/// Writes a strength result (surface, fields in model mm) and its summary.
pub fn strength_outcome(ctx: &Ctx, mesh: &TetMesh, res: &simlab_fea::StrengthResult, h: f64, mesh_seconds: f64, accuracy_kind: &str) -> Result<Outcome> {
    // 4. write the result surface and its fields (model mm)
    ctx.phase("running", 0.97, 1.0);
    ctx.report(0.0, "Saving results");
    let surf = mesh.display_surface();
    let mut map = vec![u32::MAX; surf.positions.len()];
    let mut used: Vec<usize> = vec![];
    for t in &surf.triangles {
        for &i in t {
            if map[i as usize] == u32::MAX {
                map[i as usize] = used.len() as u32;
                used.push(i as usize);
            }
        }
    }
    let compact = TriMesh {
        positions: used.iter().map(|&i| simlab_core::scale(surf.positions[i], 1e3)).collect(),
        triangles: surf.triangles.iter().map(|t| t.map(|i| map[i as usize])).collect(),
        patch: surf.patch.clone(),
    };
    simlab_core::write_display_mesh(&ctx.dir, "mesh", &compact)?;
    let mut fields = vec![];
    let write = |name: &str, label: &str, unit: &str, vals: Vec<f32>, diverging: bool, fields: &mut Vec<Value>| -> Result<()> {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for &v in &vals {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        simlab_core::write_le(&ctx.dir.join(format!("field.{name}.f32")), vals)?;
        fields.push(json!({"name": name, "label": label, "unit": unit, "min": lo, "max": hi, "scale": "linear", "diverging": diverging}));
        Ok(())
    };
    for f in res.surface_fields() {
        let vals: Vec<f32> = used.iter().map(|&i| f.values[i]).collect();
        write(f.name, f.label, f.unit, vals, f.diverging, &mut fields)?;
    }
    for (k, name) in ["ux", "uy", "uz"].iter().enumerate() {
        let vals: Vec<f32> = used.iter().map(|&i| (res.displacement[i][k] * 1e3) as f32).collect();
        write(name, &format!("Movement {}", &name[1..]), "mm", vals, true, &mut fields)?;
    }
    let mut summary = serde_json::to_value(res.summary())?;
    // The legend's range follows the headline numbers, which leave out the
    // rigidly held faces (a perfectly rigid hold exaggerates stress there);
    // those spots still show, saturated at the end of the colour scale.
    let sf = summary["safety_factor"].as_f64();
    let vm = summary["max_von_mises_mpa"].as_f64();
    for f in fields.iter_mut() {
        match (f["name"].as_str(), sf, vm) {
            (Some("safety_factor"), Some(sf), _) if sf.is_finite() => f["min"] = json!(sf),
            (Some("failure_index"), Some(sf), _) if sf.is_finite() && sf > 0.0 => f["max"] = json!(1.0 / (sf * sf)),
            (Some("von_mises"), _, Some(vm)) => f["max"] = json!(vm),
            _ => continue,
        }
        f["range_note"] = json!("Range leaves out the held faces, where a perfectly rigid hold exaggerates stress");
    }
    summary["mesh_seconds"] = json!(mesh_seconds);
    summary["element_mm"] = json!(h);
    summary["accuracy"] = accuracy(accuracy_kind);
    // the critical point in model mm
    summary["critical_point_mm"] = json!(simlab_core::scale(res.critical.position_m, 1e3));
    Ok(Outcome { summary, fields: json!(fields), slices: json!([]) })
}

/// Air load on a propeller's faces for every PropAero load, from BEMT.
pub fn prop_tractions(ctx: &Ctx, model: &ModelRow, mesh: &TetMesh, c: &StrengthConfig) -> Result<Option<Vec<Vec3>>> {
    let aero: Vec<&Load> = c.loads.iter().filter(|l| matches!(l, Load::PropAero { .. })).collect();
    if aero.is_empty() {
        return Ok(None);
    }
    let surface = crate::models::load_surface_mm(ctx.app, model)?;
    let faces: Vec<simlab_prop::BoundaryFace> = simlab_fea::boundary_faces(mesh)
        .into_iter()
        .map(|(centroid, normal, area)| simlab_prop::BoundaryFace { centroid, normal, area })
        .collect();
    let mut total = vec![[0.0; 3]; mesh.faces.len()];
    for l in aero {
        let Load::PropAero { rpm, airspeed_m_s, axis, center_mm, .. } = l else { continue };
        let ex = simlab_prop::extract_geometry(&surface, 0.001, *axis, *center_mm, &simlab_prop::ExtractOptions::default())?;
        let op = simlab_prop::solve(&ex.geometry, *rpm, *airspeed_m_s, &simlab_core::Air::default(), &simlab_prop::ModelOptions::default());
        ctx.log(&format!("propeller air load at {rpm} rpm: thrust {:.1} g, torque {:.2} mN·m", op.thrust_g, op.torque_nm * 1e3));
        let t = simlab_prop::face_tractions(&op, &ex.frame, ex.geometry.rotation, &faces);
        for (a, b) in total.iter_mut().zip(t) {
            *a = simlab_core::add(*a, b);
        }
    }
    Ok(Some(total))
}

fn prop(ctx: &Ctx, model: &ModelRow, c: &PropConfig) -> Result<Outcome> {
    let surface = crate::models::load_surface_mm(ctx.app, model)?;
    ctx.phase("running", 0.0, 1.0);
    let run = simlab_prop::run(&surface, 0.001, c, ctx)?;
    let mut summary = serde_json::to_value(run.summary())?;
    summary["accuracy"] = accuracy("prop");
    if let Some(p) = summary["points"].as_array().and_then(|a| a.iter().filter(|p| p["airspeed_m_s"].as_f64() == Some(0.0)).last()) {
        summary["short"] = json!(format!("{:.0} g at {} rpm", p["thrust_g"].as_f64().unwrap_or(0.0), group(p["rpm"].as_f64().unwrap_or(0.0) as usize)));
    }
    Ok(Outcome { summary, fields: json!([]), slices: json!([]) })
}

/// The run's estimate for `engine` ("native" or "reference"), in the API's shape.
pub fn estimate(app: &App, m: &ModelRow, c: &StudyConfig, engine: &str) -> Result<Value> {
    match c {
        StudyConfig::Strength(s) if engine == "reference" => crate::calculix::estimate(app, m, s, strength_estimate(app, m, s)?),
        StudyConfig::Strength(s) => strength_estimate(app, m, s),
        StudyConfig::Prop(_) => Ok(json!({"fits": true, "memory_bytes": 50_000_000u64, "budget_bytes": app.settings().budget_bytes(), "size": {}, "seconds": 1.0, "note": "Under a second"})),
        StudyConfig::Wind(w) => crate::wind::estimate(app, m, w, engine),
    }
}
