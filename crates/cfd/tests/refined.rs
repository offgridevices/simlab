//! The refined D3Q27 cumulant solver used for still parts: second-order
//! walls, momentum bookkeeping, a uniform stream crossing refinement levels
//! untouched, and a refined sphere through the study API.

use simlab_cfd::lattice::{FaceBc, Storage};
use simlab_cfd::mlattice::{LevelBox, MGeometry, MProbes, MultiLattice, MultiSetup, StepIn};
use simlab_cfd::shapes::{cuboid, sphere};
use simlab_cfd::tunnel::SolverOptions;
use simlab_cfd::validation::{sphere_cd_cgw, sphere_cd_sn, sphere_drag};
use simlab_cfd::voxel::{Surface, voxelize_static27};
use simlab_core::Silent;

fn setup(dims0: [u32; 3], faces: [FaceBc; 6], nu0: f64, u: f64) -> MultiSetup {
    MultiSetup {
        dims0,
        boxes: vec![],
        faces,
        nu0,
        smagorinsky: 0.0,
        u_inf: [u, 0.0, 0.0],
        u_init: [u, 0.0, 0.0],
        body_force0: [0.0; 3],
        sponge: [0; 6],
        sponge_tau: 1.0,
        sponge_sigma: 0.0,
        absorb_velocity: false,
        total_pressure_inlet: false,
        storage: Storage::Fp32,
        center: [0.0; 3],
        debug_dump: true,
    }
}

fn poiseuille(h: f64) -> (f64, f64) {
    let ya = 1.37;
    let yb = ya + h;
    let ny = (yb.ceil() as u32) + 2;
    let (nx, nz) = (3u32, 3u32);
    // plate edges off the diagonals of the lattice, so no link grazes a triangle edge
    let lo = cuboid([-5.3, -10.0, -4.1], [nx as f64 + 4.6, ya, nz as f64 + 5.7]);
    let hi = cuboid([-5.3, yb, -4.1], [nx as f64 + 4.6, ny as f64 + 10.0, nz as f64 + 5.7]);
    let mut m = lo.clone();
    let off = m.positions.len() as u32;
    m.positions.extend_from_slice(&hi.positions);
    m.triangles.extend(hi.triangles.iter().map(|t| [t[0] + off, t[1] + off, t[2] + off]));
    let vox = voxelize_static27(&Surface { verts: m.positions.clone(), tris: m.triangles.clone() }, [nx as usize, ny as usize, nz as usize]);
    let nu = 0.1;
    let g = 0.02 * 8.0 * nu / (h * h);
    let mut s = setup([nx, ny, nz], [FaceBc::Periodic; 6], nu, 0.0);
    s.body_force0 = [g, 0.0, 0.0];
    let mut lat = MultiLattice::new(s, MGeometry::Static(vox), MProbes::default()).unwrap();
    let hist = lat.run(&vec![StepIn::still(false); (3.0 * h * h / nu) as usize]).unwrap();
    let mac = lat.macros(0).unwrap();
    let (mut e2, mut r2) = (0.0, 0.0);
    for y in 0..ny {
        let yf = y as f64;
        if yf <= ya || yf >= yb {
            continue;
        }
        let exact = g / (2.0 * nu) * (yf - ya) * (yb - yf);
        let u = mac[((ny + y) * nx + 1) as usize][1] as f64;
        e2 += (u - exact).powi(2);
        r2 += exact * exact;
    }
    let fluid: f64 = mac.iter().filter(|m| !m[0].is_nan()).map(|m| m[0] as f64).sum();
    ((e2 / r2).sqrt(), (hist.last().unwrap()[0] - g * fluid) / (g * fluid))
}

#[test]
fn d3q27_poiseuille_second_order() {
    let mut e = vec![];
    for h in [8.0, 16.0, 32.0] {
        let (err, bal) = poiseuille(h);
        eprintln!("D3Q27 Poiseuille H={h}: L2 {err:.3e}, force balance {bal:+.2e}");
        assert!(bal.abs() < 1e-3);
        e.push(err);
    }
    let order = (e[1] / e[2]).log2();
    eprintln!("observed order {order:.2}");
    assert!(order > 1.7 && e[2] < 1e-3);
}

#[test]
fn d3q27_momentum_balance() {
    let n = 40u32;
    let c = 20.0;
    let m = sphere([c + 0.3, c - 0.6, c + 0.17], 6.0, 5);
    let vox = voxelize_static27(&Surface { verts: m.positions, tris: m.triangles }, [n as usize; 3]);
    let mut lat = MultiLattice::new(setup([n; 3], [FaceBc::Periodic; 6], 0.05, 0.05), MGeometry::Static(vox), MProbes::default()).unwrap();
    let mom = |lat: &MultiLattice| -> f64 { lat.macros(0).unwrap().iter().filter(|m| !m[0].is_nan()).map(|m| (m[0] * m[1]) as f64).sum() };
    let p0 = mom(&lat);
    let h = lat.run(&vec![StepIn::still(false); 600]).unwrap();
    let p1 = mom(&lat);
    let imp: f64 = h.iter().map(|v| v[0]).sum();
    let err = (p0 - p1 - imp) / imp;
    eprintln!("D3Q27 momentum: lost {:.4}, impulse {:.4}, mismatch {err:+.2e}", p0 - p1, imp);
    assert!(err.abs() < 1e-3);
}

#[test]
fn uniform_stream_crosses_refinement_unchanged() {
    let mut s = setup([64, 48, 48], [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open], 0.01, 0.06);
    s.total_pressure_inlet = true;
    for l in 1..3 {
        let d = s.dims(l - 1);
        s.boxes.push(LevelBox { lo: [d[0] as i32 / 4, d[1] as i32 / 4, d[2] as i32 / 4], n: [d[0] / 2, d[1] / 2, d[2] / 2] });
    }
    let mut lat = MultiLattice::new(s, MGeometry::None, MProbes::default()).unwrap();
    let ss = lat.sub_steps();
    lat.run(&vec![StepIn::still(false); 100 * ss]).unwrap();
    for l in 0..3 {
        for v in lat.macros(l).unwrap() {
            if v[0].is_nan() {
                continue;
            }
            assert!((v[0] - 1.0).abs() < 1e-5 && (v[1] - 0.06).abs() < 1e-5 && v[2].abs() < 1e-6, "level {l}: {v:?}");
        }
    }
}

#[test]
fn refined_sphere_re100_quick() {
    // 48 cells across: two levels (24 on the coarse grid)
    let (cd, r) = sphere_drag(100.0, 48, 12.0, &SolverOptions::default(), &Silent).unwrap();
    let refc = 0.5 * (sphere_cd_cgw(100.0) + sphere_cd_sn(100.0));
    eprintln!("refined sphere Re 100: Cd {cd:.4} vs {refc:.4}, {} cells, {:.0} s", r.summary.cells, r.summary.seconds);
    assert!((cd / refc - 1.0).abs() < 0.03);
    assert!(r.surface_pressure_pa.iter().filter(|p| p.is_finite()).count() > r.surface_pressure_pa.len() * 9 / 10);
    let sl = &r.slices[0];
    assert!(sl.fields[0].data.iter().filter(|v| v.is_finite()).count() > sl.fields[0].data.len() / 2);
}
