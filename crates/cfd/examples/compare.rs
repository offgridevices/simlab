//! Old D3Q19 solver vs refined D3Q27 cumulant solver on still bluff bodies,
//! averaged over the same window of flow passes.
//!   compare <sphere|disk> <Re> <old cells per D> <new finest cells per D> <new levels> <t0 passes> <t1 passes>
use simlab_cfd::lattice::{FaceBc, Storage};
use simlab_cfd::mlattice::*;
use simlab_cfd::tunnel::{SolverOptions, run_with};
use simlab_cfd::voxel::{Surface, voxelize_static27};
use simlab_core::{Air, GridQuality, Silent, TriMesh, WindConfig};
use std::time::Instant;

fn body(kind: &str, center: [f64; 3], d: f64) -> TriMesh {
    match kind {
        "disk" => simlab_cfd::shapes::disk(center, d, 0.05 * d),
        _ => simlab_cfd::shapes::sphere(center, d / 2.0, 5),
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let kind = a[0].clone();
    let re: f64 = a[1].parse()?;
    let d_old: u32 = a[2].parse()?;
    let d_new: f64 = a[3].parse()?;
    let nl: usize = a[4].parse()?;
    let (t0, t1): (f64, f64) = (a[5].parse()?, a[6].parse()?);
    let u = 0.075;

    // ---- old solver through the study API
    let dphys = 0.05;
    let speed = 10.0;
    let cfg = WindConfig {
        wind_dir: [1.0, 0.0, 0.0],
        speed_m_s: speed,
        air: Air { density: 1.2, kinematic_viscosity: speed * dphys / re },
        spin: None,
        quality: GridQuality::Custom { cells_across: d_old },
        duration: Some(t1),
    };
    let m = body(&kind, [0.0; 3], dphys);
    let mut opt = SolverOptions::default();
    opt.tolerance = 1e-9;
    let tt = Instant::now();
    let r = run_with(&cfg, &m, 1e-3, &opt, &Silent)?;
    let q_old = 0.5 * 1.2 * speed * speed * std::f64::consts::PI * dphys * dphys / 4.0;
    let w: Vec<f64> = r.history.iter().filter(|h| h.0 > t0 && h.0 <= t1).map(|h| h.1[0]).collect();
    let cd_old = w.iter().sum::<f64>() / w.len() as f64 / q_old;
    let half = w.len() / 2;
    let cd_old_a = w[..half].iter().sum::<f64>() / half as f64 / q_old;
    let cd_old_b = w[half..].iter().sum::<f64>() / (w.len() - half) as f64 / q_old;
    let old_cost = r.summary.cells as f64 * r.summary.steps as f64;
    let old_s = tt.elapsed().as_secs_f64();

    // ---- new solver, same domain margins, nested boxes
    let d0 = d_new / (1u64 << (nl - 1)) as f64;
    let (up, down, side) = (2.5, 5.0, 1.5);
    let dims0 = [((up + down + 1.0) * d0).ceil() as u32 + 1, ((2.0 * side + 1.0) * d0).ceil() as u32 + 1, ((2.0 * side + 1.0) * d0).ceil() as u32 + 1];
    let c0 = [(up + 0.5) * d0, (dims0[1] - 1) as f64 / 2.0, (dims0[2] - 1) as f64 / 2.0];
    let mut s = MultiSetup {
        dims0,
        boxes: vec![],
        faces: [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open],
        nu0: u * d0 / re,
        smagorinsky: 0.0,
        u_inf: [u, 0.0, 0.0],
        u_init: [u, 0.0, 0.0],
        body_force0: [0.0; 3],
        sponge: [0; 6],
        sponge_tau: 1.0,
        sponge_sigma: 0.1,
        absorb_velocity: false,
        total_pressure_inlet: true,
        storage: Storage::Fp32,
        center: [0.0; 3],
        debug_dump: false,
    };
    let sp = (0.4 * d0).round().max(6.0) as u32;
    s.sponge = [sp, (1.0 * d0).round().max(8.0) as u32, sp, sp, sp, sp];
    let ext = [(1.2, 3.0, 1.0), (0.8, 1.6, 0.75), (0.65, 1.0, 0.65)];
    let mut cen = c0;
    for l in 1..nl {
        let dl = d0 * (1u64 << (l - 1)) as f64;
        let (eu, ed, es) = ext[(l - 1).min(2)];
        let lo = [(cen[0] - (0.5 + eu) * dl).floor() as i32, (cen[1] - (0.5 + es) * dl).floor() as i32, (cen[2] - (0.5 + es) * dl).floor() as i32];
        let hi = [(cen[0] + (0.5 + ed) * dl).ceil() as i32, (cen[1] + (0.5 + es) * dl).ceil() as i32, (cen[2] + (0.5 + es) * dl).ceil() as i32];
        s.boxes.push(LevelBox { lo, n: [(hi[0] - lo[0] + 1) as u32, (hi[1] - lo[1] + 1) as u32, (hi[2] - lo[2] + 1) as u32] });
        cen = [2.0 * (cen[0] - lo[0] as f64), 2.0 * (cen[1] - lo[1] as f64), 2.0 * (cen[2] - lo[2] as f64)];
    }
    let fd = s.dims(nl - 1);
    let mb = body(&kind, cen, d_new);
    let vox = voxelize_static27(&Surface { verts: mb.positions, tris: mb.triangles }, [fd[0] as usize, fd[1] as usize, fd[2] as usize]);
    s.center = cen;
    let tt = Instant::now();
    let mut lat = MultiLattice::new(s, MGeometry::Static(vox), MProbes::default())?;
    let ss = lat.sub_steps();
    let steps = ((t1 * d_new / u) as usize).div_ceil(ss) * ss;
    let h = lat.run(&vec![StepIn::still(false); steps])?;
    let q = 0.5 * u * u * std::f64::consts::PI * d_new * d_new / 4.0;
    let i0 = (t0 * d_new / u) as usize;
    let wn: Vec<f64> = h[i0.min(h.len() - 1)..].iter().map(|v| v[0]).collect();
    let cd_new = wn.iter().sum::<f64>() / wn.len() as f64 / q;
    let hn = wn.len() / 2;
    let cd_new_a = wn[..hn].iter().sum::<f64>() / hn as f64 / q;
    let cd_new_b = wn[hn..].iter().sum::<f64>() / (wn.len() - hn) as f64 / q;
    let new_cost: f64 = lat.level_cells().iter().enumerate().map(|(l, &n)| n as f64 * (1u64 << l) as f64).sum::<f64>() * (steps / ss) as f64;
    let new_s = tt.elapsed().as_secs_f64();
    println!(
        "{kind} Re={re} window {t0}-{t1} passes | old D3Q19 D={d_old}: Cd {cd_old:.4} (halves {cd_old_a:.4}/{cd_old_b:.4}) {:.0} G updates {:.0} s | new D3Q27 D={d_new} levels={nl}: Cd {cd_new:.4} (halves {cd_new_a:.4}/{cd_new_b:.4}) {:.0} G updates {:.0} s",
        old_cost / 1e9,
        old_s,
        new_cost / 1e9,
        new_s
    );
    Ok(())
}
