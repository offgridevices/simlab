//! Reading a propeller's blade geometry from its surface mesh.
//!
//! The surface is cut with cylinders r = const around the spin axis. Each
//! cut gives one closed loop per blade (the cylindrical blade section). The
//! loop is unwrapped to the plane (arc length r*theta, axial position z),
//! which is exact for a cylinder, and measured there:
//! * chord: the two points farthest apart (leading and trailing edge)
//! * pitch angle beta: the chord line's angle from the rotation plane
//! * thickness ratio and maximum camber: upper and lower surfaces resampled
//!   along the chord.
//!
//! The raised end of every section (toward +axis) must lead for the blade to
//! push air away from `axis`, which fixes the direction of rotation.

use crate::geom::{BladeGeometry, Rotation, Station};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use simlab_core::{TriMesh, Vec3, cross, dot, normalize, scale, sub};
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

/// Orthonormal frame around the spin axis: e1, e2 span the rotation plane,
/// e1 x e2 = axis (so +theta is a right-handed turn about the axis).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub center: Vec3,
    pub axis: Vec3,
    pub e1: Vec3,
    pub e2: Vec3,
}

impl Frame {
    pub fn new(center: Vec3, axis: Vec3) -> Frame {
        let a = normalize(axis);
        let pick = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let e1 = normalize(sub(pick, scale(a, dot(pick, a))));
        let e2 = cross(a, e1);
        Frame { center, axis: a, e1, e2 }
    }
    /// (x, y, z) in the frame
    pub fn local(&self, p: Vec3) -> Vec3 {
        let d = sub(p, self.center);
        [dot(d, self.e1), dot(d, self.e2), dot(d, self.axis)]
    }
    /// unit tangential direction (+theta) at point p
    pub fn e_theta(&self, p: Vec3) -> Vec3 {
        let [x, y, _] = self.local(p);
        let r = x.hypot(y).max(1e-300);
        let (c, s) = (x / r, y / r);
        normalize([
            -s * self.e1[0] + c * self.e2[0],
            -s * self.e1[1] + c * self.e2[1],
            -s * self.e1[2] + c * self.e2[2],
        ])
    }
    pub fn radius_of(&self, p: Vec3) -> f64 {
        let [x, y, _] = self.local(p);
        x.hypot(y)
    }
}

/// The mesh in cylindrical coordinates about a frame (any length unit).
pub struct CylMesh<'a> {
    pub xy: Vec<[f64; 2]>,
    pub z: Vec<f64>,
    pub r: Vec<f64>,
    tris: &'a [[u32; 3]],
    tri_rlo: Vec<f64>,
    tri_rhi: Vec<f64>,
}

/// One connected curve of a cylinder cut.
#[derive(Clone, Debug)]
pub struct CutLoop {
    /// (theta unwrapped, z)
    pub pts: Vec<[f64; 2]>,
    pub closed: bool,
    /// goes all the way around the axis (a hub ring)
    pub wraps: bool,
    pub theta_mean: f64,
    pub theta_extent: f64,
    /// length in the unwrapped plane
    pub perimeter: f64,
}

fn wrap_pi(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

impl<'a> CylMesh<'a> {
    pub fn new(mesh: &'a TriMesh, frame: &Frame) -> CylMesh<'a> {
        let n = mesh.positions.len();
        let mut xy = Vec::with_capacity(n);
        let mut z = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for p in &mesh.positions {
            let l = frame.local(*p);
            xy.push([l[0], l[1]]);
            z.push(l[2]);
            r.push(l[0].hypot(l[1]));
        }
        let mut tri_rlo = Vec::with_capacity(mesh.triangles.len());
        let mut tri_rhi = Vec::with_capacity(mesh.triangles.len());
        for t in &mesh.triangles {
            let rs = [r[t[0] as usize], r[t[1] as usize], r[t[2] as usize]];
            tri_rlo.push(rs[0].min(rs[1]).min(rs[2]));
            tri_rhi.push(rs[0].max(rs[1]).max(rs[2]));
        }
        CylMesh { xy, z, r, tris: &mesh.triangles, tri_rlo, tri_rhi }
    }

    pub fn max_radius(&self) -> f64 {
        self.r.iter().cloned().fold(0.0, f64::max)
    }
    pub fn min_radius(&self) -> f64 {
        self.r.iter().cloned().fold(f64::INFINITY, f64::min)
    }

    /// Exact crossing of edge (i, j) with the cylinder of radius r0.
    fn cross_edge(&self, i: usize, j: usize, r0: f64) -> [f64; 3] {
        let (p, q) = (self.xy[i], self.xy[j]);
        let d = [q[0] - p[0], q[1] - p[1]];
        let a = d[0] * d[0] + d[1] * d[1];
        let b = 2.0 * (p[0] * d[0] + p[1] * d[1]);
        let c = p[0] * p[0] + p[1] * p[1] - r0 * r0;
        let mut t = if a > 0.0 {
            let disc = (b * b - 4.0 * a * c).max(0.0).sqrt();
            let t1 = (-b + disc) / (2.0 * a);
            let t2 = (-b - disc) / (2.0 * a);
            if (-1e-9..=1.0 + 1e-9).contains(&t1) { t1 } else { t2 }
        } else {
            0.5
        };
        t = t.clamp(0.0, 1.0);
        let x = p[0] + t * d[0];
        let y = p[1] + t * d[1];
        let zz = self.z[i] + t * (self.z[j] - self.z[i]);
        [y.atan2(x), zz, 0.0]
    }

    /// Cuts the surface with the cylinder r = r0.
    pub fn cut(&self, r0: f64) -> Vec<CutLoop> {
        let inside = |v: u32| self.r[v as usize] < r0;
        let mut pid: HashMap<u64, u32> = HashMap::new();
        let mut pts: Vec<[f64; 2]> = Vec::new();
        let mut adj: Vec<[u32; 2]> = Vec::new();
        let mut deg: Vec<u8> = Vec::new();
        let none = u32::MAX;
        let mut point = |i: u32, j: u32, pts: &mut Vec<[f64; 2]>, adj: &mut Vec<[u32; 2]>, deg: &mut Vec<u8>| -> u32 {
            let (a, b) = if i < j { (i, j) } else { (j, i) };
            let key = ((a as u64) << 32) | b as u64;
            *pid.entry(key).or_insert_with(|| {
                let c = self.cross_edge(a as usize, b as usize, r0);
                pts.push([c[0], c[1]]);
                adj.push([none, none]);
                deg.push(0);
                (pts.len() - 1) as u32
            })
        };
        for (ti, t) in self.tris.iter().enumerate() {
            if self.tri_rlo[ti] >= r0 || self.tri_rhi[ti] < r0 {
                continue;
            }
            let ins = [inside(t[0]), inside(t[1]), inside(t[2])];
            let mut ends = [none; 2];
            let mut k = 0;
            for e in 0..3 {
                let (i, j) = (t[e], t[(e + 1) % 3]);
                if ins[e] != ins[(e + 1) % 3] && k < 2 {
                    ends[k] = point(i, j, &mut pts, &mut adj, &mut deg);
                    k += 1;
                }
            }
            if k == 2 && ends[0] != ends[1] {
                for (u, v) in [(ends[0], ends[1]), (ends[1], ends[0])] {
                    let d = deg[u as usize] as usize;
                    if d < 2 {
                        adj[u as usize][d] = v;
                    }
                    deg[u as usize] = deg[u as usize].saturating_add(1);
                }
            }
        }
        // walk the curves: open chains from their ends first, then cycles
        let n = pts.len();
        let mut used = vec![false; n];
        let mut loops = vec![];
        let mut starts: Vec<usize> = (0..n).filter(|&i| deg[i] == 1).collect();
        starts.extend(0..n);
        for s in starts {
            if used[s] || deg[s] == 0 {
                continue;
            }
            let mut chain = vec![s];
            used[s] = true;
            let mut prev = none;
            let mut cur = s as u32;
            let mut closed = false;
            loop {
                let nb = adj[cur as usize];
                let next = if nb[0] != none && nb[0] != prev && !used[nb[0] as usize] {
                    nb[0]
                } else if nb[1] != none && nb[1] != prev && !used[nb[1] as usize] {
                    nb[1]
                } else {
                    if chain.len() > 2 && (nb[0] == s as u32 || nb[1] == s as u32) {
                        closed = true;
                    }
                    break;
                };
                used[next as usize] = true;
                chain.push(next as usize);
                prev = cur;
                cur = next;
            }
            if chain.len() < 3 {
                continue;
            }
            // unwrap theta along the chain
            let mut out = Vec::with_capacity(chain.len());
            let mut th = pts[chain[0]][0];
            out.push([th, pts[chain[0]][1]]);
            for w in chain.windows(2) {
                th += wrap_pi(pts[w[1]][0] - pts[w[0]][0]);
                out.push([th, pts[w[1]][1]]);
            }
            let total = th - out[0][0] + if closed { wrap_pi(out[0][0] - th) } else { 0.0 };
            let wraps = total.abs() > PI;
            let (mut lo, mut hi, mut sum) = (f64::INFINITY, f64::NEG_INFINITY, 0.0);
            for p in &out {
                lo = lo.min(p[0]);
                hi = hi.max(p[0]);
                sum += p[0];
            }
            let mut per = 0.0;
            for w in out.windows(2) {
                per += (r0 * (w[1][0] - w[0][0])).hypot(w[1][1] - w[0][1]);
            }
            let mean = sum / out.len() as f64;
            loops.push(CutLoop {
                pts: out,
                closed,
                wraps,
                theta_mean: wrap_pi(mean),
                theta_extent: if wraps { TAU } else { hi - lo },
                perimeter: per,
            });
        }
        loops
    }
}

/// Measured blade section (unwrapped coordinates, lengths in mesh units).
#[derive(Clone, Copy, Debug)]
pub struct SectionMeasure {
    pub chord: f64,
    /// signed blade angle for the given rotation, radians
    pub beta: f64,
    /// +1 if the raised end (toward +axis) is at larger theta
    pub handedness: f64,
    pub thickness: f64,
    pub camber: f64,
    /// chordwise position of maximum thickness (0 = leading edge)
    pub x_tmax: f64,
    pub theta_mean: f64,
}

fn convex_hull(p: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = p.to_vec();
    v.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap().then(a[1].partial_cmp(&b[1]).unwrap()));
    v.dedup();
    if v.len() < 3 {
        return v;
    }
    let crs = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut h: Vec<[f64; 2]> = Vec::with_capacity(2 * v.len());
    for &q in &v {
        while h.len() >= 2 && crs(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    let lower = h.len() + 1;
    for &q in v.iter().rev().skip(1) {
        while h.len() >= lower && crs(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    h.pop();
    h
}

/// Measures a closed section loop at radius r0. `rot` = +1 if the blade
/// moves toward +theta, -1 toward -theta, 0 = decide from this section.
pub fn measure_section(l: &CutLoop, r0: f64, rot: f64) -> Option<SectionMeasure> {
    if !l.closed || l.wraps || l.pts.len() < 6 {
        return None;
    }
    let t0 = l.pts[0][0];
    let p: Vec<[f64; 2]> = l.pts.iter().map(|q| [r0 * (q[0] - t0), q[1]]).collect();
    let hull = convex_hull(&p);
    let (mut best, mut ia, mut ib) = (-1.0, 0, 0);
    for i in 0..hull.len() {
        for j in i + 1..hull.len() {
            let d = (hull[i][0] - hull[j][0]).powi(2) + (hull[i][1] - hull[j][1]).powi(2);
            if d > best {
                best = d;
                ia = i;
                ib = j;
            }
        }
    }
    let chord = best.sqrt();
    if chord.is_nan() || chord <= 0.0 {
        return None;
    }
    let (pa, pb) = (hull[ia], hull[ib]);
    let hand = ((pb[0] - pa[0]) * (pb[1] - pa[1])).signum();
    let rot = if rot == 0.0 { if hand == 0.0 { 1.0 } else { hand } } else { rot };
    // leading edge: the end further along the direction of motion
    let (le, te) = if rot * pa[0] >= rot * pb[0] { (pa, pb) } else { (pb, pa) };
    let d = [le[0] - te[0], le[1] - te[1]];
    let beta = d[1].atan2(rot * d[0]);
    // chord frame: x from LE to TE (0..1), y toward +axis side
    let xh = [-d[0] / chord, -d[1] / chord];
    let mut yh = [-xh[1], xh[0]];
    if yh[1] < 0.0 {
        yh = [-yh[0], -yh[1]];
    }
    let q: Vec<[f64; 2]> = p
        .iter()
        .map(|v| {
            let w = [v[0] - le[0], v[1] - le[1]];
            [(w[0] * xh[0] + w[1] * xh[1]) / chord, (w[0] * yh[0] + w[1] * yh[1]) / chord]
        })
        .collect();
    // split the loop at the points nearest LE (x=0) and TE (x=1)
    let n = q.len();
    let ile = (0..n).min_by(|&a, &b| (q[a][0].hypot(q[a][1])).partial_cmp(&q[b][0].hypot(q[b][1])).unwrap()).unwrap();
    let ite = (0..n)
        .min_by(|&a, &b| ((q[a][0] - 1.0).hypot(q[a][1])).partial_cmp(&(q[b][0] - 1.0).hypot(q[b][1])).unwrap())
        .unwrap();
    let mut c1 = vec![];
    let mut k = ile;
    loop {
        c1.push(q[k]);
        if k == ite {
            break;
        }
        k = (k + 1) % n;
    }
    let mut c2 = vec![];
    let mut k = ile;
    loop {
        c2.push(q[k]);
        if k == ite {
            break;
        }
        k = (k + n - 1) % n;
    }
    if c1.len() < 2 || c2.len() < 2 {
        return None;
    }
    // y of a chain at x: extreme over all crossings (max for upper, min for lower)
    let y_at = |c: &[[f64; 2]], x: f64, upper: bool| -> Option<f64> {
        let mut out: Option<f64> = None;
        for w in c.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (a[0] - x) * (b[0] - x) <= 0.0 && a[0] != b[0] {
                let y = a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0]);
                out = Some(match out {
                    None => y,
                    Some(o) => {
                        if upper {
                            o.max(y)
                        } else {
                            o.min(y)
                        }
                    }
                });
            }
        }
        out
    };
    let mean_y = |c: &[[f64; 2]]| c.iter().map(|v| v[1]).sum::<f64>() / c.len() as f64;
    let (up, lo) = if mean_y(&c1) >= mean_y(&c2) { (&c1, &c2) } else { (&c2, &c1) };
    let (mut tmax, mut xt, mut camb, mut xc) = (0.0, 0.5, 0.0f64, 0.4);
    let m = 60;
    for i in 1..m {
        let x = 0.5 * (1.0 - (PI * i as f64 / m as f64).cos());
        if let (Some(yu), Some(yl)) = (y_at(up, x, true), y_at(lo, x, false)) {
            let t = yu - yl;
            if t > tmax {
                tmax = t;
                xt = x;
            }
            let mc = 0.5 * (yu + yl);
            if (0.05..=0.95).contains(&x) && mc.abs() > camb.abs() {
                camb = mc;
                xc = x;
            }
        }
    }
    // a blunt trailing edge: the farthest-point chord ends on one TE corner;
    // aim the chord line at the middle of the TE instead
    let mut beta = beta;
    let mid = |x: f64| -> Option<f64> { Some(0.5 * (y_at(up, x, true)? + y_at(lo, x, false)?)) };
    if let (Some(y97), Some(y99)) = (mid(0.97), mid(0.99)) {
        // mean line extrapolated to x = 1
        let ym = y99 + 0.5 * (y99 - y97);
        if ym.abs() < 0.05 {
            beta -= ym.atan();
            camb -= ym * xc;
        }
    }
    Some(SectionMeasure { chord, beta, handedness: hand, thickness: tmax, camber: camb, x_tmax: xt, theta_mean: l.theta_mean })
}

#[derive(Clone, Copy, Debug)]
pub struct ExtractOptions {
    /// radial stations to measure
    pub stations: usize,
    /// None = count from the mesh
    pub blades: Option<u32>,
}
impl Default for ExtractOptions {
    fn default() -> Self {
        ExtractOptions { stations: 25, blades: None }
    }
}

/// Blade-to-blade variation at one station.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct StationSpread {
    pub r: f64,
    pub blades_found: u32,
    /// (max - min) / mean
    pub chord_spread: f64,
    /// max - min, degrees
    pub beta_spread_deg: f64,
    pub x_tmax: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtractedProp {
    /// in metres
    pub geometry: BladeGeometry,
    /// in metres
    pub frame: Frame,
    /// radius of the centre hole, m (None if solid or not found)
    pub bore_radius: Option<f64>,
    /// where the blades join the hub, m
    pub hub_join_radius: f64,
    pub spread: Vec<StationSpread>,
    pub warnings: Vec<String>,
}

struct Classified {
    blades: Vec<usize>,
    rings: usize,
    other: usize,
}

fn classify(loops: &[CutLoop]) -> Classified {
    let pmax = loops.iter().filter(|l| !l.wraps).map(|l| l.perimeter).fold(0.0, f64::max);
    let mut c = Classified { blades: vec![], rings: 0, other: 0 };
    for (i, l) in loops.iter().enumerate() {
        if l.wraps {
            c.rings += 1;
        } else if l.perimeter >= 0.08 * pmax && l.theta_extent < PI {
            c.blades.push(i);
        } else {
            c.other += 1;
        }
    }
    c
}

/// Counts the blades: the most common number of separate blade sections
/// over 45-90 % of the radius.
pub fn count_blades(cyl: &CylMesh, r_tip: f64) -> u32 {
    let mut votes: HashMap<usize, usize> = HashMap::new();
    for i in 0..10 {
        let r0 = r_tip * (0.45 + 0.05 * i as f64);
        let c = classify(&cyl.cut(r0));
        if c.rings == 0 && !c.blades.is_empty() {
            *votes.entry(c.blades.len()).or_default() += 1;
        }
    }
    votes.into_iter().max_by_key(|&(n, v)| (v, n)).map(|(n, _)| n as u32).unwrap_or(0)
}

fn clean_blade_cut(cyl: &CylMesh, r0: f64, b: u32) -> bool {
    let loops = cyl.cut(r0);
    let c = classify(&loops);
    c.rings == 0 && c.blades.len() == b as usize && c.blades.iter().all(|&i| loops[i].closed && loops[i].theta_extent < 0.9 * TAU / b as f64)
}

/// Where the separate blades merge into the hub: the smallest radius whose
/// cut still shows exactly `b` separate closed blade sections.
pub fn hub_join_radius(cyl: &CylMesh, r_tip: f64, b: u32) -> f64 {
    let mut good = 0.6 * r_tip;
    if !clean_blade_cut(cyl, good, b) {
        // search outward for a clean radius
        let mut r = good;
        while r < 0.95 * r_tip && !clean_blade_cut(cyl, r, b) {
            r += 0.02 * r_tip;
        }
        good = r;
    }
    let rmin = cyl.min_radius().max(1e-6 * r_tip);
    let mut bad = None;
    let mut r = good;
    while r > rmin {
        r -= 0.02 * r_tip;
        if r <= rmin || !clean_blade_cut(cyl, r.max(rmin), b) {
            bad = Some(r.max(rmin));
            break;
        }
        good = r;
    }
    let Some(mut bad) = bad else { return good };
    for _ in 0..12 {
        let mid = 0.5 * (good + bad);
        if clean_blade_cut(cyl, mid, b) {
            good = mid;
        } else {
            bad = mid;
        }
    }
    good
}

/// Reads blade geometry from a propeller mesh.
/// `mesh` and `center` are in model units; `model_unit_m` is metres per
/// model unit; `axis` is the thrust direction (the drone's up).
pub fn extract_geometry(mesh: &TriMesh, model_unit_m: f64, axis: Vec3, center: Vec3, opts: &ExtractOptions) -> Result<ExtractedProp> {
    if mesh.triangles.is_empty() {
        bail!("the model has no triangles");
    }
    let u = model_unit_m;
    let frame_m = Frame::new(scale(center, u), axis);
    let frame = Frame::new(center, axis);
    let cyl = CylMesh::new(mesh, &frame);
    let r_tip = cyl.max_radius();
    if r_tip.is_nan() || r_tip <= 0.0 {
        bail!("the model has no size around the chosen axis");
    }
    let mut warnings = vec![];
    let b = match opts.blades {
        Some(b) if b > 0 => b,
        _ => count_blades(&cyl, r_tip),
    };
    if b == 0 {
        bail!("could not find separate blades around this axis; check the axis and centre");
    }
    let r_join = hub_join_radius(&cyl, r_tip, b);
    let rmin = cyl.min_radius();
    let bore = if rmin > 0.01 * r_tip && rmin < r_join { Some(rmin * u) } else { None };
    let r_start = r_join + 0.02 * (r_tip - r_join);
    let r_end = r_tip * 0.99;
    let n = opts.stations.max(5);
    let radii: Vec<f64> = (0..n).map(|k| r_start + (r_end - r_start) * (0.5 * PI * k as f64 / (n - 1) as f64).sin()).collect();
    // cut every station once; decide the rotation from all of them
    let cuts: Vec<(f64, Vec<CutLoop>)> = radii.iter().map(|&r0| (r0, cyl.cut(r0))).collect();
    let mut hand = 0.0;
    for (r0, loops) in &cuts {
        let c = classify(loops);
        for &i in &c.blades {
            if let Some(m) = measure_section(&loops[i], *r0, 0.0) {
                hand += m.handedness * m.chord * r0 * m.beta.sin().abs();
            }
        }
    }
    let rot = if hand >= 0.0 { 1.0 } else { -1.0 };
    let mut stations = vec![];
    let mut spread = vec![];
    let (mut wcam, mut wx, mut wsum) = (0.0, 0.0, 0.0);
    for (r0, loops) in &cuts {
        let c = classify(loops);
        let ms: Vec<SectionMeasure> = c
            .blades
            .iter()
            .filter_map(|&i| measure_section(&loops[i], *r0, rot))
            .filter(|m| m.chord > 1e-3 * r_tip)
            .collect();
        if ms.is_empty() {
            continue;
        }
        let k = ms.len() as f64;
        let mean = |f: &dyn Fn(&SectionMeasure) -> f64| ms.iter().map(f).sum::<f64>() / k;
        let chord = mean(&|m| m.chord);
        let st = Station {
            r: r0 * u,
            chord: chord * u,
            beta: mean(&|m| m.beta),
            thickness: mean(&|m| m.thickness),
            camber: mean(&|m| m.camber),
        };
        let cmax = ms.iter().map(|m| m.chord).fold(0.0, f64::max);
        let cmin = ms.iter().map(|m| m.chord).fold(f64::INFINITY, f64::min);
        let bmax = ms.iter().map(|m| m.beta).fold(f64::NEG_INFINITY, f64::max);
        let bmin = ms.iter().map(|m| m.beta).fold(f64::INFINITY, f64::min);
        let xt = mean(&|m| m.x_tmax);
        spread.push(StationSpread {
            r: r0 * u,
            blades_found: ms.len() as u32,
            chord_spread: (cmax - cmin) / chord,
            beta_spread_deg: (bmax - bmin).to_degrees(),
            x_tmax: xt,
        });
        let w = chord * r0 * r0;
        wcam += w * st.camber;
        wx += w * xt;
        wsum += w;
        stations.push(st);
    }
    if stations.len() < 4 {
        bail!("could not measure enough blade sections; is this a propeller, and is the axis right?");
    }
    // the tip itself
    let last = *stations.last().unwrap();
    stations.push(Station { r: r_tip * u, chord: 0.0, ..last });
    if wsum > 0.0 {
        let (cam, xt) = (wcam / wsum, wx / wsum);
        if cam < -0.005 && xt > 0.5 {
            warnings.push(
                "The blades look upside down for this thrust direction: their curved, rounded side faces the wrong way. Check the axis (thrust should point toward the drone's top)."
                    .into(),
            );
        }
    }
    let mid: Vec<&StationSpread> = spread.iter().filter(|s| s.r > 0.3 * r_tip * u && s.r < 0.95 * r_tip * u).collect();
    if mid.iter().any(|s| s.blades_found < b) {
        warnings.push("Some blade sections could not be measured on every blade; the mesh may have holes.".into());
    }
    let worst_c = mid.iter().map(|s| s.chord_spread).fold(0.0, f64::max);
    let worst_b = mid.iter().map(|s| s.beta_spread_deg).fold(0.0, f64::max);
    if worst_c > 0.05 || worst_b > 1.5 {
        warnings.push(format!(
            "The blades are not identical (up to {:.0}% chord and {:.1} degrees pitch difference); results use their average.",
            worst_c * 100.0,
            worst_b
        ));
    }
    let geometry = BladeGeometry {
        blades: b,
        radius: r_tip * u,
        hub_radius: stations[0].r,
        rotation: Rotation::from_sign(rot),
        stations,
    };
    Ok(ExtractedProp { geometry, frame: frame_m, bore_radius: bore, hub_join_radius: r_join * u, spread, warnings })
}

/// A suggestion for the UI when a model looks like a propeller.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PropGuess {
    /// thrust direction
    pub axis: Vec3,
    /// in model units
    pub center: Vec3,
    /// in model units
    pub radius: f64,
    pub hub_radius: f64,
    pub blades: u32,
    pub rotation: Rotation,
    /// 0..1, how propeller-like the model is
    pub confidence: f64,
}

#[allow(clippy::needless_range_loop)]
fn jacobi_eigen(mut a: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let mut off = 0.0;
        for i in 0..3 {
            for j in i + 1..3 {
                off += a[i][j] * a[i][j];
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..3 {
            for q in p + 1..3 {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..3 {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..3 {
                    let apk = a[p][k];
                    let aqk = a[q][k];
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for k in 0..3 {
                    let vkp = v[k][p];
                    let vkq = v[k][q];
                    v[k][p] = c * vkp - s * vkq;
                    v[k][q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// Fits a circle (Kasa) to 2-D points: (cx, cy, r, rms residual).
fn fit_circle(p: &[[f64; 2]]) -> Option<(f64, f64, f64, f64)> {
    if p.len() < 6 {
        return None;
    }
    let n = p.len() as f64;
    let (mx, my) = (p.iter().map(|q| q[0]).sum::<f64>() / n, p.iter().map(|q| q[1]).sum::<f64>() / n);
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for q in p {
        let (u, v) = (q[0] - mx, q[1] - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-300 {
        return None;
    }
    let b1 = 0.5 * (suuu + suvv);
    let b2 = 0.5 * (svvv + svuu);
    let uc = (b1 * svv - b2 * suv) / det;
    let vc = (suu * b2 - suv * b1) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    let (cx, cy) = (uc + mx, vc + my);
    let res = (p.iter().map(|q| ((q[0] - cx).hypot(q[1] - cy) - r).powi(2)).sum::<f64>() / n).sqrt();
    Some((cx, cy, r, res))
}

/// Guesses spin axis, centre and blade count of a model that may be a
/// propeller (any length unit). None when it does not look like one.
pub fn detect_prop(mesh: &TriMesh) -> Option<PropGuess> {
    if mesh.triangles.len() < 20 {
        return None;
    }
    // area-weighted surface centroid and second moments
    let mut area = 0.0;
    let mut c = [0.0; 3];
    for t in 0..mesh.triangles.len() {
        let [a, b, cc] = mesh.triangles[t];
        let (a, b, cc) = (mesh.positions[a as usize], mesh.positions[b as usize], mesh.positions[cc as usize]);
        let w = 0.5 * simlab_core::norm(cross(sub(b, a), sub(cc, a)));
        for k in 0..3 {
            c[k] += w * (a[k] + b[k] + cc[k]) / 3.0;
        }
        area += w;
    }
    if area <= 0.0 {
        return None;
    }
    let c = scale(c, 1.0 / area);
    let mut s = [[0.0; 3]; 3];
    for t in 0..mesh.triangles.len() {
        let [a, b, cc] = mesh.triangles[t];
        let (a, b, cc) = (mesh.positions[a as usize], mesh.positions[b as usize], mesh.positions[cc as usize]);
        let w = 0.5 * simlab_core::norm(cross(sub(b, a), sub(cc, a)));
        for p in [a, b, cc] {
            let d = sub(p, c);
            for i in 0..3 {
                for j in 0..3 {
                    s[i][j] += w * d[i] * d[j] / 3.0;
                }
            }
        }
    }
    let (ev, vecs) = jacobi_eigen(s);
    let mut idx = [0, 1, 2];
    idx.sort_by(|&i, &j| ev[i].partial_cmp(&ev[j]).unwrap());
    let thin = [vecs[0][idx[0]], vecs[1][idx[0]], vecs[2][idx[0]]];
    let flat = (ev[idx[0]].max(0.0) / ev[idx[1]].max(1e-300)).sqrt();
    if flat > 0.5 {
        return None;
    }
    // refine the centre on the bore (or hub) if one is visible
    let mut center = c;
    for _ in 0..2 {
        let f = Frame::new(center, thin);
        let cyl = CylMesh::new(mesh, &f);
        let r_tip = cyl.max_radius();
        let rmin = cyl.min_radius();
        let band = rmin + 0.03 * r_tip;
        let pts: Vec<[f64; 2]> = cyl.xy.iter().zip(&cyl.r).filter(|(_, r)| **r <= band).map(|(p, _)| *p).collect();
        if let Some((cx, cy, r, res)) = fit_circle(&pts) {
            let ang: Vec<f64> = pts.iter().map(|p| (p[1] - cy).atan2(p[0] - cx)).collect();
            let mut sorted = ang.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut gap: f64 = TAU - (sorted[sorted.len() - 1] - sorted[0]);
            for w in sorted.windows(2) {
                gap = gap.max(w[1] - w[0]);
            }
            if r < 0.3 * r_tip && res < 0.05 * r.max(1e-12) && gap < 0.5 * PI {
                center = [
                    center[0] + cx * f.e1[0] + cy * f.e2[0],
                    center[1] + cx * f.e1[1] + cy * f.e2[1],
                    center[2] + cx * f.e1[2] + cy * f.e2[2],
                ];
            }
        }
    }
    // choose the thrust side from the sections' shape: camber bulges toward
    // the thrust side and the thick, round edge leads
    let mut best: Option<(f64, Vec3, ExtractedProp)> = None;
    for sign in [1.0, -1.0] {
        let axis = scale(thin, sign);
        let Ok(ex) = extract_geometry(mesh, 1.0, axis, center, &ExtractOptions { stations: 12, blades: None }) else { continue };
        let g = &ex.geometry;
        let (mut wc, mut wx, mut w) = (0.0, 0.0, 0.0);
        for (st, sp) in g.stations.iter().zip(&ex.spread) {
            if st.r > 0.3 * g.radius && st.r < 0.9 * g.radius {
                let k = st.chord * st.r * st.r;
                wc += k * st.camber;
                wx += k * sp.x_tmax;
                w += k;
            }
        }
        if w <= 0.0 {
            continue;
        }
        let score = 20.0 * wc / w + (0.5 - wx / w) + 1e-3 * sign * axis[2].signum();
        if best.as_ref().map(|b| score > b.0).unwrap_or(true) {
            best = Some((score, axis, ex));
        }
    }
    let (_, axis, ex) = best?;
    let g = &ex.geometry;
    if g.blades < 2 || g.blades > 12 {
        return None;
    }
    // confidence: flatness, matching blades, sensible pitch
    let mut conf: f64 = 1.0;
    if flat > 0.35 {
        conf -= 0.3;
    }
    let b75 = g.at(0.75 * g.radius).beta.to_degrees();
    if !(3.0..=60.0).contains(&b75) {
        conf -= 0.4;
    }
    let mid: Vec<&StationSpread> = ex.spread.iter().filter(|s| s.r > 0.3 * g.radius && s.r < 0.9 * g.radius).collect();
    if mid.iter().any(|s| s.chord_spread > 0.1 || s.beta_spread_deg > 3.0) {
        conf -= 0.3;
    }
    if mid.iter().any(|s| s.blades_found < g.blades) {
        conf -= 0.2;
    }
    Some(PropGuess {
        axis,
        center,
        radius: g.radius,
        hub_radius: ex.hub_join_radius,
        blades: g.blades,
        rotation: g.rotation,
        confidence: conf.clamp(0.0, 1.0),
    })
}
