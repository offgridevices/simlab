//! Runs a strength study on a mesher output directory and prints the summary.
//!
//! ```text
//! cargo run --release -p simlab-fea --example solve_dir -- <mesh_dir> <config.json>
//!     [--material <builtin id | material.json>]   (default: config.material_id)
//!     [--solver auto|direct|iterative] [--budget-gb 32] [--unit 0.001]
//!     [--inp out.inp] [--fields out_dir] [--json summary.json]
//! ```
//! `config.json` is a `StrengthConfig` (the `strength` study config without
//! the `kind` tag). With `--fields`, writes the display surface
//! (`result.*` buffers) and one `field.<name>.bin` (f32 per vertex) per field.

use anyhow::{Context, Result, bail};
use simlab_core::{Material, StrengthConfig, TetMesh, builtin_materials, write_display_mesh, write_le};
use simlab_fea::calculix::{InpOptions, write_inp_file};
use simlab_fea::{SolveOptions, SolverChoice, StrengthInput, prepare, solve_strength};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        bail!("usage: solve_dir <mesh_dir> <config.json> [--material id|file] [--solver auto|direct|iterative] [--budget-gb N] [--unit m] [--inp file] [--fields dir] [--json file]");
    }
    let mesh_dir = PathBuf::from(&args[0]);
    let cfg: StrengthConfig = serde_json::from_slice(&std::fs::read(&args[1]).context("read config")?).context("parse config")?;
    let mut material_arg = None;
    let mut solver = SolverChoice::Auto;
    let mut budget_gb = 32.0;
    let mut unit = 0.001;
    let (mut inp, mut fields, mut json) = (None, None, None);
    let mut i = 2;
    while i < args.len() {
        let v = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--material" => material_arg = Some(v),
            "--solver" => {
                solver = match v.as_str() {
                    "direct" => SolverChoice::Direct,
                    "iterative" => SolverChoice::Iterative,
                    _ => SolverChoice::Auto,
                }
            }
            "--budget-gb" => budget_gb = v.parse()?,
            "--unit" => unit = v.parse()?,
            "--inp" => inp = Some(PathBuf::from(v)),
            "--fields" => fields = Some(PathBuf::from(v)),
            "--json" => json = Some(PathBuf::from(v)),
            other => bail!("unknown option {other}"),
        }
        i += 2;
    }
    let id = material_arg.clone().unwrap_or_else(|| cfg.material_id.clone());
    let material: Material = if id.ends_with(".json") {
        serde_json::from_slice(&std::fs::read(&id)?)?
    } else {
        builtin_materials().into_iter().find(|m| m.id == id).with_context(|| format!("no built-in material {id}"))?
    };
    let t = Instant::now();
    let mesh = TetMesh::read_dir(&mesh_dir)?;
    eprintln!("read mesh: {} nodes, {} elements, {} faces in {:.2} s", mesh.nodes.len(), mesh.tets.len(), mesh.faces.len(), t.elapsed().as_secs_f64());
    let est = simlab_fea::estimate_memory(mesh.nodes.len(), mesh.tets.len());
    eprintln!("estimate: direct {:.2} GB, iterative {:.2} GB", est.direct_bytes as f64 / 1e9, est.iterative_bytes as f64 / 1e9);
    let input = StrengthInput { mesh: &mesh, material: &material, config: &cfg, model_unit_m: unit, face_tractions: None };
    if let Some(p) = &inp {
        let prep = prepare(&input)?;
        write_inp_file(p, &mesh, &prep, &InpOptions::default())?;
        eprintln!("wrote {}", p.display());
    }
    let opts = SolveOptions { solver, memory_budget_bytes: (budget_gb * 1e9) as u64, ..Default::default() };
    let progress = simlab_core::Stderr::default();
    let r = solve_strength(&input, &opts, &progress)?;
    let s = r.summary();
    println!("{}", serde_json::to_string_pretty(&s)?);
    eprintln!("solver: {}", serde_json::to_string(&r.solver)?);
    eprintln!("timings: {}", serde_json::to_string(&r.timings)?);
    eprintln!("peaks: {}", serde_json::to_string(&r.peaks)?);
    if let Some(ir) = &r.inertia_relief {
        eprintln!("inertia relief: {}", serde_json::to_string(ir)?);
    }
    if let Some(p) = &json {
        let all = serde_json::json!({ "summary": s, "solver": r.solver, "timings": r.timings, "peaks": r.peaks, "inertia_relief": r.inertia_relief, "critical": r.critical });
        std::fs::write(p, serde_json::to_vec_pretty(&all)?)?;
    }
    if let Some(dir) = &fields {
        std::fs::create_dir_all(dir)?;
        let surf = mesh.display_surface().scaled(1.0 / unit);
        write_display_mesh(dir, "result", &surf)?;
        for f in r.surface_fields() {
            write_le(&dir.join(format!("field.{}.bin", f.name)), f.values.iter().copied())?;
        }
        eprintln!("wrote fields to {}", dir.display());
    }
    Ok(())
}
