//! Turning a triangle surface into lattice geometry, on the CPU with rayon.
//!
//! * Inside/outside: ray parity along lines parallel to x, y and z through
//!   every grid point, with a majority vote. Lines that cross the surface an
//!   odd number of times (a hole in the mesh) do not vote, so small holes
//!   and flipped triangles do not flood the inside or the outside. Where a
//!   line is consistently oriented the non-zero winding rule is used, so
//!   overlapping shells (a blade pushed into a hub) count as one solid.
//!   Edge tests use a canonical vertex order, so a line through an edge
//!   shared by two triangles counts exactly one crossing.
//! * Wall links: every link from a fluid cell into a solid cell (so the set
//!   of links is consistent and a uniform state feels no force). Each is
//!   cast as a segment against nearby triangles (Möller–Trumbore); the
//!   nearest hit gives the Bouzidi wall distance q (no hit: q = 1/2). A link
//!   between two fluid cells that crosses the surface twice (a part thinner
//!   than a cell) is a wall from both sides, so thin parts still block flow.

use crate::shader::C;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

pub type P3 = [f64; 3];

/// A surface in some grid's coordinates (grid units).
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub verts: Vec<P3>,
    pub tris: Vec<[u32; 3]>,
}

impl Surface {
    pub fn bounds(&self) -> (P3, P3) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &self.verts {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }
    fn tri(&self, t: usize) -> [P3; 3] {
        let [a, b, c] = self.tris[t];
        [self.verts[a as usize], self.verts[b as usize], self.verts[c as usize]]
    }
}

/// Grid points `origin + h * (i, j, k)` for i < dims.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub origin: P3,
    pub h: f64,
    pub dims: [usize; 3],
}
impl Grid {
    pub fn len(&self) -> usize {
        self.dims[0] * self.dims[1] * self.dims[2]
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.dims[1] + j) * self.dims[0] + i
    }
}

// Small irrational offsets of the scan lines, in grid units, so lines do not
// pass exactly through the vertices and edges of axis-aligned CAD geometry.
const EPS_B: f64 = 1.234_567_1e-5;
const EPS_C: f64 = 2.718_281_8e-5;

/// Inside test for every grid point (true = inside the surface).
pub fn inside_grid(s: &Surface, g: &Grid) -> Vec<bool> {
    let n = g.len();
    if n == 0 || s.tris.is_empty() {
        return vec![false; n];
    }
    // votes[cell] : bits 0-2 inside per axis, bits 3-5 valid per axis
    let votes: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(0)).collect();
    for axis in 0..3 {
        parity_axis(s, g, axis, &votes);
    }
    votes
        .par_iter()
        .map(|v| {
            let v = v.load(Ordering::Relaxed);
            let valid = (v >> 3) & 7;
            let ins = v & 7 & valid;
            let nv = valid.count_ones();
            let ni = ins.count_ones();
            nv > 0 && 2 * ni > nv
        })
        .collect()
}

fn parity_axis(s: &Surface, g: &Grid, a: usize, votes: &[AtomicU32]) {
    let (b, c) = ((a + 1) % 3, (a + 2) % 3);
    let (nb, nc) = (g.dims[b], g.dims[c]);
    let to_g = |p: P3| -> P3 { [(p[0] - g.origin[0]) / g.h, (p[1] - g.origin[1]) / g.h, (p[2] - g.origin[2]) / g.h] };
    let gv: Vec<P3> = s.verts.par_iter().map(|p| to_g(*p)).collect();
    // bucket triangles by the c-rows they cover
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); nc];
    for (t, tri) in s.tris.iter().enumerate() {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &v in tri {
            lo = lo.min(gv[v as usize][c]);
            hi = hi.max(gv[v as usize][c]);
        }
        let k0 = ((lo - EPS_C).ceil().max(0.0)) as usize;
        let k1f = (hi - EPS_C).floor();
        if k1f < 0.0 {
            continue;
        }
        let k1 = (k1f as usize).min(nc.saturating_sub(1));
        for row in rows.iter_mut().take(k1 + 1).skip(k0) {
            row.push(t as u32);
        }
    }
    rows.par_iter().enumerate().for_each(|(k, row)| {
        let pc = k as f64 + EPS_C;
        let mut hits: Vec<(u32, f64, i32)> = Vec::new();
        for &t in row {
            let tri = s.tris[t as usize];
            let ids = [tri[0], tri[1], tri[2]];
            let p = [gv[ids[0] as usize], gv[ids[1] as usize], gv[ids[2] as usize]];
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for q in &p {
                lo = lo.min(q[b]);
                hi = hi.max(q[b]);
            }
            let j0 = ((lo - EPS_B).ceil().max(0.0)) as usize;
            let j1f = (hi - EPS_B).floor();
            if j1f < 0.0 {
                continue;
            }
            let j1 = (j1f as usize).min(nb.saturating_sub(1));
            for j in j0..=j1 {
                let pb = j as f64 + EPS_B;
                // canonical edge functions in the (b, c) plane
                let e = |u: usize, v: usize| -> f64 {
                    let (iu, iv) = (ids[u], ids[v]);
                    let (pu, pv, sg) = if iu < iv { (p[u], p[v], 1.0) } else { (p[v], p[u], -1.0) };
                    sg * ((pv[b] - pu[b]) * (pc - pu[c]) - (pv[c] - pu[c]) * (pb - pu[b]))
                };
                let (e0, e1, e2) = (e(1, 2), e(2, 0), e(0, 1));
                let inside = (e0 > 0.0 && e1 > 0.0 && e2 > 0.0) || (e0 < 0.0 && e1 < 0.0 && e2 < 0.0);
                if !inside {
                    continue;
                }
                let sum = e0 + e1 + e2;
                let ta = (e0 * p[0][a] + e1 * p[1][a] + e2 * p[2][a]) / sum;
                hits.push((j as u32, ta, if sum > 0.0 { 1 } else { -1 }));
            }
        }
        hits.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let mut i0 = 0;
        while i0 < hits.len() {
            let j = hits[i0].0;
            let mut i1 = i0;
            while i1 < hits.len() && hits[i1].0 == j {
                i1 += 1;
            }
            let line = &hits[i0..i1];
            // closed, consistently oriented along this line: non-zero
            // winding (overlapping shells count as one solid); otherwise
            // fall back to parity if the count is even; else no vote
            let wsum: i32 = line.iter().map(|h| h.2).sum();
            let mode = if wsum == 0 { 1 } else if line.len() % 2 == 0 { 2 } else { 0 };
            mark_line(g, a, b, c, j as usize, k, line, mode, votes);
            i0 = i1;
        }
        // lines in this row with no hits at all are valid "outside" lines
        let mut have = vec![false; nb];
        for h in &hits {
            have[h.0 as usize] = true;
        }
        for (j, hv) in have.iter().enumerate() {
            if !hv {
                mark_line(g, a, b, c, j, k, &[], 1, votes);
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn mark_line(g: &Grid, a: usize, b: usize, c: usize, j: usize, k: usize, line: &[(u32, f64, i32)], mode: u8, votes: &[AtomicU32]) {
    let na = g.dims[a];
    let vbit = 1u32 << (3 + a);
    let ibit = 1u32 << a;
    let mut h = 0;
    let mut wind = 0i32;
    for i in 0..na {
        let ta = i as f64;
        while h < line.len() && line[h].1 < ta {
            wind += line[h].2;
            h += 1;
        }
        let mut pos = [0usize; 3];
        pos[a] = i;
        pos[b] = j;
        pos[c] = k;
        let id = g.idx(pos[0], pos[1], pos[2]);
        let mut bits = 0;
        if mode != 0 {
            bits |= vbit;
            let ins = if mode == 1 { wind != 0 } else { h % 2 == 1 };
            if ins {
                bits |= ibit;
            }
        }
        if bits != 0 {
            votes[id].fetch_or(bits, Ordering::Relaxed);
        }
    }
}

/// Triangles near each bin of a unit grid (`lo + (i, j, k)` .. `+1`): a
/// triangle is listed in every bin its bounding box, grown by `dilate`, touches.
pub struct Bins {
    pub lo: P3,
    pub dims: [usize; 3],
    pub start: Vec<u32>,
    pub list: Vec<u32>,
}

impl Bins {
    pub fn build(s: &Surface, lo: P3, dims: [usize; 3], dilate: f64) -> Bins {
        let nb = dims[0] * dims[1] * dims[2];
        let range = |t: usize| -> Option<([usize; 3], [usize; 3])> {
            let tri = s.tri(t);
            let mut r0 = [0usize; 3];
            let mut r1 = [0usize; 3];
            for k in 0..3 {
                let mn = tri.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min) - dilate - lo[k];
                let mx = tri.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max) + dilate - lo[k];
                if mx < 0.0 || mn >= dims[k] as f64 {
                    return None;
                }
                r0[k] = mn.floor().max(0.0) as usize;
                r1[k] = (mx.floor() as usize).min(dims[k] - 1);
            }
            Some((r0, r1))
        };
        let counts: Vec<AtomicU32> = (0..nb).map(|_| AtomicU32::new(0)).collect();
        let idx = |i: usize, j: usize, k: usize| (k * dims[1] + j) * dims[0] + i;
        (0..s.tris.len()).into_par_iter().for_each(|t| {
            if let Some((r0, r1)) = range(t) {
                for k in r0[2]..=r1[2] {
                    for j in r0[1]..=r1[1] {
                        for i in r0[0]..=r1[0] {
                            counts[idx(i, j, k)].fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        let mut start = vec![0u32; nb + 1];
        for i in 0..nb {
            start[i + 1] = start[i] + counts[i].load(Ordering::Relaxed);
        }
        let cursor: Vec<AtomicU32> = start[..nb].iter().map(|&v| AtomicU32::new(v)).collect();
        let list: Vec<AtomicU32> = (0..start[nb]).map(|_| AtomicU32::new(0)).collect();
        (0..s.tris.len()).into_par_iter().for_each(|t| {
            if let Some((r0, r1)) = range(t) {
                for k in r0[2]..=r1[2] {
                    for j in r0[1]..=r1[1] {
                        for i in r0[0]..=r1[0] {
                            let p = cursor[idx(i, j, k)].fetch_add(1, Ordering::Relaxed);
                            list[p as usize].store(t as u32, Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        let mut list: Vec<u32> = list.into_iter().map(|a| a.into_inner()).collect();
        // deterministic order inside each bin
        for i in 0..nb {
            list[start[i] as usize..start[i + 1] as usize].sort_unstable();
        }
        Bins { lo, dims, start, list }
    }

    pub fn get(&self, p: P3) -> &[u32] {
        let mut ix = [0usize; 3];
        for k in 0..3 {
            let f = (p[k] - self.lo[k]).floor();
            if f < 0.0 || f >= self.dims[k] as f64 {
                return &[];
            }
            ix[k] = f as usize;
        }
        let b = (ix[2] * self.dims[1] + ix[1]) * self.dims[0] + ix[0];
        &self.list[self.start[b] as usize..self.start[b + 1] as usize]
    }
}

/// Möller–Trumbore segment test: parameter t in [0, 1] along `o + t d`.
pub fn seg_tri(o: P3, d: P3, t: [P3; 3]) -> Option<f64> {
    let sub = |a: P3, b: P3| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: P3, b: P3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: P3, b: P3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let pv = cross(d, e2);
    let det = dot(e1, pv);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = sub(o, t[0]);
    let u = dot(tv, pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = cross(tv, e1);
    let v = dot(d, qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let tt = dot(e2, qv) * inv;
    if !(-1e-9..=1.0).contains(&tt) {
        return None;
    }
    Some(tt.max(0.0))
}

/// A wall-adjacent fluid cell and its cut links.
#[derive(Clone, Debug)]
pub struct WallCell {
    pub cell: u32,
    pub mask: u32,
    /// q for directions 1..=18 (index i-1); 1.0 where there is no wall
    pub q: [f32; 18],
}

pub struct StaticVoxels {
    /// solid cell indices
    pub solid: Vec<u32>,
    pub walls: Vec<WallCell>,
    /// the box of cells that was examined
    pub lo: [usize; 3],
    pub hi: [usize; 3],
}

/// Voxelizes a static surface given in lattice coordinates (cell centres
/// at integer points) into a grid of `dims` cells.
pub fn voxelize_static(s: &Surface, dims: [usize; 3]) -> StaticVoxels {
    let (blo, bhi) = s.bounds();
    let mut lo = [0usize; 3];
    let mut hi = [0usize; 3];
    for k in 0..3 {
        lo[k] = ((blo[k] - 2.0).floor().max(0.0)) as usize;
        hi[k] = (((bhi[k] + 2.0).ceil()) as usize).min(dims[k] - 1);
    }
    if s.tris.is_empty() || (0..3).any(|k| lo[k] > hi[k]) {
        return StaticVoxels { solid: vec![], walls: vec![], lo, hi };
    }
    let gd = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    let g = Grid { origin: [lo[0] as f64, lo[1] as f64, lo[2] as f64], h: 1.0, dims: gd };
    let inside = inside_grid(s, &g);
    let bins = Bins::build(s, [lo[0] as f64 - 0.5, lo[1] as f64 - 0.5, lo[2] as f64 - 0.5], gd, std::f64::consts::SQRT_2 + 0.01);
    let gidx = |i: usize, j: usize, k: usize| -> usize { ((k + lo[2]) * dims[1] + (j + lo[1])) * dims[0] + (i + lo[0]) };
    let solid: Vec<u32> = (0..g.len()).filter(|&l| inside[l]).map(|l| {
        let i = l % gd[0];
        let j = (l / gd[0]) % gd[1];
        let k = l / (gd[0] * gd[1]);
        gidx(i, j, k) as u32
    }).collect();
    let is_solid = |i: i64, j: i64, k: i64| -> bool {
        if i < 0 || j < 0 || k < 0 || i >= gd[0] as i64 || j >= gd[1] as i64 || k >= gd[2] as i64 {
            return false;
        }
        inside[g.idx(i as usize, j as usize, k as usize)]
    };
    // Wall links follow the solid/fluid classification (a link from a fluid
    // cell into a solid one), so every lattice line that enters the part also
    // leaves it and a uniform state exerts exactly zero force. The triangle
    // hit only sets q. A link between two fluid cells that crosses the
    // surface twice (a sheet thinner than a cell) is a wall from both sides.
    let rel = |l: usize| -> (usize, usize, usize) { (l % gd[0], (l / gd[0]) % gd[1], l / (gd[0] * gd[1])) };
    let per_cell: Vec<(usize, u32, [f32; 18], Vec<(usize, usize, f32)>)> = (0..g.len())
        .into_par_iter()
        .filter_map(|l| {
            if inside[l] {
                return None;
            }
            let (i, j, k) = rel(l);
            let p = [(i + lo[0]) as f64, (j + lo[1]) as f64, (k + lo[2]) as f64];
            let cand = bins.get(p);
            let mut q = [1.0f32; 18];
            let mut mask = 0u32;
            let mut thin = Vec::new();
            for d in 1..19 {
                let c = C[d];
                let (ni, nj, nk) = (i as i64 + c[0] as i64, j as i64 + c[1] as i64, k as i64 + c[2] as i64);
                let dv = [c[0] as f64, c[1] as f64, c[2] as f64];
                let (mut tmin, mut tmax, mut hits) = (f64::INFINITY, f64::NEG_INFINITY, 0);
                for &t in cand {
                    if let Some(tt) = seg_tri(p, dv, s.tri(t as usize)) {
                        tmin = tmin.min(tt);
                        tmax = tmax.max(tt);
                        hits += 1;
                    }
                }
                let in_region = ni >= 0 && nj >= 0 && nk >= 0 && (ni as usize) < gd[0] && (nj as usize) < gd[1] && (nk as usize) < gd[2];
                // beyond the examined box (a periodic wrap, or a part that
                // reaches the domain edge) the crossing count decides
                let nsolid = if in_region { is_solid(ni, nj, nk) } else { hits % 2 == 1 };
                if nsolid {
                    q[d - 1] = if hits > 0 { tmin.clamp(1e-3, 1.0) as f32 } else { 0.5 };
                    mask |= 1 << d;
                } else if hits >= 2 && tmax - tmin > 1e-6 {
                    q[d - 1] = tmin.clamp(1e-3, 1.0) as f32;
                    mask |= 1 << d;
                    let inside_region = ni >= 0 && nj >= 0 && nk >= 0 && (ni as usize) < gd[0] && (nj as usize) < gd[1] && (nk as usize) < gd[2];
                    if inside_region {
                        thin.push((g.idx(ni as usize, nj as usize, nk as usize), crate::shader::opp(d), (1.0 - tmax).clamp(1e-3, 1.0) as f32));
                    }
                }
            }
            if mask == 0 {
                return None;
            }
            Some((l, mask, q, thin))
        })
        .collect();
    // make thin-sheet links reciprocal
    let mut at: std::collections::HashMap<usize, usize> = per_cell.iter().enumerate().map(|(r, c)| (c.0, r)).collect();
    let mut cells: Vec<(usize, u32, [f32; 18])> = per_cell.iter().map(|c| (c.0, c.1, c.2)).collect();
    for c in &per_cell {
        for &(nl, d, qv) in &c.3 {
            if inside[nl] {
                continue;
            }
            let r = *at.entry(nl).or_insert_with(|| {
                cells.push((nl, 0, [1.0; 18]));
                cells.len() - 1
            });
            if cells[r].1 & (1 << d) == 0 {
                cells[r].1 |= 1 << d;
                cells[r].2[d - 1] = qv;
            }
        }
    }
    cells.sort_unstable_by_key(|c| c.0);
    let walls: Vec<WallCell> = cells
        .into_iter()
        .map(|(l, mask, q)| {
            let (i, j, k) = rel(l);
            WallCell { cell: gidx(i, j, k) as u32, mask, q }
        })
        .collect();
    StaticVoxels { solid, walls, lo, hi }
}

/// Body-frame geometry for a spinning part: occupancy bits on a fine grid
/// (spacing `h` lattice cells) and triangle bins for the GPU link tests.
pub struct SpinVoxels {
    pub occ_lo: P3,
    pub occ_h: f64,
    pub occ_dims: [usize; 3],
    pub occ_bits: Vec<u32>,
    pub bins: Bins,
    /// triangles as (v0, e1, e2), body coordinates
    pub tris: Vec<[f32; 12]>,
    /// largest distance of the surface from the spin axis, cells
    pub radius: f64,
    pub axial: (f64, f64),
}

/// `s` in body coordinates: lattice units, origin at the spin centre,
/// body x axis = spin axis at angle 0 (any orientation works).
pub fn voxelize_spin(s: &Surface, axis: P3, h: f64) -> SpinVoxels {
    let (blo, bhi) = s.bounds();
    let m = 2.0 * h;
    let occ_lo = [blo[0] - m, blo[1] - m, blo[2] - m];
    let occ_dims = [
        ((bhi[0] - blo[0] + 2.0 * m) / h).ceil() as usize + 1,
        ((bhi[1] - blo[1] + 2.0 * m) / h).ceil() as usize + 1,
        ((bhi[2] - blo[2] + 2.0 * m) / h).ceil() as usize + 1,
    ];
    let g = Grid { origin: occ_lo, h, dims: occ_dims };
    let inside = inside_grid(s, &g);
    let mut occ_bits = vec![0u32; g.len().div_ceil(32)];
    for (l, &v) in inside.iter().enumerate() {
        if v {
            occ_bits[l >> 5] |= 1 << (l & 31);
        }
    }
    let bin_lo = [blo[0] - 3.0, blo[1] - 3.0, blo[2] - 3.0];
    let bin_dims = [
        (bhi[0] - blo[0] + 6.0).ceil() as usize + 1,
        (bhi[1] - blo[1] + 6.0).ceil() as usize + 1,
        (bhi[2] - blo[2] + 6.0).ceil() as usize + 1,
    ];
    // (bins are kept for diagnostics; the GPU scatters triangles instead)
    let bins = Bins::build(s, bin_lo, [1, 1, 1], 0.0);
    let _ = bin_dims;
    // split triangles larger than ~1.5 cells so GPU threads get even work
    let fine = subdivide(s, 1.5);
    let tris = fine
        .tris
        .iter()
        .map(|t| {
            let a = fine.verts[t[0] as usize];
            let b = fine.verts[t[1] as usize];
            let c = fine.verts[t[2] as usize];
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            [a[0] as f32, a[1] as f32, a[2] as f32, 0.0, e1[0] as f32, e1[1] as f32, e1[2] as f32, 0.0, e2[0] as f32, e2[1] as f32, e2[2] as f32, 0.0]
        })
        .collect();
    let mut radius: f64 = 0.0;
    let mut amin = f64::INFINITY;
    let mut amax = f64::NEG_INFINITY;
    for p in &s.verts {
        let a = p[0] * axis[0] + p[1] * axis[1] + p[2] * axis[2];
        let r2 = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2] - a * a).max(0.0);
        radius = radius.max(r2.sqrt());
        amin = amin.min(a);
        amax = amax.max(a);
    }
    SpinVoxels { occ_lo, occ_h: h, occ_dims, occ_bits, bins, tris, radius, axial: (amin, amax) }
}

/// Splits triangles (1 into 4, at edge midpoints) until no edge is longer than `max_edge`.
pub fn subdivide(s: &Surface, max_edge: f64) -> Surface {
    let mut verts = s.verts.clone();
    let mut out = Vec::with_capacity(s.tris.len());
    let mut stack: Vec<[u32; 3]> = s.tris.clone();
    let len = |a: P3, b: P3| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    while let Some(t) = stack.pop() {
        let (a, b, c) = (verts[t[0] as usize], verts[t[1] as usize], verts[t[2] as usize]);
        if len(a, b).max(len(b, c)).max(len(c, a)) <= max_edge || out.len() > 50_000_000 {
            out.push(t);
            continue;
        }
        let mut mid = |p: P3, q: P3| {
            verts.push([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0]);
            (verts.len() - 1) as u32
        };
        let ab = mid(a, b);
        let bc = mid(b, c);
        let ca = mid(c, a);
        stack.extend_from_slice(&[[t[0], ab, ca], [ab, t[1], bc], [ca, bc, t[2]], [ab, bc, ca]]);
    }
    Surface { verts, tris: out }
}
