//! Quick lattice checks: plane Poiseuille flow between plates placed off the
//! grid (Bouzidi walls, second-order convergence) and momentum balance.

use simlab_cfd::lattice::{FaceBc, Geometry, Lattice, LatticeSetup, Probes, StepIn, Storage};
use simlab_cfd::shapes::cuboid;
use simlab_cfd::voxel::{Surface, voxelize_static};
use simlab_core::TriMesh;

fn surface(m: &TriMesh) -> Surface {
    Surface { verts: m.positions.clone(), tris: m.triangles.clone() }
}

fn merge(a: &TriMesh, b: &TriMesh) -> TriMesh {
    let mut m = a.clone();
    let off = m.positions.len() as u32;
    m.positions.extend_from_slice(&b.positions);
    m.triangles.extend(b.triangles.iter().map(|t| [t[0] + off, t[1] + off, t[2] + off]));
    m.patch.extend_from_slice(&b.patch);
    m
}

fn setup(dims: [u32; 3], nu: f64, g: f64, storage: Storage) -> LatticeSetup {
    LatticeSetup {
        dims,
        faces: [FaceBc::Periodic; 6],
        nu,
        smagorinsky: 0.0,
        u_inf: [0.0; 3],
        u_init: [0.0; 3],
        body_force: [g, 0.0, 0.0],
        sponge: [0; 6],
        sponge_tau: 0.0, sponge_sigma: 0.0, absorb_velocity: false, total_pressure_inlet: false,
        storage,
        center: [0.0; 3],
        debug_dump: true, wall_model: false
    }
}

/// L2 error (relative) of plane Poiseuille flow of width `h` cells.
fn poiseuille(h: f64, storage: Storage, umax: f64) -> (f64, f64) {
    let ya = 1.37;
    let yb = ya + h;
    let ny = (yb.ceil() as u32) + 2;
    let (nx, nz) = (3u32, 3u32);
    let lo = cuboid([-5.0, -10.0, -5.0], [nx as f64 + 5.0, ya, nz as f64 + 5.0]);
    let hi = cuboid([-5.0, yb, -5.0], [nx as f64 + 5.0, ny as f64 + 10.0, nz as f64 + 5.0]);
    let m = merge(&lo, &hi);
    let vox = voxelize_static(&surface(&m), [nx as usize, ny as usize, nz as usize]);
    let nu = 0.1;
    let g = umax * 8.0 * nu / (h * h);
    let mut lat = Lattice::new(setup([nx, ny, nz], nu, g, storage), Geometry::Static(vox), Probes::default()).unwrap();
    let steps = (3.0 * h * h / nu) as usize;
    let hist = lat.run(&vec![StepIn::still(false); steps]).unwrap();
    let mac = lat.macros().unwrap();
    let (mut e2, mut r2) = (0.0, 0.0);
    for y in 0..ny {
        let yf = y as f64;
        if yf <= ya || yf >= yb {
            continue;
        }
        let exact = g / (2.0 * nu) * (yf - ya) * (yb - yf);
        let c = ((1 * ny + y) * nx + 1) as usize;
        let u = mac[c][1] as f64;
        if std::env::var_os("PROFILE").is_some() { eprintln!("y={y} u={u:.6e} exact={exact:.6e} rho={}", mac[c][0]); }
        e2 += (u - exact).powi(2);
        r2 += exact * exact;
    }
    // momentum balance: wall force = body force on all fluid
    let last = hist.last().unwrap();
    let fluid: f64 = mac.iter().filter(|m| !m[0].is_nan()).map(|m| m[0] as f64).sum();
    let bal = (last[0] - g * fluid) / (g * fluid);
    ((e2 / r2).sqrt(), bal)
}

#[test]
fn poiseuille_second_order() {
    let mut errs = vec![];
    for h in [8.0, 16.0, 32.0] {
        let (e, bal) = poiseuille(h, Storage::Fp32, 0.02);
        eprintln!("Poiseuille H={h}: L2 error {e:.3e}, force balance {bal:+.2e}");
        assert!(bal.abs() < 2e-3, "wall force does not balance the body force: {bal}");
        errs.push(e);
    }
    let o1 = (errs[0] / errs[1]).log2();
    let o2 = (errs[1] / errs[2]).log2();
    eprintln!("observed order {o1:.2}, {o2:.2}");
    assert!(errs[2] < 2e-3);
    assert!(o2 > 1.6 || errs[2] < 1e-4, "convergence order too low: {o1:.2} {o2:.2}");
}

#[test]
fn poiseuille_fp16() {
    // FP16 storage with stochastic rounding must match FP32 even in a slow,
    // weakly driven channel (plain rounding stalls here at ~2.5 % error)
    for umax in [0.02, 0.06] {
        let (e, bal) = poiseuille(16.0, Storage::Fp16, umax);
        let (e32, _) = poiseuille(16.0, Storage::Fp32, umax);
        eprintln!("Poiseuille FP16 H=16 umax={umax}: L2 error {e:.3e} (FP32 {e32:.3e}), balance {bal:+.2e}");
        assert!(e < 3e-3, "FP16 error {e}");
    }
}
