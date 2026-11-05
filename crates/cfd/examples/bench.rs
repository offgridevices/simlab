//! Raw lattice throughput: bench <n> [fp16] [steps] — an n^3 periodic box
//! with a resting sphere (so wall records exist) and a gentle body force.
use simlab_cfd::lattice::*;
use simlab_cfd::voxel::*;
use std::time::Instant;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dims: Vec<u32> = a[0].split('x').map(|s| s.parse().unwrap()).collect();
    let dims = if dims.len() == 1 { [dims[0]; 3] } else { [dims[0], dims[1], dims[2]] };
    let storage = if a.iter().any(|s| s == "fp16") { Storage::Fp16 } else { Storage::Fp32 };
    let steps: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    let c = [dims[0] as f64 / 2.0, dims[1] as f64 / 2.0, dims[2] as f64 / 2.0];
    let r = dims.iter().copied().min().unwrap() as f64 / 8.0;
    let m = simlab_cfd::shapes::sphere(c, r, 4);
    let vox = voxelize_static(&Surface { verts: m.positions.clone(), tris: m.triangles.clone() }, [dims[0] as usize, dims[1] as usize, dims[2] as usize]);
    let st = LatticeSetup { dims, faces: [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open], nu: 0.001, smagorinsky: 0.1, u_inf: [0.05, 0.0, 0.0], u_init: [0.05, 0.0, 0.0], body_force: [0.0; 3], sponge: [8; 6], sponge_tau: 1.0, sponge_sigma: 0.1, absorb_velocity: false, total_pressure_inlet: true, storage, center: c, debug_dump: false , wall_model: std::env::var_os("WM").is_some()};
    let t0 = Instant::now();
    let mut lat = Lattice::new(st, Geometry::Static(vox), Probes::default())?;
    let setup = t0.elapsed().as_secs_f64();
    lat.run(&vec![StepIn::still(false); 20])?;
    let t1 = Instant::now();
    let h = lat.run(&vec![StepIn::still(false); steps])?;
    let dt = t1.elapsed().as_secs_f64();
    let n = dims.iter().map(|&d| d as f64).product::<f64>();
    println!("{:?} {:?}: {:.0} M cells, {:.2} GB, setup {:.1}s, {:.0} MLUPs (Fx {:.3})", dims, lat.setup.storage, n / 1e6, lat.memory_bytes as f64 / 1e9, setup, n * steps as f64 / dt / 1e6, h.last().unwrap()[0]);
    Ok(())
}
