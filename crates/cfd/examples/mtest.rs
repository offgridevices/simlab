//! Development checks for the multi-level D3Q27 solver.
//!   mtest poiseuille <H>
//!   mtest airfoil <cells per chord> <alpha> <Re> [passes] [levels]
//!   mtest uniform <levels>
use simlab_cfd::lattice::{FaceBc, Storage};
use simlab_cfd::mlattice::*;
use simlab_cfd::shapes::cuboid;
use simlab_cfd::voxel::*;

fn base(dims0: [u32; 3], faces: [FaceBc; 6], nu0: f64, u: f64) -> MultiSetup {
    MultiSetup {
        dims0,
        boxes: vec![],
        faces,
        nu0,
        smagorinsky: std::env::var("SMAG").ok().map(|v| v.parse().unwrap()).unwrap_or(0.0),
        u_inf: [u, 0.0, 0.0],
        u_init: [u, 0.0, 0.0],
        body_force0: [0.0; 3],
        sponge: [0; 6],
        sponge_tau: 1.0,
        sponge_sigma: 0.1,
        absorb_velocity: false,
        total_pressure_inlet: true,
        storage: if std::env::var_os("FP16").is_some() { Storage::Fp16 } else { Storage::Fp32 },
        center: [0.0; 3],
        debug_dump: true,
        wall_model: std::env::var_os("WM").is_some(),
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a[0].as_str() {
        "poiseuille" => {
            let h: f64 = a[1].parse()?;
            let ya: f64 = std::env::var("YA").ok().map(|v| v.parse().unwrap()).unwrap_or(1.37);
            let yb = ya + h;
            let ny = (yb.ceil() as u32) + 2;
            let (nx, nz) = (3u32, 3u32);
            let lo = cuboid([-5.3, -10.0, -4.1], [nx as f64 + 4.6, ya, nz as f64 + 5.7]);
            let hi = cuboid([-5.3, yb, -4.1], [nx as f64 + 4.6, ny as f64 + 10.0, nz as f64 + 5.7]);
            let mut m = lo.clone();
            let off = m.positions.len() as u32;
            m.positions.extend_from_slice(&hi.positions);
            m.triangles.extend(hi.triangles.iter().map(|t| [t[0] + off, t[1] + off, t[2] + off]));
            let vox = voxelize_static27(&Surface { verts: m.positions.clone(), tris: m.triangles.clone() }, [nx as usize, ny as usize, nz as usize]);
            let nu: f64 = std::env::var("NU").ok().map(|v| v.parse().unwrap()).unwrap_or(0.1);
            let g = 0.02 * 8.0 * nu / (h * h);
            let mut s = base([nx, ny, nz], [FaceBc::Periodic; 6], nu, 0.0);
            s.body_force0 = [g, 0.0, 0.0];
            s.u_inf = [0.0; 3];
            s.u_init = [0.0; 3];
            let mut lat = MultiLattice::new(s, MGeometry::Static(vox), MProbes::default())?;
            let mult: f64 = std::env::var("MULT").ok().map(|v| v.parse().unwrap()).unwrap_or(3.0);
            let steps = (mult * h * h / nu) as usize;
            let hst = lat.run(&vec![StepIn::still(false); steps])?;
            let mom = |lat: &MultiLattice| -> f64 { lat.macros(0).unwrap().iter().filter(|m| !m[0].is_nan()).map(|m| (m[0] * m[1]) as f64).sum() };
            let p1 = mom(&lat);
            let h2 = lat.run(&vec![StepIn::still(false); 100])?;
            let p2 = mom(&lat);
            let fsum: f64 = h2.iter().map(|v| v[0]).sum();
            println!("  dP over 100 steps {:.4e}, sum F {:.4e}", p2 - p1, fsum);
            let r = lat.records()?;
            let mut fx = 0.0f64;
            let mut cnt = [0u32; 27];
            for w in r.chunks_exact(24) {
                fx += f32::from_bits(w[15]) as f64;
                for i in 0..27 { if w[0] & (1 << i) != 0 { cnt[i] += 1; } }
            }
            println!("  records {} sum Fx {:.5e} last-step F {:.5e}; links per dir {:?}", r.len() / 24, fx, h2.last().unwrap()[0], cnt);
            let mac = lat.macros(0)?;
            let (mut e2, mut r2) = (0.0, 0.0);
            for y in 0..ny {
                let yf = y as f64;
                if yf <= ya || yf >= yb {
                    continue;
                }
                let exact = g / (2.0 * nu) * (yf - ya) * (yb - yf);
                let u = mac[((1 * ny + y) * nx + 1) as usize][1] as f64;
                e2 += (u - exact).powi(2);
                r2 += exact * exact;
            }
            let fluid: f64 = mac.iter().filter(|m| !m[0].is_nan()).map(|m| m[0] as f64).sum();
            println!("poiseuille H={h}: L2 {:.3e}, force balance {:+.2e}", (e2 / r2).sqrt(), (hst.last().unwrap()[0] - g * fluid) / (g * fluid));
        }
        "sphere" => {
            // sphere drag with nested refinement: D cells per diameter on the finest level
            let re: f64 = a[1].parse()?;
            let d_f: f64 = a[2].parse()?;
            let nl: usize = a[3].parse()?;
            let passes: f64 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(20.0);
            let u = 0.075;
            let d0 = d_f / (1u64 << (nl - 1)) as f64; // diameter in coarsest cells
            let (up, down, side) = (2.5, 5.0, 1.5);
            let dims0 = [((up + down + 1.0) * d0).ceil() as u32, ((2.0 * side + 1.0) * d0).ceil() as u32, ((2.0 * side + 1.0) * d0).ceil() as u32];
            let c0 = [(up + 0.5) * d0, dims0[1] as f64 / 2.0, dims0[2] as f64 / 2.0];
            let nu0 = u * d0 / re;
            let mut s = base(dims0, [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open], nu0, u);
            let sp = (0.4 * d0).round().max(6.0) as u32;
            s.sponge = [sp, (1.0 * d0).round().max(8.0) as u32, sp, sp, sp, sp];
            // nested boxes (in D): upstream, downstream, side half-widths per level
            let ext = [(1.2, 3.0, 1.0), (0.8, 1.6, 0.75), (0.65, 1.0, 0.65), (0.6, 0.8, 0.6)];
            let mut cen = c0; // sphere centre in current level coords
            for l in 1..nl {
                let sc = (1u64 << (l - 1)) as f64; // parent cells per coarsest cell
                let dl = d0 * sc; // diameter in parent cells
                let (eu, ed, es) = ext[(l - 1).min(3)];
                let lo = [(cen[0] - (0.5 + eu) * dl).floor() as i32, (cen[1] - (0.5 + es) * dl).floor() as i32, (cen[2] - (0.5 + es) * dl).floor() as i32];
                let hi = [(cen[0] + (0.5 + ed) * dl).ceil() as i32, (cen[1] + (0.5 + es) * dl).ceil() as i32, (cen[2] + (0.5 + es) * dl).ceil() as i32];
                s.boxes.push(LevelBox { lo, n: [(hi[0] - lo[0] + 1) as u32, (hi[1] - lo[1] + 1) as u32, (hi[2] - lo[2] + 1) as u32] });
                cen = [2.0 * (cen[0] - lo[0] as f64), 2.0 * (cen[1] - lo[1] as f64), 2.0 * (cen[2] - lo[2] as f64)];
            }
            let fd = s.dims(nl - 1);
            let r = d_f / 2.0;
            let m = simlab_cfd::shapes::sphere(cen, r, 5);
            let vox = voxelize_static27(&Surface { verts: m.positions, tris: m.triangles }, [fd[0] as usize, fd[1] as usize, fd[2] as usize]);
            s.center = cen;
            s.debug_dump = false;
            let mut lat = MultiLattice::new(s, MGeometry::Static(vox), MProbes::default())?;
            let ss = lat.sub_steps();
            let steps = ((passes * d_f / u) as usize).div_ceil(ss) * ss;
            let t0 = std::time::Instant::now();
            let h = lat.run(&vec![StepIn::still(false); steps])?;
            let q = 0.5 * u * u * std::f64::consts::PI * r * r;
            let half = h.len() / 2;
            let cd1 = h[h.len() / 4..half].iter().map(|v| v[0]).sum::<f64>() / (half - h.len() / 4) as f64 / q;
            let cd2 = h[half..].iter().map(|v| v[0]).sum::<f64>() / (h.len() - half) as f64 / q;
            let cl = h[half..].iter().map(|v| (v[1] * v[1] + v[2] * v[2]).sqrt()).sum::<f64>() / (h.len() - half) as f64 / q;
            let cells: u64 = lat.level_cells().iter().sum();
            println!(
                "sphere Re={re} D={d_f} levels={nl}: Cd={cd2:.4} (previous quarter {cd1:.4}) |Cl| {cl:.3} cells {:?} = {:.1} M, {:.0} s, {:.0} MLUPs(fine-eq)",
                lat.level_cells(), cells as f64 / 1e6, t0.elapsed().as_secs_f64(),
                lat.level_cells().iter().enumerate().map(|(l, &n)| n as f64 * (1u64 << l) as f64).sum::<f64>() * (steps / ss) as f64 / t0.elapsed().as_secs_f64() / 1e6
            );
        }
        "forcing" => {
            let mut s = base([8, 8, 8], [FaceBc::Periodic; 6], 0.1, 0.0);
            s.body_force0 = [1e-5, 0.0, 0.0];
            s.u_inf = [0.0; 3];
            s.u_init = [0.0; 3];
            let mut lat = MultiLattice::new(s, MGeometry::None, MProbes::default())?;
            let mom = |lat: &MultiLattice| -> (f64, f64) { let m = lat.macros(0).unwrap(); (m.iter().map(|m| (m[0] * m[1]) as f64).sum(), m.iter().map(|m| m[0] as f64).sum()) };
            let (p0, m0) = mom(&lat);
            lat.run(&vec![StepIn::still(false); 100])?;
            let (p1, _) = mom(&lat);
            println!("forcing: dP/step / (gM) = {:.6}", (p1 - p0) / 100.0 / (1e-5 * m0));
        }
        "momentum" => {
            let n = 40u32;
            let c = 20.0;
            let m = simlab_cfd::shapes::sphere([c + 0.3, c - 0.6, c + 0.17], 6.0, 5);
            let vox = voxelize_static27(&Surface { verts: m.positions, tris: m.triangles }, [n as usize; 3]);
            let s = base([n; 3], [FaceBc::Periodic; 6], 0.05, 0.05);
            let mut lat = MultiLattice::new(s, MGeometry::Static(vox), MProbes::default())?;
            let mom = |lat: &MultiLattice| -> f64 { lat.macros(0).unwrap().iter().filter(|m| !m[0].is_nan()).map(|m| (m[0] * m[1]) as f64).sum() };
            let p0 = mom(&lat);
            let h = lat.run(&vec![StepIn::still(false); 600])?;
            let p1 = mom(&lat);
            let imp: f64 = h.iter().map(|v| v[0]).sum();
            println!("momentum lost {:.5} impulse {:.5} mismatch {:+.3e}", p0 - p1, imp, (p0 - p1 - imp) / imp);
        }
        "uniform" => {
            let nl: usize = a[1].parse()?;
            let mut s = base([64, 48, 48], [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open], 0.01, 0.06);
            for l in 1..nl {
                let d = s.dims(l - 1);
                s.boxes.push(LevelBox { lo: [d[0] as i32 / 4, d[1] as i32 / 4, d[2] as i32 / 4], n: [d[0] / 2, d[1] / 2, d[2] / 2] });
            }
            let mut lat = MultiLattice::new(s, MGeometry::None, MProbes::default())?;
            let ss = lat.sub_steps();
            for it in 0..5 {
                lat.run(&vec![StepIn::still(false); 40 * ss])?;
                for l in 0..nl {
                    let m = lat.macros(l)?;
                    let (mut r0, mut r1, mut u0, mut u1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                    for v in &m {
                        if v[0].is_nan() {
                            continue;
                        }
                        r0 = r0.min(v[0]);
                        r1 = r1.max(v[0]);
                        u0 = u0.min(v[1]);
                        u1 = u1.max(v[1]);
                    }
                    println!("iter {it} level {l}: rho [{r0:.6}, {r1:.6}] ux [{u0:.6}, {u1:.6}]");
                }
            }
        }
        "airfoil" => {
            let c: f64 = a[1].parse()?;
            let alpha: f64 = a[2].parse::<f64>()?.to_radians();
            let re: f64 = a[3].parse()?;
            let passes: f64 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(10.0);
            let nl: usize = a.get(5).map(|s| s.parse().unwrap()).unwrap_or(1);
            let u = 0.075;
            // coarsest grid: chord c / 2^(nl-1) cells
            let cf = c / (1u64 << (nl - 1)) as f64;
            let span_c: f64 = std::env::var("SPAN").ok().map(|v| v.parse().unwrap()).unwrap_or(0.0);
            let (nx, ny, nz) = ((12.0 * cf) as u32, (8.0 * cf) as u32, if nl == 1 { ((span_c * cf) as u32).max(4) } else { 12 });
            let nu_f = u * c / re;
            let nu0 = nu_f / (1u64 << (nl - 1)) as f64;
            let faces = if nl == 1 {
                [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Periodic, FaceBc::Periodic]
            } else {
                [FaceBc::Velocity, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open, FaceBc::Open]
            };
            let mut s = base([nx, ny, nz], faces, nu0, u);
            let sp = (0.5 * cf) as u32;
            s.sponge = [sp, 2 * sp, sp, sp, 0, 0];
            // refinement boxes around the airfoil (x0 = 4 chords, y0 = 4 chords)
            for l in 1..nl {
                let d = s.dims(l - 1);
                let sc = (1u64 << (l - 1)) as f64; // parent cells per coarsest cell
                let cl = cf * sc; // chord in parent cells
                let mx = (0.5f64.max(1.5 / (l as f64))) * cl;
                let lo = [(4.0 * cl - 0.25 * cl - mx) as i32, (4.0 * cl - mx * 0.8) as i32, 2];
                let hi = [(4.0 * cl + 0.75 * cl + 1.5 * mx) as i32, (4.0 * cl + mx * 0.8) as i32, d[2] as i32 - 3];
                s.boxes.push(LevelBox { lo, n: [(hi[0] - lo[0] + 1) as u32, (hi[1] - lo[1] + 1) as u32, (hi[2] - lo[2] + 1) as u32] });
            }
            let fd = s.dims(nl - 1);
            // finest-level origin in finest units
            let mut org = [0f64; 3];
            let mut scl = (1u64 << (nl - 1)) as f64;
            for b in &s.boxes {
                for k in 0..3 {
                    org[k] += b.lo[k] as f64 * scl;
                }
                scl /= 2.0;
            }
            let (x0, y0) = (4.0 * c - org[0], 4.0 * c - org[1]);
            let nsec = 400;
            let mut sec = vec![];
            for i in 0..nsec {
                let th = 2.0 * std::f64::consts::PI * i as f64 / nsec as f64;
                let xc = 0.5 * (1.0 + th.cos());
                let t = 0.6 * (0.2969 * xc.sqrt() - 0.1260 * xc - 0.3516 * xc * xc + 0.2843 * xc.powi(3) - 0.1036 * xc.powi(4));
                let yc = if th < std::f64::consts::PI { t } else { -t };
                let (px, py) = ((xc - 0.25) * c, yc * c);
                sec.push([x0 + px * alpha.cos() + py * alpha.sin(), y0 - px * alpha.sin() + py * alpha.cos()]);
            }
            let span = fd[2] as f64;
            let (z0, z1) = if nl == 1 { (-3.0, span + 3.0) } else { (1.5, span - 2.5) };
            let mut verts = vec![];
            for z in [z0, z1] {
                for p in &sec {
                    verts.push([p[0], p[1], z]);
                }
            }
            let mut tris = vec![];
            let n = nsec as u32;
            for i in 0..n {
                let j = (i + 1) % n;
                tris.push([i, j, n + j]);
                tris.push([i, n + j, n + i]);
            }
            for i in 1..n - 1 {
                tris.push([0, i + 1, i]);
                tris.push([n, n + i, n + i + 1]);
            }
            let mut m = simlab_core::TriMesh { positions: verts, triangles: tris, patch: vec![] };
            m.patch = vec![0; m.triangles.len()];
            simlab_cfd::shapes::fix_orientation(&mut m);
            let vox = voxelize_static27(&Surface { verts: m.positions.clone(), tris: m.triangles.clone() }, [fd[0] as usize, fd[1] as usize, fd[2] as usize]);
            s.center = [x0, y0, 0.0];
            s.debug_dump = std::env::var_os("IMG").is_some();
            if std::env::var_os("LINKS").is_some() {
                let mut cnt = [0u32; 27];
                for w in &vox.walls { for i in 0..27 { if w.mask & (1 << i) != 0 { cnt[i] += 1; } } }
                // per z-layer counts
                let mut zc = vec![0u32; fd[2] as usize];
                for w in &vox.walls { zc[(w.cell / (fd[0] * fd[1])) as usize] += w.mask.count_ones(); }
                println!("solid {} walls {} per dir {:?} per z {:?}", vox.solid.len(), vox.walls.len(), cnt, zc);
            }
            let mut lat = MultiLattice::new(s, MGeometry::Static(vox), MProbes::default())?;
            let ss = lat.sub_steps();
            let steps = ((passes * c / u) as usize).div_ceil(ss) * ss;
            let t0 = std::time::Instant::now();
            let h = lat.run(&vec![StepIn::still(false); steps])?;
            let k = h.len() / 2;
            if let Ok(path) = std::env::var("IMG") {
                let m = lat.macros(nl - 1)?;
                let z = (fd[2] / 2) as usize;
                let mut raw: Vec<f32> = Vec::new();
                for y in 0..fd[1] as usize { for x in 0..fd[0] as usize { let v = m[(z * fd[1] as usize + y) * fd[0] as usize + x]; raw.push(v[1]); raw.push(v[2]); raw.push(v[0]); } }
                std::fs::write(&path, bytemuck::cast_slice(&raw))?;
                println!("image {} x {}", fd[0], fd[1]);
                let q1 = 0.5 * u * u * c * (if nl == 1 { fd[2] as f64 } else { 1.0 });
                let step = (h.len() / 30).max(1);
                for (i, v) in h.iter().enumerate().step_by(step) { println!("  t={:.2} Cl={:.3} Cd={:.3}", i as f64 * u / c, v[1] / q1, v[0] / q1); }
            }
            let (fx, fy) = h[k..].iter().fold((0.0, 0.0), |a, v| (a.0 + v[0], a.1 + v[1]));
            let spanw = if nl == 1 { span } else { z1 - z0 };
            let q = 0.5 * u * u * c * spanw * (h.len() - k) as f64;
            println!(
                "NACA0012 c={c} levels={nl} alpha={:.1} Re={re}: Cl={:.4} Cd={:.4} (cells {:?}, {:.0} s)",
                alpha.to_degrees(),
                fy / q,
                fx / q,
                lat.level_cells(),
                t0.elapsed().as_secs_f64()
            );
        }
        _ => anyhow::bail!("unknown"),
    }
    Ok(())
}
