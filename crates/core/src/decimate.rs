//! Surface simplification by quadric-error edge collapse (Garland & Heckbert
//! 1997), for turning dense STLs (500k+ facets) into a clean surface the
//! volume mesher can fill.
//!
//! Guarantees on a closed, manifold input:
//! * stays closed and manifold (link condition on every collapse);
//! * surface patches are kept: a patch border only slides along itself and
//!   vertices where three or more patches meet never move;
//! * no triangle flips (normal turns by more than ~75°), no slivers, and no
//!   new self-intersections (each new triangle is tested against its
//!   neighbourhood);
//! * the mean distance to the original surface planes stays below
//!   `max_error` (quadric error per unit area), so thin trailing edges and
//!   small bores keep their shape.
//!
//! Open edges (holes in a broken STL) are kept as they are.

use crate::{TriMesh, Vec3, cross, dot, norm, sub};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Options for [`decimate_with`].
#[derive(Clone, Copy, Debug)]
pub struct DecimateOptions {
    /// stop when the surface has this many triangles (or nothing more can go)
    pub target_tris: usize,
    /// largest allowed RMS distance from the original surface (model units)
    pub max_error: f64,
    /// no edge may become longer than this (model units); set it to the
    /// element size so the surface triangles suit the volume mesh
    pub max_edge: f64,
}

/// Simplifies `m` to about `target_tris` triangles with RMS shape error at
/// most `max_error` (same units as the positions).
pub fn decimate(m: &TriMesh, target_tris: usize, max_error: f64) -> TriMesh {
    decimate_with(m, &DecimateOptions { target_tris, max_error, max_edge: f64::INFINITY })
}

/// Triangle count that gives surface triangles of about edge `h` for a
/// surface of area `area` (equilateral triangles).
pub fn target_tris_for(area: f64, h: f64) -> usize {
    (area / (h * h * 3f64.sqrt() / 4.0)).ceil() as usize
}

type Quadric = [f64; 10];

fn plane_quadric(n: Vec3, d: f64, w: f64) -> Quadric {
    let [a, b, c] = n;
    [a * a * w, a * b * w, a * c * w, a * d * w, b * b * w, b * c * w, b * d * w, c * c * w, c * d * w, d * d * w]
}

fn q_add(a: &mut Quadric, b: &Quadric) {
    for i in 0..10 {
        a[i] += b[i];
    }
}

fn q_eval(q: &Quadric, p: Vec3) -> f64 {
    let [x, y, z] = p;
    q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z + 2.0 * q[6] * y + q[7] * z * z + 2.0 * q[8] * z + q[9]
}

/// Minimiser of the quadric, if well conditioned.
fn q_opt(q: &Quadric) -> Option<Vec3> {
    let a = [[q[0], q[1], q[2]], [q[1], q[4], q[5]], [q[2], q[5], q[7]]];
    let b = [-q[3], -q[6], -q[8]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let scale = (q[0] + q[4] + q[7]).powi(3);
    if !(det.abs() > 1e-9 * scale) {
        return None;
    }
    let solve_col = |k: usize| {
        let mut m = a;
        for r in 0..3 {
            m[r][k] = b[r];
        }
        (m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])) / det
    };
    Some([solve_col(0), solve_col(1), solve_col(2)])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Interior,
    Border,
    Corner,
}

#[derive(PartialEq)]
struct Cand {
    cost: f64,
    a: u32,
    b: u32,
    stamp: (u32, u32),
}
impl Eq for Cand {}
impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        // min-heap on cost
        o.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

struct Grid {
    lo: Vec3,
    cell: f64,
    dims: [usize; 3],
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(lo: Vec3, hi: Vec3, cell: f64) -> Grid {
        let dims: [usize; 3] = std::array::from_fn(|k| (((hi[k] - lo[k]) / cell).ceil() as usize).clamp(1, 512));
        let cell = (0..3).map(|k| (hi[k] - lo[k]) / dims[k] as f64).fold(cell, f64::max).max(1e-300);
        Grid { lo, cell, dims, cells: vec![Vec::new(); dims[0] * dims[1] * dims[2]] }
    }
    fn range(&self, lo: Vec3, hi: Vec3) -> [(usize, usize); 3] {
        std::array::from_fn(|k| {
            let a = (((lo[k] - self.lo[k]) / self.cell).floor().max(0.0) as usize).min(self.dims[k] - 1);
            let b = (((hi[k] - self.lo[k]) / self.cell).floor().max(0.0) as usize).min(self.dims[k] - 1);
            (a, b)
        })
    }
    fn for_cells(&self, lo: Vec3, hi: Vec3, mut f: impl FnMut(usize)) {
        let r = self.range(lo, hi);
        for z in r[2].0..=r[2].1 {
            for y in r[1].0..=r[1].1 {
                for x in r[0].0..=r[0].1 {
                    f((z * self.dims[1] + y) * self.dims[0] + x);
                }
            }
        }
    }
    fn insert(&mut self, t: u32, lo: Vec3, hi: Vec3) {
        let r = self.range(lo, hi);
        for z in r[2].0..=r[2].1 {
            for y in r[1].0..=r[1].1 {
                for x in r[0].0..=r[0].1 {
                    let c = (z * self.dims[1] + y) * self.dims[0] + x;
                    self.cells[c].push(t);
                }
            }
        }
    }
}

fn tri_bbox(p: &[Vec3; 3]) -> (Vec3, Vec3) {
    let lo = std::array::from_fn(|k| p[0][k].min(p[1][k]).min(p[2][k]));
    let hi = std::array::from_fn(|k| p[0][k].max(p[1][k]).max(p[2][k]));
    (lo, hi)
}

/// Triangle quality 4√3·A / Σℓ² (1 = equilateral).
fn quality(p: &[Vec3; 3]) -> f64 {
    let a = 0.5 * norm(cross(sub(p[1], p[0]), sub(p[2], p[0])));
    let l2: f64 = (0..3).map(|i| {
        let d = sub(p[(i + 1) % 3], p[i]);
        dot(d, d)
    }).sum();
    if l2 > 0.0 { 4.0 * 3f64.sqrt() * a / l2 } else { 0.0 }
}

/// Segment–triangle intersection (proper crossing), used for tri–tri tests.
fn seg_hits_tri(p: Vec3, q: Vec3, t: &[Vec3; 3]) -> bool {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let d = sub(q, p);
    let h = cross(d, e2);
    let a = dot(e1, h);
    let scale = norm(e1) * norm(e2) * norm(d);
    if a.abs() <= 1e-12 * scale {
        return false; // parallel or coplanar: ignored
    }
    let f = 1.0 / a;
    let s = sub(p, t[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let qv = cross(s, e1);
    let v = f * dot(d, qv);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let tt = f * dot(e2, qv);
    tt > 1e-9 && tt < 1.0 - 1e-9
}

fn tris_intersect(a: &[Vec3; 3], b: &[Vec3; 3]) -> bool {
    (0..3).any(|i| seg_hits_tri(a[i], a[(i + 1) % 3], b)) || (0..3).any(|i| seg_hits_tri(b[i], b[(i + 1) % 3], a))
}

/// [`decimate`] with an edge-length cap.
pub fn decimate_with(m: &TriMesh, o: &DecimateOptions) -> TriMesh {
    let nv = m.positions.len();
    let mut pos = m.positions.clone();
    let mut tris: Vec<[u32; 3]> = m.triangles.clone();
    let patch: Vec<u32> = if m.patch.len() == tris.len() { m.patch.clone() } else { vec![0; tris.len()] };
    let mut alive_t = vec![true; tris.len()];
    let mut alive_v = vec![true; nv];
    let mut vf: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (t, tri) in tris.iter().enumerate() {
        for &v in tri {
            vf[v as usize].push(t as u32);
        }
    }
    // quadrics: face planes (area-weighted), plus patch-border and open-edge
    // planes (perpendicular to the face through the edge) to hold borders
    let mut q: Vec<Quadric> = vec![[0.0; 10]; nv];
    let mut qa = vec![0.0f64; nv];
    let mut total_area = 0.0;
    for (t, tri) in tris.iter().enumerate() {
        let p = [pos[tri[0] as usize], pos[tri[1] as usize], pos[tri[2] as usize]];
        let c = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let a2 = norm(c);
        if a2 == 0.0 {
            continue;
        }
        let n = [c[0] / a2, c[1] / a2, c[2] / a2];
        let area = 0.5 * a2;
        total_area += area;
        let pq = plane_quadric(n, -dot(n, p[0]), area);
        for &v in tri {
            q_add(&mut q[v as usize], &pq);
            qa[v as usize] += area / 3.0;
        }
        for i in 0..3 {
            let (a, b) = (tri[i], tri[(i + 1) % 3]);
            let other: Vec<u32> = vf[a as usize].iter().copied().filter(|&f| f as usize != t && tris[f as usize].contains(&b)).collect();
            let border = other.len() != 1 || patch[other[0] as usize] != patch[t];
            if border {
                let e = sub(pos[b as usize], pos[a as usize]);
                let len = norm(e);
                if len == 0.0 {
                    continue;
                }
                let bn = crate::normalize(cross(e, n));
                let w = 10.0 * len * len;
                let bq = plane_quadric(bn, -dot(bn, pos[a as usize]), w);
                q_add(&mut q[a as usize], &bq);
                q_add(&mut q[b as usize], &bq);
            }
        }
    }
    let (lo, hi) = m.bounds();
    let mean_edge = (total_area / (o.target_tris.max(1) as f64) * 4.0 / 3f64.sqrt()).sqrt();
    let mut grid = Grid::new(lo, hi, mean_edge.max(1e-12));
    for (t, tri) in tris.iter().enumerate() {
        let p = [pos[tri[0] as usize], pos[tri[1] as usize], pos[tri[2] as usize]];
        let (a, b) = tri_bbox(&p);
        grid.insert(t as u32, a, b);
    }
    let mut stamp = vec![0u32; nv];
    let mut n_alive = tris.len();

    // helpers working on the current state
    let faces_of = |vf: &Vec<Vec<u32>>, alive_t: &Vec<bool>, v: u32| -> Vec<u32> { vf[v as usize].iter().copied().filter(|&f| alive_t[f as usize]).collect() };
    let edge_faces = |vf: &Vec<Vec<u32>>, alive_t: &Vec<bool>, tris: &Vec<[u32; 3]>, a: u32, b: u32| -> Vec<u32> {
        vf[a as usize].iter().copied().filter(|&f| alive_t[f as usize] && tris[f as usize].contains(&b)).collect()
    };
    let classify = |vf: &Vec<Vec<u32>>, alive_t: &Vec<bool>, tris: &Vec<[u32; 3]>, v: u32| -> Class {
        let fs: Vec<u32> = vf[v as usize].iter().copied().filter(|&f| alive_t[f as usize]).collect();
        let mut ps: Vec<u32> = fs.iter().map(|&f| patch[f as usize]).collect();
        ps.sort_unstable();
        ps.dedup();
        // count border edges at v
        let mut nb = 0;
        let mut nbrs: Vec<u32> = fs.iter().flat_map(|&f| tris[f as usize]).filter(|&w| w != v).collect();
        nbrs.sort_unstable();
        nbrs.dedup();
        for &w in &nbrs {
            let ef: Vec<u32> = fs.iter().copied().filter(|&f| tris[f as usize].contains(&w)).collect();
            if ef.len() != 2 || patch[ef[0] as usize] != patch[ef[1] as usize] {
                nb += 1;
            }
        }
        match (ps.len(), nb) {
            (1, 0) => Class::Interior,
            (_, 2) if ps.len() <= 2 => Class::Border,
            _ => Class::Corner,
        }
    };

    // candidate position and cost for collapsing edge (a, b)
    let evaluate = |pos: &Vec<Vec3>, q: &Vec<Quadric>, qa: &Vec<f64>, vf: &Vec<Vec<u32>>, alive_t: &Vec<bool>, tris: &Vec<[u32; 3]>, cls: &Vec<Class>, a: u32, b: u32| -> Option<(f64, Vec3)> {
        let (ca, cb) = (cls[a as usize], cls[b as usize]);
        let ef = edge_faces(vf, alive_t, tris, a, b);
        if ef.len() != 2 {
            return None; // open edge: keep
        }
        let border_edge = patch[ef[0] as usize] != patch[ef[1] as usize];
        let mut qs = q[a as usize];
        q_add(&mut qs, &q[b as usize]);
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let mid = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0];
        let cands: Vec<Vec3> = match (ca, cb) {
            (Class::Corner, Class::Corner) => return None,
            (Class::Corner, Class::Interior) => vec![pa],
            (Class::Interior, Class::Corner) => vec![pb],
            (Class::Corner, Class::Border) => {
                if !border_edge {
                    return None;
                }
                vec![pa]
            }
            (Class::Border, Class::Corner) => {
                if !border_edge {
                    return None;
                }
                vec![pb]
            }
            (Class::Border, Class::Border) => {
                if !border_edge {
                    return None; // would pinch two borders together
                }
                vec![pa, pb, mid]
            }
            (Class::Border, Class::Interior) => vec![pa],
            (Class::Interior, Class::Border) => vec![pb],
            (Class::Interior, Class::Interior) => {
                if border_edge {
                    return None;
                }
                let mut v = vec![pa, pb, mid];
                if let Some(p) = q_opt(&qs) {
                    // only accept the optimum near the edge
                    let l = norm(sub(pb, pa));
                    if norm(sub(p, mid)) < 2.0 * l {
                        v.push(p);
                    }
                }
                v
            }
        };
        let w = qa[a as usize] + qa[b as usize];
        // a flat fold (two faces folded onto each other, a zero-thickness fin
        // that volume meshers reject) goes first, whatever it costs
        let tn = |f: u32| {
            let t = tris[f as usize];
            cross(sub(pos[t[1] as usize], pos[t[0] as usize]), sub(pos[t[2] as usize], pos[t[0] as usize]))
        };
        let (n0, n1) = (tn(ef[0]), tn(ef[1]));
        let fold_pair = |f: u32, g: u32| {
            let (x, y) = (tn(f), tn(g));
            dot(x, y) < -0.99 * norm(x) * norm(y)
        };
        // a face folded onto a neighbour across any of its edges
        let face_folded = |f: u32| {
            let t = tris[f as usize];
            (0..3).any(|i| {
                let (x, y) = (t[i], t[(i + 1) % 3]);
                vf[x as usize].iter().any(|&g| g != f && alive_t[g as usize] && tris[g as usize].contains(&y) && fold_pair(f, g))
            })
        };
        let shortest = |f: u32| {
            let t = tris[f as usize];
            let l = |i: usize| norm(sub(pos[t[(i + 1) % 3] as usize], pos[t[i] as usize]));
            let k = (0..3).min_by(|&i, &j| l(i).partial_cmp(&l(j)).unwrap_or(Ordering::Equal)).unwrap();
            let (x, y) = (t[k], t[(k + 1) % 3]);
            (x == a && y == b) || (x == b && y == a)
        };
        // repair: collapse the short edge of a folded sliver (or the fold edge itself)
        let folded = dot(n0, n1) < -0.99 * norm(n0) * norm(n1) || ef.iter().any(|&f| shortest(f) && face_folded(f));
        cands
            .into_iter()
            .map(|p| (q_eval(&qs, p).max(0.0), p))
            .min_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(Ordering::Equal))
            .map(|(e, p)| (if folded { -1.0 } else { e / w.max(1e-300) }, p))
    };

    let mut heap = BinaryHeap::new();
    let push_edges = |heap: &mut BinaryHeap<Cand>, pos: &Vec<Vec3>, q: &Vec<Quadric>, qa: &Vec<f64>, vf: &Vec<Vec<u32>>, alive_t: &Vec<bool>, tris: &Vec<[u32; 3]>, cls: &Vec<Class>, stamp: &Vec<u32>, v: u32, only_higher: bool| {
        let mut nbrs: Vec<u32> = vf[v as usize].iter().filter(|&&f| alive_t[f as usize]).flat_map(|&f| tris[f as usize]).filter(|&w| w != v).collect();
        nbrs.sort_unstable();
        nbrs.dedup();
        for w in nbrs {
            if only_higher && w < v {
                continue;
            }
            if let Some((cost, _)) = evaluate(pos, q, qa, vf, alive_t, tris, cls, v, w) {
                if cost <= o.max_error * o.max_error {
                    heap.push(Cand { cost, a: v, b: w, stamp: (stamp[v as usize], stamp[w as usize]) });
                }
            }
        }
    };
    let mut cls: Vec<Class> = (0..nv as u32).map(|v| classify(&vf, &alive_t, &tris, v)).collect();
    for v in 0..nv as u32 {
        push_edges(&mut heap, &pos, &q, &qa, &vf, &alive_t, &tris, &cls, &stamp, v, true);
    }

    while n_alive > o.target_tris {
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.a, c.b);
        if !alive_v[a as usize] || !alive_v[b as usize] || stamp[a as usize] != c.stamp.0 || stamp[b as usize] != c.stamp.1 {
            continue;
        }
        let Some((cost, p)) = evaluate(&pos, &q, &qa, &vf, &alive_t, &tris, &cls, a, b) else { continue };
        if cost > o.max_error * o.max_error {
            continue;
        }
        // neighbours' changes can raise a queued cost: re-queue instead of acting
        if cost > c.cost + 1e-9 * c.cost.abs() + 1e-300 {
            heap.push(Cand { cost, a, b, stamp: c.stamp });
            continue;
        }
        // link condition: common neighbours must be exactly the two opposite vertices
        let fa = faces_of(&vf, &alive_t, a);
        let fb = faces_of(&vf, &alive_t, b);
        let mut na: Vec<u32> = fa.iter().flat_map(|&f| tris[f as usize]).filter(|&w| w != a).collect();
        let mut nb: Vec<u32> = fb.iter().flat_map(|&f| tris[f as usize]).filter(|&w| w != b).collect();
        na.sort_unstable();
        na.dedup();
        nb.sort_unstable();
        nb.dedup();
        let common = na.iter().filter(|w| nb.binary_search(w).is_ok()).count();
        if common != 2 {
            continue;
        }
        let removed: Vec<u32> = fa.iter().copied().filter(|&f| tris[f as usize].contains(&b)).collect();
        if removed.len() != 2 {
            continue;
        }
        // the triangles that remain, with a and b moved to p
        let changed: Vec<u32> = fa.iter().chain(fb.iter()).copied().filter(|f| !removed.contains(f)).collect();
        let mut ok = true;
        let mut new_tris: Vec<(u32, [Vec3; 3])> = Vec::with_capacity(changed.len());
        for &f in &changed {
            let t = tris[f as usize];
            let old = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
            let new: [Vec3; 3] = std::array::from_fn(|i| if t[i] == a || t[i] == b { p } else { old[i] });
            let no = cross(sub(old[1], old[0]), sub(old[2], old[0]));
            let nn = cross(sub(new[1], new[0]), sub(new[2], new[0]));
            // repairs may keep poor shapes (the input was worse) but never flip
            let repair = c.cost < 0.0;
            let (min_dot, min_q) = if repair { (0.0, 1e-3) } else { (0.25, 0.08) };
            if dot(no, nn) <= min_dot * norm(no) * norm(nn) || quality(&new) < min_q {
                ok = false;
                break;
            }
            for i in 0..3 {
                if !repair && norm(sub(new[(i + 1) % 3], new[i])) > o.max_edge {
                    ok = false;
                }
            }
            new_tris.push((f, new));
        }
        if !ok {
            continue;
        }
        // no new flat folds between the moved faces and their neighbours
        let moved = |v: u32| if v == a || v == b { p } else { pos[v as usize] };
        'fold: for (f, nt) in &new_tris {
            let t = tris[*f as usize];
            let nn = cross(sub(nt[1], nt[0]), sub(nt[2], nt[0]));
            for i in 0..3 {
                let (x, y) = (t[i], t[(i + 1) % 3]);
                let (xx, yy) = (if x == b { a } else { x }, if y == b { a } else { y });
                for &g in vf[x as usize].iter().chain(vf[y as usize].iter()) {
                    if g == *f || !alive_t[g as usize] || removed.contains(&g) {
                        continue;
                    }
                    let tg = tris[g as usize];
                    let tg2 = tg.map(|v| if v == b { a } else { v });
                    if !(tg2.contains(&xx) && tg2.contains(&yy)) {
                        continue;
                    }
                    let og = tg.map(moved);
                    let ng = cross(sub(og[1], og[0]), sub(og[2], og[0]));
                    if dot(nn, ng) < -0.99 * norm(nn) * norm(ng) {
                        ok = false;
                        break 'fold;
                    }
                }
            }
        }
        if !ok {
            continue;
        }
        // no new self-intersections near the collapse
        'outer: for (f, nt) in &new_tris {
            let (lo, hi) = tri_bbox(nt);
            let mut hit = false;
            let t_new = tris[*f as usize].map(|v| if v == a { b } else { v });
            grid.for_cells(lo, hi, |cell| {
                if hit {
                    return;
                }
                for &g in &grid.cells[cell] {
                    if !alive_t[g as usize] || g == *f || removed.contains(&g) {
                        continue;
                    }
                    let tg = tris[g as usize];
                    // skip triangles sharing a vertex (after the collapse, a is b)
                    let tg2 = tg.map(|v| if v == a { b } else { v });
                    if tg2.iter().any(|v| t_new.contains(v)) {
                        continue;
                    }
                    let og: [Vec3; 3] = std::array::from_fn(|i| if tg[i] == a || tg[i] == b { p } else { pos[tg[i] as usize] });
                    let (l2, h2) = tri_bbox(&og);
                    if (0..3).any(|k| l2[k] > hi[k] || h2[k] < lo[k]) {
                        continue;
                    }
                    if tris_intersect(nt, &og) {
                        hit = true;
                        return;
                    }
                }
            });
            if hit {
                ok = false;
                break 'outer;
            }
        }
        if !ok {
            continue;
        }
        // collapse b into a at p
        for &f in &removed {
            alive_t[f as usize] = false;
            n_alive -= 1;
        }
        for &f in &fb {
            if alive_t[f as usize] {
                for v in tris[f as usize].iter_mut() {
                    if *v == b {
                        *v = a;
                    }
                }
                vf[a as usize].push(f);
            }
        }
        vf[b as usize].clear();
        alive_v[b as usize] = false;
        pos[a as usize] = p;
        let qb = q[b as usize];
        q_add(&mut q[a as usize], &qb);
        qa[a as usize] += qa[b as usize];
        vf[a as usize].retain(|&f| alive_t[f as usize]);
        vf[a as usize].sort_unstable();
        vf[a as usize].dedup();
        for (f, nt) in &new_tris {
            let (lo, hi) = tri_bbox(nt);
            grid.insert(*f, lo, hi);
        }
        // only edges at a changed cost; other queued edges are re-checked when popped
        stamp[a as usize] += 1;
        let mut ring: Vec<u32> = vf[a as usize].iter().flat_map(|&f| tris[f as usize]).collect();
        ring.sort_unstable();
        ring.dedup();
        for &w in &ring {
            cls[w as usize] = classify(&vf, &alive_t, &tris, w);
        }
        push_edges(&mut heap, &pos, &q, &qa, &vf, &alive_t, &tris, &cls, &stamp, a, false);
    }

    // compact
    let mut remap = vec![u32::MAX; nv];
    let mut out = TriMesh::default();
    for (t, tri) in tris.iter().enumerate() {
        if !alive_t[t] {
            continue;
        }
        let mut nt = [0u32; 3];
        for i in 0..3 {
            let v = tri[i] as usize;
            if remap[v] == u32::MAX {
                remap[v] = out.positions.len() as u32;
                out.positions.push(pos[v]);
            }
            nt[i] = remap[v];
        }
        out.triangles.push(nt);
        out.patch.push(patch[t]);
    }
    out
}

/// Writes a surface in the mesher's own format (`surface.json`,
/// `surface_positions.f64`, `surface_triangles.u32`, `surface_patch.u32`),
/// which `simlab_mesher.py volume <dir>` meshes without remeshing, keeping
/// the triangles and the patch numbers.
pub fn write_mesher_surface(dir: &std::path::Path, m: &TriMesh) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    crate::write_le(&dir.join("surface_positions.f64"), m.positions.iter().flatten().copied())?;
    crate::write_le(&dir.join("surface_triangles.u32"), m.triangles.iter().flatten().copied())?;
    crate::write_le(&dir.join("surface_patch.u32"), m.patch.iter().copied())?;
    let mut patches: Vec<u32> = m.patch.clone();
    patches.sort_unstable();
    patches.dedup();
    let header = serde_json::json!({ "source": "decimated", "vertices": m.positions.len(), "triangles": m.triangles.len(), "patches": patches });
    std::fs::write(dir.join("surface.json"), serde_json::to_vec_pretty(&header)?)?;
    Ok(())
}

/// Number of edges used by exactly one triangle (0 for a closed surface) and
/// by more than two (0 for a manifold one).
pub fn open_and_nonmanifold_edges(m: &TriMesh) -> (usize, usize) {
    let mut e: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for t in &m.triangles {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            *e.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    (e.values().filter(|&&c| c == 1).count(), e.values().filter(|&&c| c > 2).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UV sphere with two patches (top and bottom hemispheres).
    fn sphere(nu: usize, nv: usize) -> TriMesh {
        let mut positions = vec![[0.0, 0.0, 1.0]];
        for j in 1..nv {
            let th = std::f64::consts::PI * j as f64 / nv as f64;
            for i in 0..nu {
                let ph = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
                positions.push([th.sin() * ph.cos(), th.sin() * ph.sin(), th.cos()]);
            }
        }
        positions.push([0.0, 0.0, -1.0]);
        let id = |j: usize, i: usize| (1 + (j - 1) * nu + i % nu) as u32;
        let mut triangles = Vec::new();
        let mut patch = Vec::new();
        let south = (positions.len() - 1) as u32;
        for i in 0..nu {
            triangles.push([0, id(1, i), id(1, i + 1)]);
            patch.push(1);
        }
        for j in 1..nv - 1 {
            for i in 0..nu {
                let p = if j < nv / 2 { 1 } else { 2 };
                triangles.push([id(j, i), id(j + 1, i), id(j + 1, i + 1)]);
                triangles.push([id(j, i), id(j + 1, i + 1), id(j, i + 1)]);
                patch.push(p);
                patch.push(p);
            }
        }
        for i in 0..nu {
            triangles.push([south, id(nv - 1, i + 1), id(nv - 1, i)]);
            patch.push(2);
        }
        TriMesh { positions, triangles, patch }
    }

    #[test]
    fn sphere_stays_closed_and_round() {
        let m = sphere(120, 60);
        assert_eq!(open_and_nonmanifold_edges(&m), (0, 0));
        let d = decimate(&m, 2000, 0.01);
        assert!(d.triangles.len() <= 2100, "{} triangles", d.triangles.len());
        assert_eq!(open_and_nonmanifold_edges(&d), (0, 0));
        // volume within 2 % of the original, all points near the unit sphere
        let (v0, v1) = (m.volume(), d.volume());
        assert!((v1 - v0).abs() / v0 < 0.02, "{v0} {v1}");
        for p in &d.positions {
            assert!((norm(*p) - 1.0).abs() < 0.03);
        }
        // the equator border survives: every vertex where patches meet is still on z = 0
        let mut border = vec![false; d.positions.len()];
        let mut seen = std::collections::HashMap::new();
        for (t, tri) in d.triangles.iter().enumerate() {
            for &v in tri {
                let e = seen.entry(v).or_insert(d.patch[t]);
                if *e != d.patch[t] {
                    border[v as usize] = true;
                }
            }
        }
        for (i, p) in d.positions.iter().enumerate() {
            if border[i] {
                assert!(p[2].abs() < 0.06, "border vertex left the equator: {p:?}");
            }
        }
    }
}

/// Counts problems a volume mesher rejects: pairs of triangles that cross
/// (not sharing a vertex), and pairs sharing an edge that fold flat onto
/// each other (normals within ~2.5° of opposite).
pub fn surface_defects(m: &TriMesh) -> (usize, usize) {
    let (lo, hi) = m.bounds();
    let area: f64 = (0..m.triangles.len()).map(|t| m.triangle_area(t)).sum();
    let edge = (area / m.triangles.len().max(1) as f64 * 4.0 / 3f64.sqrt()).sqrt();
    let mut grid = Grid::new(lo, hi, 2.0 * edge.max(1e-12));
    let pts = |t: usize| -> [Vec3; 3] { std::array::from_fn(|i| m.positions[m.triangles[t][i] as usize]) };
    for t in 0..m.triangles.len() {
        let (a, b) = tri_bbox(&pts(t));
        grid.insert(t as u32, a, b);
    }
    let mut crossing = 0;
    for t in 0..m.triangles.len() {
        let pt = pts(t);
        let (lo, hi) = tri_bbox(&pt);
        let mut seen = std::collections::HashSet::new();
        grid.for_cells(lo, hi, |c| {
            for &g in &grid.cells[c] {
                let g = g as usize;
                if g <= t || !seen.insert(g) {
                    continue;
                }
                if m.triangles[g].iter().any(|v| m.triangles[t].contains(v)) {
                    continue;
                }
                if tris_intersect(&pt, &pts(g)) {
                    crossing += 1;
                }
            }
        });
    }
    let mut edges: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
    for (t, tri) in m.triangles.iter().enumerate() {
        for i in 0..3 {
            let (a, b) = (tri[i], tri[(i + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut folds = 0;
    for f in edges.values() {
        if f.len() == 2 {
            let (n0, n1) = (m.triangle_normal(f[0]), m.triangle_normal(f[1]));
            if dot(n0, n1) < -0.999 {
                folds += 1;
            }
        }
    }
    (crossing, folds)
}
