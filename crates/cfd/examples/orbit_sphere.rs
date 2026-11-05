// A small sphere orbiting far from the spin axis ≈ a sphere moving straight
// through still fluid: its drag must match the fixed sphere in a stream.
use simlab_cfd::lattice::*;
use simlab_cfd::voxel::*;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r: f64 = a[0].parse()?;
    let ro: f64 = a[1].parse()?;
    let u: f64 = a[2].parse()?;
    let re: f64 = a[3].parse()?;
    let steps: usize = a[4].parse()?;
    let fixed = a.get(5).map(|s| s == "fixed").unwrap_or(false);
    let nu = u * 2.0 * r / re;
    let nxy = (2.0 * (ro + r) + 40.0) as u32;
    let nz = (2.0 * r + 40.0) as u32;
    let c = [nxy as f64 / 2.0, nxy as f64 / 2.0, nz as f64 / 2.0];
    let base = LatticeSetup { dims: [nxy, nxy, nz], faces: [FaceBc::Open; 6], nu, smagorinsky: 0.0, u_inf: [0.0; 3], u_init: [0.0; 3], body_force: [0.0; 3], sponge: [10; 6], sponge_tau: 1.0, sponge_sigma: 0.1, absorb_velocity: false, total_pressure_inlet: false, storage: Storage::Fp32, center: c, debug_dump: true , wall_model: std::env::var_os("WM").is_some()};
    let stokes = 3.0 * std::f64::consts::PI * nu * 2.0 * r * u;
    if fixed {
        // fixed sphere in a uniform stream (same domain size, velocity inlet)
        let n2 = [nxy, (2.0 * r + 40.0) as u32 * 2, nz];
        let cc = [n2[0] as f64 / 2.0, n2[1] as f64 / 2.0, nz as f64 / 2.0];
        let spinpath = a.get(6).map(|s| s == "spin").unwrap_or(false);
        let geom = if spinpath {
            let m = simlab_cfd::shapes::sphere([0.0; 3], r, 5);
            let vox = voxelize_spin(&Surface { verts: m.positions, tris: m.triangles }, [0.0, 0.0, 1.0], 0.125);
            let b = r as i32 + 4;
            Geometry::Spin { vox, box_lo: [cc[0] as i32 - b, cc[1] as i32 - b, cc[2] as i32 - b], box_n: [2 * b as u32 + 1; 3] }
        } else {
            let m = simlab_cfd::shapes::sphere(cc, r, 5);
            Geometry::Static(voxelize_static(&Surface { verts: m.positions, tris: m.triangles }, [n2[0] as usize, n2[1] as usize, n2[2] as usize]))
        };
        let st = LatticeSetup { dims: n2, faces: [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open], u_inf: [u, 0.0, 0.0], u_init: [u, 0.0, 0.0], total_pressure_inlet: true, center: cc, ..base };
        let mut lat = Lattice::new(st, geom, Probes::default())?;
        for it in 0..10 {
            let h = lat.run(&vec![StepIn::still(false); steps / 10])?;
            println!("fixed step {}: F/F_stokes = {:.4}", (it + 1) * steps / 10, h.last().unwrap()[0] / stokes);
        }
        return Ok(());
    }
    let m = simlab_cfd::shapes::sphere([ro, 0.0, 0.0], r, 5);
    let s = Surface { verts: m.positions, tris: m.triangles };
    let vox = voxelize_spin(&s, [0.0, 0.0, 1.0], 0.125);
    let b = (ro + r + 3.0).ceil() as i32;
    let g = Geometry::Spin { vox, box_lo: [c[0] as i32 - b, c[1] as i32 - b, c[2] as i32 - r as i32 - 3], box_n: [2 * b as u32 + 1, 2 * b as u32 + 1, 2 * r as u32 + 7] };
    let mut lat = Lattice::new(base, g, Probes::default())?;
    let om = u / ro;
    let rot = |t: f64| -> [[f64; 3]; 3] { [[t.cos(), -t.sin(), 0.0], [t.sin(), t.cos(), 0.0], [0.0, 0.0, 1.0]] };
    let mut th = 0.0;
    let mut prev = rot(0.0);
    for it in 0..10 {
        let mut v = Vec::new();
        for _ in 0..steps / 10 { th += om; let rr = rot(th); v.push(StepIn { rot: rr, rot_prev: prev, omega: [0.0, 0.0, om], sample: false }); prev = rr; }
        let h = lat.run(&v)?;
        let l = h.last().unwrap();
        // drag = minus the force along the direction of motion (tangential)
        let (cs, sn) = (th.cos(), th.sin());
        let ft = -(l[0] * -sn + l[1] * cs);
        // mean tangential drag over the chunk (direction rotates slowly)
        let mut fm = 0.0;
        let mut thh = th - om * h.len() as f64;
        for x in &h { thh += om; fm += -(x[0] * -thh.sin() + x[1] * thh.cos()); }
        let fm = fm / h.len() as f64;
        println!("   mean drag/F_stokes over chunk {:.4}", fm / stokes);
        if std::env::var_os("SPIKES").is_some() && it == 1 {
            let mut thh = th - om * h.len() as f64;
            let mut line = String::new();
            for (j, x) in h.iter().enumerate() { thh += om; let d = -(x[0] * -thh.sin() + x[1] * thh.cos()) / stokes; if d.abs() > 5.0 { line += &format!(" {j}:{d:.0}"); } }
            println!("   spikes: {line}");
        }
        let k = h.len() / 2;
        let tz: f64 = h[k..].iter().map(|x| x[5]).sum::<f64>() / (h.len() - k) as f64;
        let mac = lat.macros()?;
        let mass: f64 = mac.iter().filter(|m| !m[0].is_nan()).map(|m| m[0] as f64 - 1.0).sum();
        let nf = mac.iter().filter(|m| !m[0].is_nan()).count();
        let (mut rmin, mut rmax) = (f32::MAX, f32::MIN);
        for m in &mac { if !m[0].is_nan() { rmin = rmin.min(m[0]); rmax = rmax.max(m[0]); } }
        println!("   excess mass {mass:.3}, fluid cells {nf}, rho [{rmin:.5}, {rmax:.5}]");
        println!("orbit step {} ({:.2} rev): drag/F_stokes = {:.4}, torque/ro/F_stokes = {:.4}", (it + 1) * steps / 10, th / std::f64::consts::TAU, ft / stokes, -tz / ro / stokes);
    }
    Ok(())
}
