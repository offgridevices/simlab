//! NACA 0012 section (extruded, periodic span) at an angle of attack: how
//! many cells per chord the solver needs for lift and drag.
//!   airfoil <cells per chord> <alpha deg> <Re> [passes]
use simlab_cfd::lattice::*;
use simlab_cfd::voxel::*;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c: f64 = a[0].parse()?;
    let alpha: f64 = a[1].parse::<f64>()?.to_radians();
    let re: f64 = a[2].parse()?;
    let passes: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(10.0);
    let smag: f64 = std::env::var("SMAG").ok().map(|v| v.parse().unwrap()).unwrap_or(0.1);
    let u = 0.075;
    let nu = u * c / re;
    let (nx, ny, nz) = ((12.0 * c) as u32, (8.0 * c) as u32, 4u32);
    let (x0, y0) = (4.0 * c, 4.0 * c);
    // section points, closed, rotated by alpha about the quarter chord
    let nsec = 200;
    let mut sec = vec![];
    for i in 0..nsec {
        let th = 2.0 * std::f64::consts::PI * i as f64 / nsec as f64;
        let xc = 0.5 * (1.0 + th.cos()); // 1 -> 0 -> 1
        let t = 0.6 * (0.2969 * xc.sqrt() - 0.1260 * xc - 0.3516 * xc * xc + 0.2843 * xc.powi(3) - 0.1036 * xc.powi(4));
        let yc = if th < std::f64::consts::PI { t } else { -t };
        let (px, py) = ((xc - 0.25) * c, yc * c);
        sec.push([x0 + px * alpha.cos() + py * alpha.sin(), y0 - px * alpha.sin() + py * alpha.cos()]);
    }
    let mut verts = vec![];
    for z in [-3.0, nz as f64 + 3.0] { for p in &sec { verts.push([p[0], p[1], z]); } }
    let mut tris = vec![];
    let n = nsec as u32;
    for i in 0..n { let j = (i + 1) % n; tris.push([i, j, n + j]); tris.push([i, n + j, n + i]); }
    for i in 1..n - 1 { tris.push([0, i + 1, i]); tris.push([n, n + i, n + i + 1]); }
    let mut m = simlab_core::TriMesh { positions: verts, triangles: tris, patch: vec![] };
    m.patch = vec![0; m.triangles.len()];
    simlab_cfd::shapes::fix_orientation(&mut m);
    let s = Surface { verts: m.positions.clone(), tris: m.triangles.clone() };
    let vox = voxelize_static(&s, [nx as usize, ny as usize, nz as usize]);
    let sp = (0.5 * c) as u32;
    let st = LatticeSetup { dims: [nx, ny, nz], faces: [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Periodic, FaceBc::Periodic], nu, smagorinsky: smag, u_inf: [u, 0.0, 0.0], u_init: [u, 0.0, 0.0], body_force: [0.0; 3], sponge: [sp, 2 * sp, sp, sp, 0, 0], sponge_tau: 1.0, sponge_sigma: 0.1, absorb_velocity: false, total_pressure_inlet: true, storage: Storage::Fp32, center: [x0, y0, 0.0], debug_dump: false , wall_model: std::env::var_os("WM").is_some()};
    let mut lat = Lattice::new(st, Geometry::Static(vox), Probes::default())?;
    let steps = (passes * c / u) as usize;
    let h = lat.run(&vec![StepIn::still(false); steps])?;
    let k = h.len() / 2;
    let (fx, fy) = h[k..].iter().fold((0.0, 0.0), |a, v| (a.0 + v[0], a.1 + v[1]));
    let q = 0.5 * u * u * c * nz as f64 * (h.len() - k) as f64;
    println!("NACA0012 c={c} alpha={:.1} Re={re}: Cl={:.4} Cd={:.4} (tau {:.5})", alpha.to_degrees(), fy / q, fx / q, 0.5 + 3.0 * nu);
    Ok(())
}
