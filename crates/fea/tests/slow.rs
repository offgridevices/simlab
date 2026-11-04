//! Slow validation (Gmsh meshes) and CalculiX comparisons. Run with
//! `cargo test --release -p simlab-fea --test slow -- --ignored --nocapture`.
mod common;
use common::*;
use simlab_core::{Load, Material, Silent, StrengthConfig, Support, TetMesh, Vec3};
use simlab_fea::assembly::Dirichlet;
use simlab_fea::mesh_gen::{Midside, box_mesh};
use simlab_fea::{Prepared, SolveOptions, Stiffening, StrengthInput, StrengthResult, postprocess_with, prepare, prepare_with_bc, solve_prepared};

fn solve_full(mesh: &TetMesh, mat: &Material, c: &StrengthConfig, bc: Option<Dirichlet>, opts: &SolveOptions) -> (Prepared, StrengthResult) {
    let input = StrengthInput { mesh, material: mat, config: c, model_unit_m: 0.001, face_tractions: None };
    let prep = match bc {
        Some(b) => prepare_with_bc(&input, b).unwrap(),
        None => prepare(&input).unwrap(),
    };
    let (u, rep, pre) = solve_prepared(&input, &prep, opts, &Silent, (0.0, 1.0)).unwrap();
    (prep.clone(), postprocess_with(&input, prep, u, rep, pre).unwrap())
}

/// Patch ids of boundary faces whose corners all satisfy `f`.
fn patches_where(mesh: &TetMesh, f: impl Fn(Vec3) -> bool) -> Vec<u32> {
    let mut v: Vec<u32> = mesh.faces.iter().zip(&mesh.face_patch).filter(|(fc, _)| fc[..3].iter().all(|&n| f(mesh.nodes[n as usize]))).map(|(_, &p)| p).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[test]
#[ignore]
#[allow(clippy::approx_constant)] // 3.140 is the Peterson fit coefficient
fn plate_with_hole_peterson() {
    let Some((py, script)) = mesher() else { panic!("mesher venv not found") };
    let dir = scratch().join("plate");
    std::fs::create_dir_all(&dir).unwrap();
    let (w, d, l, t) = (50.0, 10.0, 150.0, 2.0);
    let step = dir.join("plate.step");
    gmsh_step(
        &py,
        &format!("b = gmsh.model.occ.addBox(0, 0, 0, {l}, {w}, {t})\nc = gmsh.model.occ.addCylinder({}, {}, -1, 0, 0, {}, {})\ngmsh.model.occ.cut([(3, b)], [(3, c)])", l / 2.0, w / 2.0, t + 2.0, d / 2.0),
        &step,
    );
    let size: f64 = std::env::var("PLATE_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.7);
    let mesh = mesh_step(&py, &script, &step, &dir.join("mesh"), size, 0.001);
    let left = patches_where(&mesh, |x| x[0].abs() < 1e-9);
    let right = patches_where(&mesh, |x| (x[0] - l * 1e-3).abs() < 1e-9);
    let p = 10e6;
    let c = cfg(
        Support::FreeFlying,
        vec![
            Load::Pressure { patches: left, pressure_pa: -p, label: String::new() },
            Load::Pressure { patches: right, pressure_pa: -p, label: String::new() },
        ],
        [0.0, 0.0, 1.0],
    );
    let mat = steel();
    let (prep, r) = solve_full(&mesh, &mat, &c, None, &SolveOptions::default());
    let (xc, yc, rad) = (l / 2.0 * 1e-3, w / 2.0 * 1e-3, d / 2.0 * 1e-3);
    let hole: Vec<usize> = nodes_where(&mesh, |x| ((x[0] - xc).powi(2) + (x[1] - yc).powi(2)).sqrt() - rad < 1e-7);
    let peak = hole.iter().map(|&i| r.stress[i][0]).fold(f64::MIN, f64::max);
    // thickness average at the top of the hole (θ = 90°)
    let top: Vec<usize> = hole.iter().copied().filter(|&i| (mesh.nodes[i][0] - xc).abs() < 1e-7).collect();
    let avg = top.iter().map(|&i| r.stress[i][0]).sum::<f64>() / top.len().max(1) as f64;
    let dw = d / w;
    let kt = 3.0 - 3.140 * dw + 3.667 * dw * dw - 1.527 * dw.powi(3);
    let nom = p * w / (w - d);
    println!(
        "plate with hole ({} nodes): peak σxx {:.3} MPa = Kt {:.4}; thickness-average at θ=90° Kt {:.4} ({} nodes); Peterson Kt_n {:.4}; errors {:.2} % / {:.2} %",
        mesh.nodes.len(),
        peak / 1e6,
        peak / nom,
        avg / nom,
        top.len(),
        kt,
        100.0 * rel(peak / nom, kt),
        100.0 * rel(avg / nom, kt)
    );
    assert!(rel(peak / nom, kt) < 0.03);
    if std::env::var("PLATE_SIZE").is_ok() {
        return; // size study: skip the CalculiX run
    }
    if let Some(cmp) = ccx_compare("plate", &mesh, &prep, &r, true) {
        assert!(cmp.displacement < 1e-3 && cmp.von_mises < 1e-3);
    }
}

#[test]
#[ignore]
fn nafems_le10() {
    let Some((py, script)) = mesher() else { panic!("mesher venv not found") };
    let dir = scratch().join("le10");
    std::fs::create_dir_all(&dir).unwrap();
    let step = dir.join("le10.step");
    // quarter elliptic annulus, 0.6 m thick, split at z = 0 so the mid-plane has nodes (mm)
    gmsh_step(
        &py,
        "occ = gmsh.model.occ\n\
         def ring(z0, h):\n\
         \x20   o = occ.addDisk(0, 0, z0, 3250, 2750)\n\
         \x20   i = occ.addDisk(0, 0, z0, 2000, 1000)\n\
         \x20   a, _ = occ.cut([(2, o)], [(2, i)])\n\
         \x20   q = occ.addRectangle(0, 0, z0, 4000, 4000)\n\
         \x20   s, _ = occ.intersect(a, [(2, q)])\n\
         \x20   return occ.extrude(s, 0, 0, h)\n\
         ring(-300, 300)\n\
         ring(0, 300)\n\
         occ.removeAllDuplicates()",
        &step,
    );
    let size: f64 = std::env::var("LE10_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
    let mesh = mesh_step(&py, &script, &step, &dir.join("mesh"), size, 0.001);
    let n = mesh.nodes.len();
    let ell = |x: Vec3, a: f64, b: f64| (x[0] / a).powi(2) + (x[1] / b).powi(2);
    let mut bc = Dirichlet::new(3 * n);
    let mut outer = vec![false; n];
    for f in &mesh.faces {
        let pts: Vec<Vec3> = f[..3].iter().map(|&v| mesh.nodes[v as usize]).collect();
        let on = |g: &dyn Fn(Vec3) -> bool| pts.iter().all(|p| g(*p));
        for &v in f {
            let v = v as usize;
            if on(&|p| p[1].abs() < 1e-9) {
                bc.set(3 * v + 1, 0.0);
            }
            if on(&|p| p[0].abs() < 1e-9) {
                bc.set(3 * v, 0.0);
            }
            if on(&|p| (ell(p, 3.25, 2.75) - 1.0).abs() < 1e-5) {
                bc.set(3 * v, 0.0);
                bc.set(3 * v + 1, 0.0);
                outer[v] = true;
            }
        }
    }
    let mut mid = 0;
    for v in 0..n {
        if outer[v] && mesh.nodes[v][2].abs() < 1e-9 {
            bc.set(3 * v + 2, 0.0);
            mid += 1;
        }
    }
    let top = patches_where(&mesh, |x| (x[2] - 0.3).abs() < 1e-9);
    let c = cfg(Support::FreeFlying, vec![Load::Pressure { patches: top, pressure_pa: 1e6, label: String::new() }], [0.0, 0.0, 1.0]);
    let mat = Material::isotropic("le10", "le10", 7800.0, 210e3, 0.3, 500.0);
    let (_prep, r) = solve_full(&mesh, &mat, &c, Some(bc), &SolveOptions::default());
    let d = (0..n).min_by(|&a, &b| {
        let da = simlab_core::norm(simlab_core::sub(mesh.nodes[a], [2.0, 0.0, 0.3]));
        let db = simlab_core::norm(simlab_core::sub(mesh.nodes[b], [2.0, 0.0, 0.3]));
        da.partial_cmp(&db).unwrap()
    })
    .unwrap();
    let syy = r.stress[d][1];
    println!("NAFEMS LE10 ({n} nodes, size {size} mm, {mid} mid-plane nodes held): σyy at D = {:.4} MPa (target -5.38), error {:.2} %", syy / 1e6, 100.0 * rel(syy, -5.38e6));
    assert!(rel(syy, -5.38e6) < 0.03);
}

// ---------------------------------------------------------------- CalculiX

fn ccx_cases(native: bool) -> Vec<(String, f64, f64)> {
    let mut out = vec![];
    let mut aniso = Material::isotropic("a", "a", 1250.0, 2117.0, 0.38, 50.8);
    aniso.e_z_mpa = 1340.0;
    aniso.uts_z_mpa = 22.86;
    let beam = box_mesh(30, 3, 3, |p| [p[0] * 0.09, p[1] * 0.012, p[2] * 0.008], Midside::Mapped);
    let off = SolveOptions { stiffening: Stiffening::Off, ..Default::default() };
    let cases: Vec<(&str, TetMesh, Material, StrengthConfig)> = vec![
        ("cantilever", beam.clone(), steel(), cfg(Support::Fixed { patches: vec![1] }, vec![Load::Force { patches: vec![2], force_n: [0.0, 0.0, -100.0], label: String::new() }], [0.0, 0.0, 1.0])),
        (
            "anisotropic",
            beam.clone(),
            aniso.clone(),
            cfg(
                Support::Fixed { patches: vec![1] },
                vec![
                    Load::Force { patches: vec![2], force_n: [5.0, 3.0, -20.0], label: String::new() },
                    Load::Pressure { patches: vec![6], pressure_pa: 2e4, label: String::new() },
                    Load::Acceleration { g: [0.0, 0.0, -10.0], label: String::new() },
                ],
                [0.4, 0.0, 1.0],
            ),
        ),
        (
            "spin",
            box_mesh(30, 3, 2, |p| [p[0] * 0.04 + 0.005, (p[1] - 0.5) * 0.008, (p[2] - 0.5) * 0.003], Midside::Mapped),
            aniso.clone(),
            cfg(Support::Fixed { patches: vec![1] }, vec![Load::Spin { rpm: 30000.0, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3], label: String::new() }], [0.0, 0.0, 1.0]),
        ),
        (
            "inertia_relief",
            beam.clone(),
            aniso,
            cfg(
                Support::FreeFlying,
                vec![
                    Load::Motor { patches: vec![2], thrust_n: 12.0, torque_nm: 0.1, axis: [0.0, 0.0, 1.0], motor_mass_g: 30.0, label: String::new() },
                    Load::Force { patches: vec![1], force_n: [0.0, 0.0, -4.0], label: String::new() },
                    Load::Acceleration { g: [0.0, 0.0, -1.0], label: String::new() },
                ],
                [0.0, 0.0, 1.0],
            ),
        ),
    ];
    for (name, mesh, mat, c) in cases {
        let (prep, r) = solve_full(&mesh, &mat, &c, None, &off);
        let tag = format!("{name}-{}", if native { "native" } else { "nodal" });
        match ccx_compare(&tag, &mesh, &prep, &r, native) {
            Some(cmp) => out.push((tag, cmp.displacement, cmp.von_mises)),
            None => panic!("CalculiX is not available (no ccx on PATH and no simlab-reference:1 image)"),
        }
    }
    out
}

/// Same mesh, same loads: displacements and nodal stresses within 0.1 % of
/// the peak, node by node. Loads written as the identical nodal forces.
#[test]
#[ignore]
fn calculix_same_loads() {
    for (name, du, dv) in ccx_cases(false) {
        assert!(du < 1e-3 && dv < 1e-3, "{name}: displacement {du:.2e}, von Mises {dv:.2e}");
    }
}

/// Same, with CalculiX's own load cards (*DLOAD P, GRAV, CENTRIF).
#[test]
#[ignore]
fn calculix_native_loads() {
    for (name, du, dv) in ccx_cases(true) {
        assert!(du < 1e-3 && dv < 1e-3, "{name}: displacement {du:.2e}, von Mises {dv:.2e}");
    }
}
