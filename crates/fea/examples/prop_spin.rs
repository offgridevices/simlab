//! Spin-only stress test of a propeller, set up like the earlier CalculiX
//! study (hub bottom face clamped, centrifugal load about z through the
//! origin), for comparison.
//!
//! ```text
//! cargo run --release -p simlab-fea --example prop_spin -- <prop.inp | mesh_dir>
//!     [--rpm 51500] [--rho 1270] [--e-mpa 1700] [--nu 0.38] [--uts-mpa 39]
//!     [--zcut -2.67] [--rcut 4.9] [--blade-r 5.3] [--write-inp out.inp]
//!     [--solver auto|direct|iterative] [--stiffening auto|off] [--compare-frd job.frd]
//! ```
//! A `.inp` deck is read in millimetres and its `FIX` node set is clamped;
//! a mesher directory is clamped at nodes with z < zcut and r < rcut (mm).

use anyhow::{Result, bail};
use simlab_core::{Load, Material, MeshQuality, Silent, StrengthConfig, Support, TetMesh, norm};
use simlab_fea::assembly::Dirichlet;
use simlab_fea::calculix::{InpOptions, read_inp_mesh, write_inp_file};
use simlab_fea::{SolveOptions, StrengthInput, postprocess_with, prepare_with_bc, solve_prepared};
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        bail!("usage: prop_spin <prop.inp | mesh_dir> [options]");
    }
    let mut o = std::collections::HashMap::new();
    for kv in args[1..].chunks(2) {
        o.insert(kv[0].trim_start_matches("--").to_string(), kv.get(1).cloned().unwrap_or_default());
    }
    let get = |k: &str, d: f64| o.get(k).map(|v| v.parse().unwrap()).unwrap_or(d);
    let (rpm, rho, e, nu, uts) = (get("rpm", 51500.0), get("rho", 1270.0), get("e-mpa", 1700.0), get("nu", 0.38), get("uts-mpa", 39.0));
    let (zcut, rcut, blade_r) = (get("zcut", -2.67) * 1e-3, get("rcut", 4.9) * 1e-3, get("blade-r", 5.3) * 1e-3);
    let t = Instant::now();
    let (mesh, fix): (TetMesh, Vec<u32>) = if args[0].ends_with(".inp") {
        let m = read_inp_mesh(&std::fs::read_to_string(&args[0])?, 1e-3)?;
        let fix = m.nsets.get("FIX").cloned().unwrap_or_default();
        (m.mesh, fix)
    } else {
        let m = TetMesh::read_dir(std::path::Path::new(&args[0]))?;
        let fix = (0..m.nodes.len() as u32)
            .filter(|&i| {
                let p = m.nodes[i as usize];
                p[2] < zcut && (p[0] * p[0] + p[1] * p[1]).sqrt() < rcut
            })
            .collect();
        (m, fix)
    };
    if fix.is_empty() {
        bail!("no clamped nodes found");
    }
    eprintln!("mesh: {} nodes, {} elements, {} clamped nodes ({:.1} s)", mesh.nodes.len(), mesh.tets.len(), fix.len(), t.elapsed().as_secs_f64());
    let mat = Material::isotropic("prop", "prop", rho, e, nu, uts);
    let cfg = StrengthConfig {
        material_id: "prop".into(),
        print_up: [0.0, 0.0, 1.0],
        support: Support::FreeFlying,
        loads: vec![Load::Spin { rpm, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3], label: String::new() }],
        quality: MeshQuality::Normal,
        regions: vec![],
    };
    let input = StrengthInput { mesh: &mesh, material: &mat, config: &cfg, model_unit_m: 0.001, face_tractions: None };
    let mut bc = Dirichlet::new(3 * mesh.nodes.len());
    for &v in &fix {
        for k in 0..3 {
            bc.set(3 * v as usize + k, 0.0);
        }
    }
    let prep = prepare_with_bc(&input, bc)?;
    if let Some(p) = o.get("write-inp") {
        write_inp_file(std::path::Path::new(p), &mesh, &prep, &InpOptions::default())?;
    }
    let t = Instant::now();
    let opts = SolveOptions {
        solver: match o.get("solver").map(|s| s.as_str()) {
            Some("direct") => simlab_fea::SolverChoice::Direct,
            Some("iterative") => simlab_fea::SolverChoice::Iterative,
            _ => simlab_fea::SolverChoice::Auto,
        },
        rel_tol: get("tol", 1e-9),
        max_iterations: get("maxit", 5000.0) as usize,
        stiffening: if o.get("stiffening").map(|s| s.as_str()) == Some("off") { simlab_fea::Stiffening::Off } else { simlab_fea::Stiffening::Auto },
        ..Default::default()
    };
    let (u, rep, pre) = solve_prepared(&input, &prep, &opts, &simlab_core::Stderr::default(), (0.0, 1.0))?;
    let _ = Silent;
    let solve_s = t.elapsed().as_secs_f64();
    eprintln!("stiffening passes {} (last change {:.1e}), fallback {:?}", rep.stiffening_passes, rep.stiffening_change, rep.fallback);
    let r = postprocess_with(&input, prep, u, rep, pre)?;
    let rad = |i: usize| (mesh.nodes[i][0].powi(2) + mesh.nodes[i][1].powi(2)).sqrt();
    let (mut blade, mut bi, mut hub) = (0.0f64, 0usize, 0.0f64);
    let mut lift = (0.0, 0usize);
    for i in 0..mesh.nodes.len() {
        let v = r.von_mises[i];
        if rad(i) > blade_r {
            if v > blade {
                blade = v;
                bi = i;
            }
        } else {
            hub = hub.max(v);
        }
        if rad(i) > 36.5e-3 {
            lift.0 += r.displacement[i][2];
            lift.1 += 1;
        }
    }
    let w = rpm * std::f64::consts::TAU / 60.0;
    let rmax = (0..mesh.nodes.len()).map(rad).fold(0.0, f64::max);
    let bar = rho * w * w * (rmax * rmax - 0.005f64.powi(2)) / 2.0;
    let sf = uts * 1e6 / blade;
    println!("rpm {rpm}, rho {rho}, E {e} MPa, nu {nu}; tip radius {:.2} mm", rmax * 1e3);
    println!("blade peak von Mises (r > {:.1} mm): {:.2} MPa at r = {:.2} mm, z = {:.2} mm", blade_r * 1e3, blade / 1e6, rad(bi) * 1e3, mesh.nodes[bi][2] * 1e3);
    println!("hub/clamp peak: {:.2} MPa", hub / 1e6);
    println!("uniform-bar root estimate rho w^2 (R^2 - 5mm^2)/2: {:.2} MPa", bar / 1e6);
    println!("blade safety factor vs {uts} MPa: {:.2}; breaks at about {:.0} rpm", sf, rpm * sf.sqrt());
    println!("tip lift (mean uz, r > 36.5 mm): {:.3} mm; max movement {:.3} mm", lift.0 / lift.1.max(1) as f64 * 1e3, r.max_displacement_m * 1e3);
    println!("whole-part headline SF (excludes clamped nodes): {:.2}; robust element peak SF {:.2}", r.safety_factor, 1.0 / r.peaks.fi_element.sqrt());
    println!("mass {:.3} g; solve {:.2} s ({}); |reaction| {:.2} N", r.mass_kg * 1e3, solve_s, r.solver.detail, norm(r.reaction_force_n));
    if let Some(f) = o.get("compare-frd") {
        let frd = simlab_fea::calculix::read_frd_file(std::path::Path::new(f))?;
        let n = mesh.nodes.len();
        let c = simlab_fea::calculix::compare(&r.displacement, &r.stress, &frd.displacements(n)?, &frd.stresses(n)?);
        println!(
            "vs CalculiX: displacement max diff {:.2e} of peak; von Mises max diff {:.2e}, RMS {:.2e} of peak; peak vM {:.4} vs {:.4} MPa; peak |u| {:.5} vs {:.5} mm",
            c.displacement,
            c.von_mises,
            c.von_mises_rms,
            c.peak_von_mises.0 / 1e6,
            c.peak_von_mises.1 / 1e6,
            c.peak_displacement.0 * 1e3,
            c.peak_displacement.1 * 1e3
        );
    }
    Ok(())
}
