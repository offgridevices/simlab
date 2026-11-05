// Stokes torque on a rotating cube: moving geometry (re-voxelized each step)
// vs static voxelization with the same wall velocity (quasi-steady Stokes flow)
use simlab_cfd::lattice::*;
use simlab_cfd::voxel::*;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let half: f64 = a[0].parse()?;
    let ucorner: f64 = a[1].parse()?;
    let steps: usize = a[2].parse()?;
    let mode = a[3].as_str();
    let ang: f64 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(0.0);
    let n = 96u32;
    let nu = 1.0 / 6.0;
    let c = n as f64 / 2.0 - 0.5;
    let th: f64 = std::env::var("THICK").ok().map(|v| v.parse().unwrap()).unwrap_or(half);
    let m = simlab_cfd::shapes::cuboid([-half, -th * 0.5, -half * 0.3], [half, th * 0.5, half * 0.3]);
    let m = { let s = simlab_cfd::voxel::subdivide(&Surface { verts: m.positions.clone(), tris: m.triangles.clone() }, 1.0); s };
    let om = ucorner / (half * 2f64.sqrt());
    let rot = |t: f64| -> [[f64; 3]; 3] { [[t.cos(), -t.sin(), 0.0], [t.sin(), t.cos(), 0.0], [0.0, 0.0, 1.0]] };
    let mk = |geom| LatticeSetup { dims: [n; 3], faces: [FaceBc::Open; 6], nu, smagorinsky: 0.0, u_inf: [0.0; 3], u_init: [0.0; 3], body_force: [0.0; 3], sponge: [6; 6], sponge_tau: 1.0, sponge_sigma: 0.1, absorb_velocity: false, total_pressure_inlet: false, storage: Storage::Fp32, center: [c; 3], debug_dump: { let _ : &Geometry = geom; false }, wall_model: std::env::var_os("WM").is_some() };
    if mode == "static" {
        let r = rot(ang);
        let s = Surface { verts: m.verts.iter().map(|p| [c + r[0][0] * p[0] + r[0][1] * p[1], c + r[1][0] * p[0] + r[1][1] * p[1], c + p[2]]).collect(), tris: m.tris.clone() };
        let g = Geometry::Static(voxelize_static(&s, [n as usize; 3]));
        let mut lat = Lattice::new(mk(&g), g, Probes::default())?;
        let mut st = StepIn::still(false);
        st.omega = [0.0, 0.0, om];
        let h = lat.run(&vec![st; steps])?;
        let k = h.len() / 5;
        let mean: f64 = h[h.len() - k..].iter().map(|x| x[5]).sum::<f64>() / k as f64;
        println!("static angle {ang:.3}: Mz = {mean:.5e}");
    } else {
        let vox = voxelize_spin(&m, [0.0, 0.0, 1.0], 0.125);
        let r = (half * 1.5).ceil() as i32 + 3;
        let g = Geometry::Spin { vox, box_lo: [c as i32 - r, c as i32 - r, c as i32 - r], box_n: [2 * r as u32 + 2; 3] };
        let mut lat = Lattice::new(mk(&g), g, Probes::default())?;
        let mut th = 0.0;
        let mut prev = rot(0.0);
        let mut v = Vec::new();
        for _ in 0..steps { th += om; let rr = rot(th); v.push(StepIn { rot: rr, rot_prev: prev, omega: [0.0, 0.0, om], sample: false }); prev = rr; }
        let h = lat.run(&v)?;
        // average over the last quarter turn (cube symmetry period)
        let k = ((std::f64::consts::PI / 2.0) / om) as usize;
        let mean: f64 = h[h.len() - k..].iter().map(|x| x[5]).sum::<f64>() / k as f64;
        println!("moving: turned {:.2} rev, Mz(mean last quarter turn) = {mean:.5e}", th / std::f64::consts::TAU);
    }
    Ok(())
}
