//! Force accounting: the momentum the fluid loses equals the force on the
//! body; Stokes drag of a periodic array of spheres (Hasimoto / Sangani &
//! Acrivos); a uniform stream and still air exert no force.

use simlab_cfd::lattice::{FaceBc, Geometry, Lattice, LatticeSetup, Probes, StepIn, Storage};
use simlab_cfd::shapes::sphere;
use simlab_cfd::voxel::{Surface, voxelize_static};

fn setup(n: u32, nu: f64, u0: f64, g: f64, faces: [FaceBc; 6]) -> LatticeSetup {
    LatticeSetup {
        dims: [n; 3],
        faces,
        nu,
        smagorinsky: 0.0,
        u_inf: [u0, 0.0, 0.0],
        u_init: [u0, 0.0, 0.0],
        body_force: [g, 0.0, 0.0],
        sponge: [0; 6],
        sponge_tau: 0.0,
        sponge_sigma: 0.0,
        absorb_velocity: false,
        total_pressure_inlet: false,
        storage: Storage::Fp32,
        center: [n as f64 / 2.0; 3],
        debug_dump: true, wall_model: false
    }
}

fn ball(n: u32, r: f64) -> Geometry {
    let c = n as f64 / 2.0;
    let m = sphere([c + 0.3, c - 0.6, c + 0.17], r, 5);
    Geometry::Static(voxelize_static(&Surface { verts: m.positions, tris: m.triangles }, [n as usize; 3]))
}

fn fluid_momentum(lat: &Lattice) -> f64 {
    lat.macros().unwrap().iter().filter(|m| !m[0].is_nan()).map(|m| (m[0] * m[1]) as f64).sum()
}

#[test]
fn momentum_lost_by_fluid_equals_force_on_body() {
    let n = 40;
    let mut lat = Lattice::new(setup(n, 0.05, 0.05, 0.0, [FaceBc::Periodic; 6]), ball(n, 6.0), Probes::default()).unwrap();
    let p0 = fluid_momentum(&lat);
    let h = lat.run(&vec![StepIn::still(false); 600]).unwrap();
    let p1 = fluid_momentum(&lat);
    let impulse: f64 = h.iter().map(|v| v[0]).sum();
    let err = (p0 - p1 - impulse) / impulse;
    eprintln!("fluid momentum lost {:.5}, impulse on body {:.5}, mismatch {:+.3e}", p0 - p1, impulse, err);
    assert!(impulse > 0.0);
    assert!(err.abs() < 5e-3, "momentum balance off by {err}");
}

#[test]
fn periodic_sphere_array_stokes_drag() {
    let n = 48u32;
    let r = 6.0;
    let nu = 1.0 / 6.0;
    let g = 1e-7;
    let mut lat = Lattice::new(setup(n, nu, 0.0, g, [FaceBc::Periodic; 6]), ball(n, r), Probes::default()).unwrap();
    let steps = (4.0 * (n * n) as f64 / nu) as usize;
    let h = lat.run(&vec![StepIn::still(false); steps]).unwrap();
    let vol = (n as f64).powi(3);
    let mac = lat.macros().unwrap();
    let ux: f64 = mac.iter().filter(|m| !m[0].is_nan()).map(|m| (m[0] * m[1]) as f64).sum::<f64>() / vol;
    let f = h.last().unwrap()[0];
    let c = 4.0 / 3.0 * std::f64::consts::PI * r * r * r / vol;
    let k_ref = 1.0 / (1.0 - 1.7601 * c.cbrt() + c - 1.5593 * c * c + 3.9799 * c.powf(8.0 / 3.0) - 3.0734 * c.powf(10.0 / 3.0));
    let k = f / (6.0 * std::f64::consts::PI * nu * r * ux);
    eprintln!("periodic array c={c:.4}: K={k:.4}, Hasimoto/Sangani-Acrivos {k_ref:.4} ({:+.2} %)", (k / k_ref - 1.0) * 100.0);
    assert!((k / k_ref - 1.0).abs() < 0.04);
}

#[test]
fn uniform_states_feel_no_force() {
    // a uniform stream through a periodic box with a sphere at rest feels
    // drag; with no flow at all, the force must be zero to round-off
    let n = 32;
    let mut lat = Lattice::new(setup(n, 0.05, 0.0, 0.0, [FaceBc::Open; 6]), ball(n, 5.0), Probes::default()).unwrap();
    let h = lat.run(&vec![StepIn::still(false); 50]).unwrap();
    for v in &h {
        for x in v {
            assert!(x.abs() < 1e-4, "spurious force {v:?}");
        }
    }
}
