//! Quick validation cases on structured meshes (run by `cargo test`).
mod common;
use common::*;
use simlab_core::{Load, Material, Silent, Support, TetMesh, Vec3};
use simlab_fea::assembly::Dirichlet;
use simlab_fea::mesh_gen::{Midside, box_mesh};
use simlab_fea::{Elastic, SolveOptions, SolverChoice, StrengthInput, postprocess, prepare_with_bc};
use std::f64::consts::PI;

fn mm(p: Vec3, s: [f64; 3]) -> Vec3 {
    [p[0] * s[0] * 1e-3, p[1] * s[1] * 1e-3, p[2] * s[2] * 1e-3]
}

fn distorted_cube(n: usize) -> TetMesh {
    box_mesh(
        n,
        n,
        n,
        |p| {
            let d = 0.25 / n as f64;
            let q = [
                p[0] + d * (PI * p[0] * n as f64).sin() * (2.0 * PI * p[1] + 1.0).sin(),
                p[1] + d * (PI * p[1] * n as f64).sin() * (3.0 * p[2]).cos(),
                p[2] + d * (PI * p[2] * n as f64).sin() * (5.0 * p[0]).sin(),
            ];
            mm(q, [10.0; 3])
        },
        Midside::Straight,
    )
}

fn aniso() -> Material {
    let mut m = Material::isotropic("petg-ish", "test", 1270.0, 2000.0, 0.38, 50.0);
    m.e_z_mpa = 1400.0;
    m.uts_z_mpa = 30.0;
    m
}

#[test]
fn patch_test_displacement() {
    // prescribed linear displacement on the boundary: interior must follow exactly
    let mesh = distorted_cube(3);
    let el = Elastic::new(2e9, 1.4e9, 0.35, 1000.0, 50e6, 30e6, [0.3, -0.5, 0.8]).unwrap();
    let a = [[1e-3, 2e-4, -3e-4], [-1e-4, -5e-4, 4e-4], [2.5e-4, 1e-4, 7e-4]];
    let lin = |x: Vec3| -> Vec3 { std::array::from_fn(|i| 1e-5 * (i as f64 + 1.0) + (0..3).map(|j| a[i][j] * x[j]).sum::<f64>()) };
    let mut bc = Dirichlet::new(3 * mesh.nodes.len());
    for f in &mesh.faces {
        for &v in f {
            let u = lin(mesh.nodes[v as usize]);
            for k in 0..3 {
                bc.set(3 * v as usize + k, u[k]);
            }
        }
    }
    let f = vec![0.0; 3 * mesh.nodes.len()];
    for choice in [SolverChoice::Direct, SolverChoice::Iterative] {
        let opts = SolveOptions { solver: choice, rel_tol: 1e-12, ..Default::default() };
        let (u, _) = simlab_fea::solver::solve(&mesh, &el, &bc, &f, &opts, &Silent, (0.0, 1.0)).unwrap();
        let mut err: f64 = 0.0;
        for (i, x) in mesh.nodes.iter().enumerate() {
            let w = lin(*x);
            for k in 0..3 {
                err = err.max((u[3 * i + k] - w[k]).abs());
            }
        }
        assert!(err < 1e-9 * 1e-5, "{choice:?}: displacement error {err:e}");
        let st = simlab_fea::recovery::recover(&mesh, &el, &u, false);
        let eps = [a[0][0], a[1][1], a[2][2], a[0][1] + a[1][0], a[1][2] + a[2][1], a[2][0] + a[0][2]];
        let sig: Vec<f64> = (0..6).map(|i| (0..6).map(|j| el.d[i][j] * eps[j]).sum()).collect();
        let smax = sig.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let mut serr: f64 = 0.0;
        for s in &st.nodal {
            for k in 0..6 {
                serr = serr.max((s[k] - sig[k]).abs());
            }
        }
        println!("patch test ({choice:?}): max displacement error {err:.2e} m, max stress error {:.2e} of max stress", serr / smax);
        assert!(serr < 1e-8 * smax, "{choice:?}: stress error {}", serr / smax);
    }
}

#[test]
fn patch_test_free_body_tension() {
    // equal and opposite pulls on x=0 and x=1: free-flying, uniform σxx = p everywhere
    let mesh = distorted_cube(3);
    let mat = aniso();
    let p = 1e6;
    let c = cfg(
        Support::FreeFlying,
        vec![
            Load::Pressure { patches: vec![1], pressure_pa: -p, label: String::new() },
            Load::Pressure { patches: vec![2], pressure_pa: -p, label: String::new() },
        ],
        [0.2, 0.4, 1.0],
    );
    let r = run(&mesh, &mat, &c, SolverChoice::Direct);
    let mut err: f64 = 0.0;
    for s in &r.stress {
        let want = [p, 0.0, 0.0, 0.0, 0.0, 0.0];
        for k in 0..6 {
            err = err.max((s[k] - want[k]).abs() / p);
        }
    }
    let ir = r.inertia_relief.as_ref().unwrap();
    println!("free-body patch test: max stress error {err:.2e}, relative reaction {:.2e}", ir.relative_reaction);
    assert!(err < 1e-8, "stress error {err}");
    assert!(ir.relative_reaction < 1e-9);
}

#[test]
fn cantilever_timoshenko() {
    let (l, b, h) = (100.0, 10.0, 10.0);
    let mesh = box_mesh(40, 4, 4, |p| mm(p, [l, b, h]), Midside::Mapped);
    let mat = steel();
    let pz = 100.0;
    let c = cfg(Support::Fixed { patches: vec![1] }, vec![Load::Force { patches: vec![2], force_n: [0.0, 0.0, -pz], label: String::new() }], [0.0, 0.0, 1.0]);
    let r = run(&mesh, &mat, &c, SolverChoice::Direct);
    let tip = nodes_where(&mesh, |x| (x[0] - l * 1e-3).abs() < 1e-9);
    let w = -tip.iter().map(|&i| r.displacement[i][2]).sum::<f64>() / tip.len() as f64;
    let (e, nu) = (210e9, 0.3);
    let g = e / (2.0 * (1.0 + nu));
    let i = b * h.powi(3) / 12.0 * 1e-12;
    let a = b * h * 1e-6;
    let kappa = 10.0 * (1.0 + nu) / (12.0 + 11.0 * nu);
    let lm = l * 1e-3;
    let want = pz * lm.powi(3) / (3.0 * e * i) + pz * lm / (kappa * a * g);
    println!("cantilever tip: got {:.5} mm, Timoshenko {:.5} mm, error {:.3} %", w * 1e3, want * 1e3, 100.0 * rel(w, want));
    assert!(rel(w, want) < 0.01);
    // bending stress at mid-span top fibre: M c / I
    let mid = nodes_where(&mesh, |x| (x[0] - 0.05).abs() < 1e-9 && (x[2] - h * 1e-3).abs() < 1e-9);
    let sx = mid.iter().map(|&i| r.stress[i][0]).sum::<f64>() / mid.len() as f64;
    let want_s = pz * 0.05 * (h / 2.0 * 1e-3) / i;
    println!("cantilever mid-span top stress: got {:.3} MPa, beam theory {:.3} MPa, error {:.3} %", sx / 1e6, want_s / 1e6, 100.0 * rel(sx, want_s));
    assert!(rel(sx, want_s) < 0.01);
    // reactions balance the load
    assert!((r.reaction_force_n[2] - pz).abs() < 1e-6 * pz, "reaction {:?}", r.reaction_force_n);

    // the iterative solver gives the same stresses
    let ri = run(&mesh, &mat, &c, SolverChoice::Iterative);
    let smax = r.von_mises.iter().fold(0.0f64, |m, v| m.max(*v));
    let mut d: f64 = 0.0;
    for (a, b) in r.stress.iter().zip(&ri.stress) {
        for k in 0..6 {
            d = d.max((a[k] - b[k]).abs());
        }
    }
    println!("iterative vs direct: max stress difference {:.2e} of peak, {} iterations", d / smax, ri.solver.iterations);
    assert!(d / smax < 1e-4);
}

fn elongation(mesh: &TetMesh, u: &[Vec3], l: f64) -> f64 {
    let a = nodes_where(mesh, |x| x[0].abs() < 1e-12);
    let b = nodes_where(mesh, |x| (x[0] - l).abs() < 1e-12);
    let m = |s: &[usize]| s.iter().map(|&i| u[i][0]).sum::<f64>() / s.len() as f64;
    m(&b) - m(&a)
}

#[test]
fn anisotropic_bar_ratio() {
    let l = 40.0;
    let mesh = box_mesh(8, 2, 2, |p| mm(p, [l, 10.0, 10.0]), Midside::Mapped);
    let mat = aniso();
    let p = 2e6;
    let loads = vec![
        Load::Pressure { patches: vec![1], pressure_pa: -p, label: String::new() },
        Load::Pressure { patches: vec![2], pressure_pa: -p, label: String::new() },
    ];
    // along the layers (print up = z) and across them (print up = x)
    let ra = run(&mesh, &mat, &cfg(Support::FreeFlying, loads.clone(), [0.0, 0.0, 1.0]), SolverChoice::Direct);
    let rb = run(&mesh, &mat, &cfg(Support::FreeFlying, loads, [1.0, 0.0, 0.0]), SolverChoice::Direct);
    let (da, db) = (elongation(&mesh, &ra.displacement, l * 1e-3), elongation(&mesh, &rb.displacement, l * 1e-3));
    let want = mat.e_xy_mpa / mat.e_z_mpa;
    println!("anisotropic bar: elongation ratio {:.6} (expected E_xy/E_z = {:.6}); along-layer strain {:.6e} vs {:.6e}", db / da, want, da / (l * 1e-3), p / 2e9);
    assert!(rel(db / da, want) < 1e-9);
    assert!(rel(da / (l * 1e-3), p / 2e9) < 1e-9);
    // Tsai-Hill: across layers SF = Z/σ, along layers SF = X/σ
    println!("safety factors: along {:.6} (want {:.6}), across {:.6} (want {:.6})", ra.safety_factor, 50e6 / p, rb.safety_factor, 30e6 / p);
    assert!(rel(ra.safety_factor, 50e6 / p) < 1e-8);
    assert!(rel(rb.safety_factor, 30e6 / p) < 1e-8);
}

#[test]
fn inertia_relief_free_bar() {
    // a free bar pulled at x = 0 accelerates; axial stress falls linearly to 0 at x = L
    let (l, b) = (100.0, 10.0);
    let mesh = box_mesh(20, 2, 2, |p| mm(p, [l, b, b]), Midside::Mapped);
    let mat = steel();
    let f = 1000.0;
    let c = cfg(Support::FreeFlying, vec![Load::Force { patches: vec![1], force_n: [-f, 0.0, 0.0], label: String::new() }], [0.0, 0.0, 1.0]);
    let r = run(&mesh, &mat, &c, SolverChoice::Direct);
    let a = b * b * 1e-6;
    let mut err: f64 = 0.0;
    for (i, x) in mesh.nodes.iter().enumerate() {
        let want = f * (1.0 - x[0] / (l * 1e-3)) / a;
        err = err.max((r.stress[i][0] - want).abs() / (f / a));
        for k in 1..6 {
            err = err.max(r.stress[i][k].abs() / (f / a));
        }
    }
    let ir = r.inertia_relief.as_ref().unwrap();
    let want_acc = -f / (7850.0 * a * l * 1e-3);
    println!(
        "inertia relief bar: max stress error {err:.2e} of F/A, acceleration {:.4} m/s² (want {:.4}), relative support reaction {:.2e}",
        ir.accel_m_s2[0], want_acc, ir.relative_reaction
    );
    assert!(err < 1e-8);
    assert!(rel(ir.accel_m_s2[0], want_acc) < 1e-10);
    assert!(ir.relative_reaction < 1e-9);
}

#[test]
fn gravity_free_body_has_no_stress() {
    let mesh = distorted_cube(3);
    let c = cfg(Support::FreeFlying, vec![Load::Acceleration { g: [0.3, -0.2, -1.0], label: String::new() }], [0.0, 0.0, 1.0]);
    let r = run(&mesh, &steel(), &c, SolverChoice::Direct);
    let scale = 7850.0 * 9.80665 * 0.01; // ρ g L
    let vm = r.von_mises.iter().fold(0.0f64, |m, v| m.max(*v));
    println!("gravity-only free body: max von Mises {:.2e} of ρgL", vm / scale);
    assert!(vm < 1e-8 * scale);
    assert!(r.safety_factor > 1e6);
}

#[test]
fn rotating_bar() {
    // bar from -L/2..L/2 spinning about z through its centre: σ = ρω²(L²/4 − x²)/2
    let (l, b) = (100.0, 4.0);
    let mesh = box_mesh(50, 2, 2, |p| mm([p[0] - 0.5, p[1] - 0.5, p[2] - 0.5], [l, b, b]), Midside::Mapped);
    let mat = Material::isotropic("petg", "petg", 1270.0, 1700.0, 0.38, 50.0);
    let rpm = 20000.0;
    let c = cfg(Support::FreeFlying, vec![Load::Spin { rpm, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3], label: String::new() }], [0.0, 0.0, 1.0]);
    let r = run(&mesh, &mat, &c, SolverChoice::Direct);
    let w = rpm * 2.0 * PI / 60.0;
    let lm = l * 1e-3;
    let mut worst: f64 = 0.0;
    let mid = |x: Vec3| x[1].abs() < 1e-12 && x[2].abs() < 1e-12;
    for (i, x) in mesh.nodes.iter().enumerate() {
        if mid(*x) && x[0].abs() < 0.4 * lm {
            let want = 1270.0 * w * w * (lm * lm / 4.0 - x[0] * x[0]) / 2.0;
            worst = worst.max(rel(r.stress[i][0], want));
        }
    }
    let center = 1270.0 * w * w * lm * lm / 8.0;
    let c0 = nodes_where(&mesh, |x| x[0].abs() < 1e-12 && mid(x))[0];
    println!("rotating bar: centre stress {:.4} MPa vs {:.4} MPa (error {:.3} %), worst |x|<0.4L error {:.3} %", r.stress[c0][0] / 1e6, center / 1e6, 100.0 * rel(r.stress[c0][0], center), 100.0 * worst);
    assert!(worst < 0.02);
    assert!(r.break_rpm.is_some());
    // spin-only: break rpm = rpm·√SF
    assert!(rel(r.break_rpm.unwrap(), rpm * r.safety_factor.sqrt()) < 1e-12);
}

fn hoop_radial(s: &[f64; 6], x: Vec3) -> (f64, f64) {
    let t = x[1].atan2(x[0]);
    let (c, sn) = (t.cos(), t.sin());
    let srr = s[0] * c * c + s[1] * sn * sn + 2.0 * s[3] * sn * c;
    let stt = s[0] * sn * sn + s[1] * c * c - 2.0 * s[3] * sn * c;
    (stt, srr)
}

#[test]
fn lame_thick_cylinder() {
    // quarter annulus, plane strain, internal pressure
    let (a, b, t, p) = (10.0, 20.0, 2.0, 10e6);
    let mesh = box_mesh(
        12,
        16,
        1,
        |q| {
            let r = a + (b - a) * q[0];
            let th = 0.5 * PI * q[1];
            [r * th.cos() * 1e-3, r * th.sin() * 1e-3, t * q[2] * 1e-3]
        },
        Midside::Mapped,
    );
    let mat = steel();
    let c = cfg(Support::FreeFlying, vec![Load::Pressure { patches: vec![1], pressure_pa: p, label: String::new() }], [0.0, 0.0, 1.0]);
    let input = StrengthInput { mesh: &mesh, material: &mat, config: &c, model_unit_m: 0.001, face_tractions: None };
    let mut bc = Dirichlet::new(3 * mesh.nodes.len());
    for (f, &pt) in mesh.faces.iter().zip(&mesh.face_patch) {
        for &v in f {
            let v = v as usize;
            match pt {
                3 => bc.set(3 * v + 1, 0.0), // y = 0 plane
                4 => bc.set(3 * v, 0.0),     // x = 0 plane
                5 | 6 => bc.set(3 * v + 2, 0.0),
                _ => {}
            }
        }
    }
    let prep = prepare_with_bc(&input, bc).unwrap();
    let (u, rep) = simlab_fea::solver::solve(&mesh, &prep.elastic, &prep.bc, &prep.f, &SolveOptions::default(), &Silent, (0.0, 1.0)).unwrap();
    let r = postprocess(&input, prep, u, rep).unwrap();
    let k = p * a * a / (b * b - a * a);
    let mut worst: f64 = 0.0;
    let mut at_a = (0.0, 0);
    for (i, x) in mesh.nodes.iter().enumerate() {
        let rr = (x[0] * x[0] + x[1] * x[1]).sqrt() * 1e3;
        let (stt, srr) = hoop_radial(&r.stress[i], *x);
        let want_t = k * (1.0 + b * b / (rr * rr));
        let want_r = k * (1.0 - b * b / (rr * rr));
        let want_z = 2.0 * 0.3 * k;
        let peak = p * (b * b + a * a) / (b * b - a * a);
        worst = worst.max((stt - want_t).abs() / peak).max((srr - want_r).abs() / peak).max((r.stress[i][2] - want_z).abs() / peak);
        if (rr - a).abs() < 1e-9 {
            at_a.0 += stt;
            at_a.1 += 1;
        }
    }
    let hoop_a = at_a.0 / at_a.1 as f64;
    let want = p * (b * b + a * a) / (b * b - a * a);
    println!("Lamé: bore hoop {:.4} MPa vs {:.4} MPa (error {:.3} %); worst field error {:.3} % of the peak", hoop_a / 1e6, want / 1e6, 100.0 * rel(hoop_a, want), 100.0 * worst);
    assert!(rel(hoop_a, want) < 0.01);
    assert!(worst < 0.01);
}

#[test]
fn rotating_annular_disk() {
    // thin annulus, free-flying, spinning about its axis: bore hoop stress (plane stress)
    let (a, b, t) = (5.0, 40.0, 1.0);
    let mesh = box_mesh(
        16,
        64,
        1,
        |q| {
            // grade radially towards the bore
            let s = q[0] * q[0] * 0.5 + q[0] * 0.5;
            let r = a + (b - a) * s;
            let th = 2.0 * PI * q[1];
            [r * th.cos() * 1e-3, r * th.sin() * 1e-3, t * (q[2] - 0.5) * 1e-3]
        },
        Midside::Mapped,
    );
    let mat = Material::isotropic("petg", "petg", 1270.0, 1700.0, 0.38, 50.0);
    let rpm = 30000.0;
    let c = cfg(Support::FreeFlying, vec![Load::Spin { rpm, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3], label: String::new() }], [0.0, 0.0, 1.0]);
    let r = run(&mesh, &mat, &c, SolverChoice::Direct);
    let w = rpm * 2.0 * PI / 60.0;
    let (am, bm, nu, rho) = (a * 1e-3, b * 1e-3, 0.38, 1270.0);
    let want_bore = rho * w * w / 4.0 * ((3.0 + nu) * bm * bm + (1.0 - nu) * am * am);
    let bore = nodes_where(&mesh, |x| ((x[0] * x[0] + x[1] * x[1]).sqrt() - am).abs() < 1e-9);
    let got = bore.iter().map(|&i| hoop_radial(&r.stress[i], mesh.nodes[i]).0).sum::<f64>() / bore.len() as f64;
    // mid-radius field check
    let mut worst: f64 = 0.0;
    for (i, x) in mesh.nodes.iter().enumerate() {
        let rr = (x[0] * x[0] + x[1] * x[1]).sqrt();
        let (stt, srr) = hoop_radial(&r.stress[i], *x);
        let wt = rho * w * w / 8.0 * ((3.0 + nu) * (am * am + bm * bm + am * am * bm * bm / (rr * rr)) - (1.0 + 3.0 * nu) * rr * rr);
        let wr = (3.0 + nu) / 8.0 * rho * w * w * (am * am + bm * bm - am * am * bm * bm / (rr * rr) - rr * rr);
        worst = worst.max((stt - wt).abs().max((srr - wr).abs()) / want_bore);
    }
    let ir = r.inertia_relief.as_ref().unwrap();
    println!(
        "rotating disk: bore hoop {:.4} MPa vs {:.4} MPa (error {:.3} %), worst field error {:.3} % of peak, relative reaction {:.1e}",
        got / 1e6,
        want_bore / 1e6,
        100.0 * rel(got, want_bore),
        100.0 * worst,
        ir.relative_reaction
    );
    assert!(rel(got, want_bore) < 0.02);
    assert!(worst < 0.02);
}

#[test]
fn memory_estimate_is_sane() {
    let e = simlab_fea::estimate_memory(100_000, 70_000);
    assert!(e.dofs == 300_000 && e.direct_bytes > e.iterative_bytes / 4 && e.direct_bytes < 64 << 30);
}

#[test]
fn stress_stiffening_tension_beam() {
    // cantilever under end tension T and tip load P: δ/δ_linear = 3(kL − tanh kL)/(kL)³, k = √(T/EI)
    let (l, b) = (200.0, 10.0);
    let mesh = box_mesh(40, 3, 3, |p| mm(p, [l, b, b]), Midside::Mapped);
    let mat = steel();
    let (e, i) = (210e9, (b * 1e-3).powi(4) / 12.0);
    let kl = 2.0;
    let t = kl * kl * e * i / (l * 1e-3).powi(2);
    let p = 10.0;
    let loads = vec![Load::Force { patches: vec![2], force_n: [t, 0.0, -p], label: String::new() }];
    let c = cfg(Support::Fixed { patches: vec![1] }, loads, [0.0, 0.0, 1.0]);
    let input = StrengthInput { mesh: &mesh, material: &mat, config: &c, model_unit_m: 0.001, face_tractions: None };
    let tip = |r: &simlab_fea::StrengthResult| {
        let n = nodes_where(&mesh, |x| (x[0] - l * 1e-3).abs() < 1e-9);
        -n.iter().map(|&k| r.displacement[k][2]).sum::<f64>() / n.len() as f64
    };
    let mut res = vec![];
    for s in [simlab_fea::Stiffening::Off, simlab_fea::Stiffening::On] {
        let opts = SolveOptions { stiffening: s, ..Default::default() };
        res.push(tip(&simlab_fea::solve_strength(&input, &opts, &Silent).unwrap()));
    }
    let want = 3.0 * (kl - kl.tanh()) / kl.powi(3);
    println!("stress stiffening: deflection ratio {:.4} vs beam theory {:.4} (error {:.2} %)", res[1] / res[0], want, 100.0 * rel(res[1] / res[0], want));
    assert!(rel(res[1] / res[0], want) < 0.02);
}

#[test]
fn boundary_faces_helper() {
    // closed box: areas sum to the surface, normals point away from the centre
    let mesh = box_mesh(3, 2, 2, |p| mm(p, [30.0, 20.0, 10.0]), Midside::Mapped);
    let f = simlab_fea::boundary_faces(&mesh);
    assert_eq!(f.len(), mesh.faces.len());
    let area: f64 = f.iter().map(|x| x.2).sum();
    let want = 2.0 * (30.0 * 20.0 + 30.0 * 10.0 + 20.0 * 10.0) * 1e-6;
    assert!(rel(area, want) < 1e-12);
    let c = [0.015, 0.01, 0.005];
    for (p, n, _) in &f {
        assert!(simlab_core::dot(*n, simlab_core::sub(*p, c)) > 0.0);
    }
    // the closed surface's normals integrate to zero
    let s = f.iter().fold([0.0; 3], |s, (_, n, a)| simlab_core::add(s, simlab_core::scale(*n, *a)));
    assert!(simlab_core::norm(s) < 1e-15);
}
