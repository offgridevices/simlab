//! Voxelization checks (CPU, plus the GPU re-voxelizer of spinning parts).

use simlab_cfd::lattice::{FaceBc, Geometry, IDENTITY, Lattice, LatticeSetup, Probes, StepIn, Storage};
use simlab_cfd::shader::RS;
use simlab_cfd::shapes::{PropSpec, cuboid, sphere};
use simlab_cfd::voxel::{Grid, Surface, inside_grid, voxelize_spin, voxelize_static};
use simlab_core::TriMesh;

fn surf(m: &TriMesh) -> Surface {
    Surface { verts: m.positions.clone(), tris: m.triangles.clone() }
}

#[test]
fn sphere_volume_and_links() {
    let r = 10.3;
    let m = sphere([20.2, 19.7, 20.4], r, 5);
    let v = voxelize_static(&surf(&m), [41, 41, 41]);
    let exact = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
    let got = v.solid.len() as f64;
    assert!((got / exact - 1.0).abs() < 0.03, "solid cells {got} vs volume {exact}");
    // every wall link has a q in (0, 1]
    for w in &v.walls {
        for d in 1..19 {
            if w.mask & (1 << d) != 0 {
                assert!(w.q[d - 1] > 0.0 && w.q[d - 1] <= 1.0);
            }
        }
    }
    // wall links come in balanced opposite pairs (zero force on a uniform state)
    let mut net = [0i64; 3];
    for w in &v.walls {
        for d in 1..19 {
            if w.mask & (1 << d) != 0 {
                for k in 0..3 {
                    net[k] += simlab_cfd::shader::C[d][k] as i64;
                }
            }
        }
    }
    assert_eq!(net, [0, 0, 0]);
}

#[test]
fn holes_and_overlaps_do_not_flood() {
    // a box with two triangles removed (a hole) still fills its inside
    let mut m = cuboid([3.3, 3.3, 3.3], [16.7, 12.6, 14.2]);
    m.triangles.remove(0);
    m.triangles.remove(0);
    let g = Grid { origin: [0.0; 3], h: 1.0, dims: [20, 16, 18] };
    let ins = inside_grid(&surf(&m), &g);
    let n = ins.iter().filter(|&&b| b).count();
    let exact = (16 - 4 + 1) * (12 - 4 + 1) * (14 - 4 + 1);
    assert!((n as i64 - exact as i64).abs() <= exact as i64 / 20, "{n} vs {exact}");
    // two overlapping boxes: the overlap stays solid (non-zero winding)
    let a = cuboid([2.3, 2.3, 2.3], [10.6, 10.6, 10.6]);
    let b = cuboid([6.4, 6.4, 6.4], [14.7, 14.7, 14.7]);
    let mut u = a.clone();
    let off = u.positions.len() as u32;
    u.positions.extend_from_slice(&b.positions);
    u.triangles.extend(b.triangles.iter().map(|t| [t[0] + off, t[1] + off, t[2] + off]));
    let g = Grid { origin: [0.0; 3], h: 1.0, dims: [17, 17, 17] };
    let ins = inside_grid(&surf(&u), &g);
    assert!(ins[g.idx(8, 8, 8)], "overlap region must be solid");
}

#[test]
fn thin_plate_blocks_links() {
    // a plate 0.2 cells thick lying between cell centres: no solid cells,
    // but the links across it are walls from both sides
    let m = cuboid([2.0, 2.0, 5.4], [12.0, 12.0, 5.6]);
    let v = voxelize_static(&surf(&m), [15, 15, 12]);
    assert!(v.solid.is_empty());
    let up = v.walls.iter().filter(|w| w.mask & (1 << 5) != 0).count();
    let down = v.walls.iter().filter(|w| w.mask & (1 << 6) != 0).count();
    assert!(up > 50 && up == down, "{up} {down}");
}

#[test]
fn gpu_revoxelizer_matches_cpu() {
    // the spinning-part path at angle zero must find the same walls as the static one
    let spec = PropSpec::five_inch();
    let s = 1.0 / 0.0025; // 2.5 mm cells
    let m = spec.mesh().scaled(s);
    let n = 64u32;
    let c = [31.0, 32.0, 30.0];
    let body = Surface { verts: m.positions.clone(), tris: m.triangles.clone() };
    let lat_s = Surface { verts: m.positions.iter().map(|p| [p[0] + c[0], p[1] + c[1], p[2] + c[2]]).collect(), tris: m.triangles.clone() };
    let st = voxelize_static(&lat_s, [n as usize; 3]);
    let sp = voxelize_spin(&body, [0.0, 0.0, 1.0], 0.125);
    let r = sp.radius.ceil() as i32 + 3;
    let box_lo = [c[0] as i32 - r, c[1] as i32 - r, c[2] as i32 - 6];
    let box_n = [2 * r as u32 + 1, 2 * r as u32 + 1, 13];
    let setup = LatticeSetup {
        dims: [n; 3],
        faces: [FaceBc::Periodic; 6],
        nu: 0.05,
        smagorinsky: 0.0,
        u_inf: [0.0; 3],
        u_init: [0.0; 3],
        body_force: [0.0; 3],
        sponge: [0; 6],
        sponge_tau: 0.0,
        sponge_sigma: 0.0,
        absorb_velocity: false,
        total_pressure_inlet: false,
        storage: Storage::Fp32,
        center: c,
        debug_dump: false, wall_model: false
    };
    let mut lat = Lattice::new(setup, Geometry::Spin { vox: sp, box_lo, box_n }, Probes::default()).unwrap();
    lat.run(&[StepIn { rot: IDENTITY, rot_prev: IDENTITY, omega: [0.0; 3], sample: false }]).unwrap();
    let recs = lat.records().unwrap();
    let mut gpu: std::collections::HashMap<u32, (u32, Vec<f32>)> = std::collections::HashMap::new();
    for r in recs.chunks_exact(RS as usize) {
        if r[0] != 0 {
            let q: Vec<f32> = (0..9).flat_map(|k| {
                let w = r[2 + k];
                [half(w & 0xffff), half(w >> 16)]
            }).collect();
            gpu.insert(r[1], (r[0], q));
        }
    }
    let (mut same, mut diff, mut qerr, mut qbad) = (0usize, 0usize, 0f64, 0usize);
    let solid: std::collections::HashSet<u32> = st.solid.iter().copied().collect();
    let (mut only_cpu, mut only_gpu, mut cpu_thin, mut gpu_thin) = (0, 0, 0, 0);
    for w in &st.walls {
        match gpu.get(&w.cell) {
            Some((mask, q)) => {
                let common = mask & w.mask;
                for d in 1..19 {
                    let c = simlab_cfd::shader::C[d];
                    let nb = (w.cell as i64 + c[0] as i64 + (c[1] as i64) * n as i64 + (c[2] as i64) * (n * n) as i64) as u32;
                    let thin = !solid.contains(&nb);
                    if (w.mask & !mask) & (1 << d) != 0 { only_cpu += 1; if thin { cpu_thin += 1; } }
                    if (mask & !w.mask) & (1 << d) != 0 { only_gpu += 1; if thin { gpu_thin += 1; } }
                }
                diff += (mask ^ w.mask).count_ones() as usize;
                same += common.count_ones() as usize;
                for d in 1..19 {
                    if common & (1 << d) != 0 {
                        let e = (q[d - 1] - w.q[d - 1]).abs() as f64;
                        if e > 0.05 {
                            qbad += 1;
                        }
                        qerr = qerr.max(e);
                    }
                }
            }
            None => diff += w.mask.count_ones() as usize,
        }
    }
    eprintln!("only cpu {only_cpu} (thin {cpu_thin}), only gpu {only_gpu} (cpu-fluid neighbour {gpu_thin}), cpu walls missing on gpu: {}", st.walls.iter().filter(|w| !gpu.contains_key(&w.cell)).count());
    eprintln!("links: {same} agree, {diff} differ, max q difference {qerr:.4}, {qbad} differ by more than 0.05");
    // the spinning path classifies cells from a body-frame occupancy grid
    // 8x finer than the lattice, so cells within ~0.1 cell of the surface
    // may differ; thin-sheet links are not made reciprocal on the GPU
    assert!(diff as f64 <= 0.06 * same as f64, "{diff} of {same} links differ");
    // f32 ray tests can slip between two triangles on an edge (the f64 CPU
    // path does not); such a link falls back to q = 1
    assert!(qbad as f64 <= 0.002 * same as f64, "{qbad} links with different q");
}

fn half(h: u32) -> f32 {
    let e = (h >> 10) & 0x1f;
    let m = h & 0x3ff;
    if e == 0 {
        return m as f32 * 2f32.powi(-24);
    }
    (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15)
}
