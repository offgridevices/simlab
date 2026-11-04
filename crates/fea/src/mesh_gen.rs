//! Structured tet10 meshes of mapped boxes, for tests and validation.
//!
//! The unit cube [0,1]³ is cut into nx×ny×nz cells, each split into 6
//! tetrahedra (Kuhn/Freudenthal, conforming between cells), then mapped
//! through `map`. Boundary faces get patches 1..6 for the cube sides
//! x=0, x=1, y=0, y=1, z=0, z=1. Nodes that map to the same point are
//! merged and faces that become interior are dropped, so a periodic map
//! (an annulus, a full disk ring) works.

use simlab_core::{TetMesh, Vec3};
use std::collections::HashMap;

/// How midside nodes are placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Midside {
    /// mapped like the corners (curved elements follow a curved map)
    Mapped,
    /// average of the two mapped corners (straight-sided elements)
    Straight,
}

pub fn box_mesh<F: Fn(Vec3) -> Vec3>(nx: usize, ny: usize, nz: usize, map: F, mid: Midside) -> TetMesh {
    let (gx, gy) = (2 * nx + 1, 2 * ny + 1);
    let gid = |i: usize, j: usize, k: usize| (k * gy + j) * gx + i;
    let param = |i: usize, j: usize, k: usize| [i as f64 / (2 * nx) as f64, j as f64 / (2 * ny) as f64, k as f64 / (2 * nz) as f64];
    let mut tets_g: Vec<[usize; 10]> = Vec::new();
    let perms = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let edges = crate::elem::TET_EDGES;
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                for p in &perms {
                    let mut c = [[2 * i, 2 * j, 2 * k]; 4];
                    for s in 0..3 {
                        c[s + 1] = c[s];
                        c[s + 1][p[s]] += 2;
                    }
                    // orientation in parameter space (map assumed orientation-preserving)
                    let d = |a: [usize; 3], b: [usize; 3]| [b[0] as f64 - a[0] as f64, b[1] as f64 - a[1] as f64, b[2] as f64 - a[2] as f64];
                    let vol = simlab_core::dot(d(c[0], c[1]), simlab_core::cross(d(c[0], c[2]), d(c[0], c[3])));
                    if vol < 0.0 {
                        c.swap(1, 2);
                    }
                    let mut t = [0usize; 10];
                    for a in 0..4 {
                        t[a] = gid(c[a][0], c[a][1], c[a][2]);
                    }
                    for (e, &(a, b)) in edges.iter().enumerate() {
                        let m = [(c[a][0] + c[b][0]) / 2, (c[a][1] + c[b][1]) / 2, (c[a][2] + c[b][2]) / 2];
                        t[4 + e] = gid(m[0], m[1], m[2]);
                    }
                    tets_g.push(t);
                }
            }
        }
    }
    // positions of used grid points
    let mut pos: HashMap<usize, Vec3> = HashMap::new();
    let unflat = |g: usize| (g % gx, (g / gx) % gy, g / (gx * gy));
    for t in &tets_g {
        for a in 0..4 {
            pos.entry(t[a]).or_insert_with(|| {
                let (i, j, k) = unflat(t[a]);
                map(param(i, j, k))
            });
        }
    }
    for t in &tets_g {
        for (e, &(a, b)) in edges.iter().enumerate() {
            let g = t[4 + e];
            if pos.contains_key(&g) {
                continue;
            }
            let p = match mid {
                Midside::Mapped => {
                    let (i, j, k) = unflat(g);
                    map(param(i, j, k))
                }
                Midside::Straight => {
                    let (pa, pb) = (pos[&t[a]], pos[&t[b]]);
                    [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0]
                }
            };
            pos.insert(g, p);
        }
    }
    // merge coincident points
    let mut keys: Vec<usize> = pos.keys().copied().collect();
    keys.sort_unstable();
    let mut scale = 0.0f64;
    for p in pos.values() {
        for v in p {
            scale = scale.max(v.abs());
        }
    }
    let q = 1e-9 * scale.max(1e-30);
    let mut by_point: HashMap<[i64; 3], u32> = HashMap::new();
    let mut remap: HashMap<usize, u32> = HashMap::new();
    let mut nodes = Vec::new();
    for g in keys {
        let p = pos[&g];
        let key = [(p[0] / q).round() as i64, (p[1] / q).round() as i64, (p[2] / q).round() as i64];
        let id = *by_point.entry(key).or_insert_with(|| {
            nodes.push(p);
            (nodes.len() - 1) as u32
        });
        remap.insert(g, id);
    }
    let tets: Vec<[u32; 10]> = tets_g.iter().map(|t| std::array::from_fn(|a| remap[&t[a]])).collect();
    // boundary faces of the parameter cube
    let mut faces: Vec<([u32; 6], u32)> = Vec::new();
    for t in &tets_g {
        for fc in crate::topo::TET_FACES {
            let cs: Vec<(usize, usize, usize)> = fc.iter().map(|&a| unflat(t[a])).collect();
            let side = if cs.iter().all(|c| c.0 == 0) {
                1
            } else if cs.iter().all(|c| c.0 == 2 * nx) {
                2
            } else if cs.iter().all(|c| c.1 == 0) {
                3
            } else if cs.iter().all(|c| c.1 == 2 * ny) {
                4
            } else if cs.iter().all(|c| c.2 == 0) {
                5
            } else if cs.iter().all(|c| c.2 == 2 * nz) {
                6
            } else {
                0
            };
            if side == 0 {
                continue;
            }
            let [a, b, c] = fc;
            let mid = |x: usize, y: usize| {
                let e = edges.iter().position(|&(p, q)| (p == x && q == y) || (p == y && q == x)).unwrap();
                remap[&t[4 + e]]
            };
            faces.push(([remap[&t[a]], remap[&t[b]], remap[&t[c]], mid(a, b), mid(b, c), mid(c, a)], side));
        }
    }
    // drop faces that became interior after merging
    let mut count: HashMap<[u32; 3], usize> = HashMap::new();
    for (f, _) in &faces {
        let mut k = [f[0], f[1], f[2]];
        k.sort_unstable();
        *count.entry(k).or_default() += 1;
    }
    faces.retain(|(f, _)| {
        let mut k = [f[0], f[1], f[2]];
        k.sort_unstable();
        count[&k] == 1
    });
    TetMesh { nodes, tets, faces: faces.iter().map(|f| f.0).collect(), face_patch: faces.iter().map(|f| f.1).collect() }
}
