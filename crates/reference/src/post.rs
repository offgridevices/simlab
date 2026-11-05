//! Reading results back: forces from the solver log or postProcessing,
//! surface pressure per part vertex, and flow slices on a regular grid.

use crate::geom::vertex_normals;
use crate::plan::Plan;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use simlab_core::{TriMesh, Vec3, add, dot, norm, scale, sub};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A flow slice: a flat rectangle through the part, sampled on a regular
/// grid of `nu` × `nv` points. Point (i, j) sits at
/// `origin + u·(i + 0.5)/nu + v·(j + 0.5)/nv` (cell centres), and each
/// field's values are row-major in v then u: `values[j * nu + i]`, the
/// layout of the engine's `slice.<name>.bin`. NaN where the point is inside
/// the part (or outside the domain). Lengths in metres, model axes.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RefSlice {
    pub name: String,
    pub label: String,
    pub origin_m: Vec3,
    pub u_m: Vec3,
    pub v_m: Vec3,
    pub nu: usize,
    pub nv: usize,
    pub fields: Vec<SliceField>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SliceField {
    /// "speed" (|U|, m/s) or "pressure" (Pa above ambient)
    pub name: String,
    pub label: String,
    pub unit: String,
    pub min: f32,
    pub max: f32,
    pub values: Vec<f32>,
}

/// The two slice rectangles (empty fields) for a plan: for a still part in a
/// wind, the "side" plane (wind × lift) and the "top" plane (wind × side);
/// for a spinning part, the "axial" plane (containing the spin axis) and
/// the "disc" plane (the rotor plane).
pub fn slice_layout(p: &Plan, surface: &TriMesh) -> Vec<RefSlice> {
    let long = 240usize;
    let mk = |name: &str, label: &str, c: Vec3, u_dir: Vec3, u0: f64, u1: f64, v_dir: Vec3, v0: f64, v1: f64| {
        let (lu, lv) = (u1 - u0, v1 - v0);
        let h = lu.max(lv) / long as f64;
        let nu = (lu / h).round().max(2.0) as usize;
        let nv = (lv / h).round().max(2.0) as usize;
        RefSlice {
            name: name.into(),
            label: label.into(),
            origin_m: add(c, add(scale(u_dir, u0), scale(v_dir, v0))),
            u_m: scale(u_dir, lu),
            v_m: scale(v_dir, lv),
            nu,
            nv,
            fields: vec![],
        }
    };
    if let Some(r) = &p.rotor {
        let a = r.jet_dir;
        // a radial direction: the frame axis least aligned with the spin axis
        let f = &p.frame;
        let cand = [f.e2, f.e3, f.e1];
        let mut radial = cand[0];
        for c in cand {
            if dot(c, a).abs() < dot(radial, a).abs() {
                radial = c;
            }
        }
        let radial = simlab_core::normalize(sub(radial, scale(a, dot(radial, a))));
        let third = simlab_core::cross(a, radial);
        let rr = r.radius;
        vec![
            mk("axial", "Through the spin axis", r.center, radial, -2.0 * rr, 2.0 * rr, a, -1.5 * rr, 4.5 * rr),
            mk("disc", "Rotor plane", r.center, radial, -1.6 * rr, 1.6 * rr, third, -1.6 * rr, 1.6 * rr),
        ]
    } else {
        let f = &p.frame;
        let (elo, ehi) = crate::geom::extents_in(surface, f, p.center);
        let l = p.ref_length;
        vec![
            mk("side", "Side view (wind and lift)", p.center, f.e1, elo[0] - 1.5 * l, ehi[0] + 3.0 * l, f.e3, elo[2] - 1.25 * l, ehi[2] + 1.25 * l),
            mk("top", "Top view (wind and side)", p.center, f.e1, elo[0] - 1.5 * l, ehi[0] + 3.0 * l, f.e2, elo[1] - 1.25 * l, ehi[1] + 1.25 * l),
        ]
    }
}

pub fn slice_points(s: &RefSlice) -> Vec<Vec3> {
    let mut pts = Vec::with_capacity(s.nu * s.nv);
    for j in 0..s.nv {
        for i in 0..s.nu {
            let a = (i as f64 + 0.5) / s.nu as f64;
            let b = (j as f64 + 0.5) / s.nv as f64;
            pts.push(add(s.origin_m, add(scale(s.u_m, a), scale(s.v_m, b))));
        }
    }
    pts
}

/// Points just outside each vertex of the part (half a surface cell along
/// the vertex normal), where the near-wall pressure is sampled.
pub fn vertex_probe_points(surface: &TriMesh, offset: f64) -> Vec<Vec3> {
    let n = vertex_normals(surface);
    surface.positions.iter().zip(&n).map(|(&p, &n)| add(p, scale(n, offset))).collect()
}

// ------------------------------------------------------------- raw files

/// Rows of numbers from an OpenFOAM raw file (lines starting with '#' skipped).
pub fn read_raw(path: &Path) -> Result<Vec<Vec<f64>>> {
    let s = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(s.lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let r: Vec<f64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (r.len() >= 4).then_some(r)
        })
        .collect())
}

/// The newest time directory under postProcessing/<fo>/.
pub fn latest_time_dir(case: &Path, fo: &str) -> Option<PathBuf> {
    let dir = case.join("postProcessing").join(fo);
    let mut best: Option<(f64, PathBuf)> = None;
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        if let Ok(t) = e.file_name().to_string_lossy().parse::<f64>()
            && best.as_ref().is_none_or(|(b, _)| t > *b) {
                best = Some((t, e.path()));
            }
    }
    best.map(|(_, p)| p)
}

/// Files in `dir` (recursively one level) whose name starts with `prefix`.
fn files_starting(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_starting(&p, prefix));
        } else if e.file_name().to_string_lossy().starts_with(prefix) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Sampled values at a known list of points: (p, U) per point, None where
/// OpenFOAM found no cell. Output rows are matched back to the requested
/// points by position (OpenFOAM may reorder or drop them).
/// Pressure (kinematic) and velocity per requested point; None where not found.
pub type CloudSamples = (Vec<Option<f64>>, Vec<Option<Vec3>>);

pub fn read_cloud(dir: &Path, set: &str, points: &[Vec3]) -> Result<CloudSamples> {
    let files = files_starting(dir, &format!("{set}_"));
    if files.is_empty() {
        bail!("no samples for {set} in {}", dir.display());
    }
    let index = PointIndex::new(points);
    let mut p = vec![None; points.len()];
    let mut u = vec![None; points.len()];
    for f in files {
        let rows = read_raw(&f)?;
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        for r in rows {
            let x = [r[0], r[1], r[2]];
            let Some(i) = index.find(x) else { continue };
            match r.len() {
                4 => p[i] = Some(r[3]),
                6 => u[i] = Some([r[3], r[4], r[5]]),
                // p and U in one file: x y z p Ux Uy Uz (the order in `fields`)
                7 => {
                    p[i] = Some(r[3]);
                    u[i] = Some([r[4], r[5], r[6]]);
                }
                n => bail!("unexpected {n} columns in {name}"),
            }
        }
    }
    Ok((p, u))
}

/// Exact-ish point lookup (rows are written with 8+ significant digits).
struct PointIndex {
    h: f64,
    tol: f64,
    map: HashMap<[i64; 3], Vec<usize>>,
    pts: Vec<Vec3>,
}
impl PointIndex {
    fn new(pts: &[Vec3]) -> Self {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in pts {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let size = (0..3).map(|k| hi[k] - lo[k]).fold(1e-12, f64::max);
        let h = size / 2000.0;
        let mut map: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
        for (i, p) in pts.iter().enumerate() {
            map.entry(Self::key(*p, h)).or_default().push(i);
        }
        PointIndex { h, tol: size * 1e-6 + 1e-12, map, pts: pts.to_vec() }
    }
    fn key(p: Vec3, h: f64) -> [i64; 3] {
        [(p[0] / h).floor() as i64, (p[1] / h).floor() as i64, (p[2] / h).floor() as i64]
    }
    fn find(&self, x: Vec3) -> Option<usize> {
        let k = Self::key(x, self.h);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = self.map.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        for &i in v {
                            let d = norm(sub(self.pts[i], x));
                            if d <= self.tol && best.is_none_or(|(b, _)| d < b) {
                                best = Some((d, i));
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }
}

/// Pressure per vertex (Pa): the near-wall probe where OpenFOAM found one,
/// otherwise inverse-distance weighting of the 4 nearest sampled face values
/// on the part, otherwise the average of neighbouring vertices.
pub fn surface_pressure(
    surface: &TriMesh,
    probe: &[Option<f64>],
    faces: &[(Vec3, f64)],
    rho: f64,
) -> (Vec<f32>, usize) {
    let n = surface.positions.len();
    let mut out: Vec<Option<f64>> = probe.iter().map(|x| x.map(|p| p * rho)).collect();
    out.resize(n, None);
    let mut filled = 0;
    if !faces.is_empty() {
        let knn = Knn::new(faces.iter().map(|f| f.0).collect());
        for (o, &pos) in out.iter_mut().zip(&surface.positions) {
            if o.is_none() {
                let near = knn.nearest(pos, 4);
                let (mut sw, mut sv) = (0.0, 0.0);
                for (d, j) in near {
                    let w = 1.0 / (d * d + 1e-24);
                    sw += w;
                    sv += w * faces[j].1;
                }
                if sw > 0.0 {
                    *o = Some(sv / sw * rho);
                    filled += 1;
                }
            }
        }
    }
    // neighbours, for anything still missing
    let mut adj: Vec<Vec<u32>> = vec![vec![]; n];
    if out.iter().any(|x| x.is_none()) {
        for t in &surface.triangles {
            for k in 0..3 {
                adj[t[k] as usize].push(t[(k + 1) % 3]);
                adj[t[k] as usize].push(t[(k + 2) % 3]);
            }
        }
        for _ in 0..50 {
            let missing: Vec<usize> = (0..n).filter(|&i| out[i].is_none()).collect();
            if missing.is_empty() {
                break;
            }
            let mut changed = false;
            for i in missing {
                let vals: Vec<f64> = adj[i].iter().filter_map(|&j| out[j as usize]).collect();
                if !vals.is_empty() {
                    out[i] = Some(vals.iter().sum::<f64>() / vals.len() as f64);
                    filled += 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
    (out.into_iter().map(|x| x.map(|v| v as f32).unwrap_or(f32::NAN)).collect(), filled)
}

/// k-nearest search on a uniform grid.
pub struct Knn {
    pts: Vec<Vec3>,
    lo: Vec3,
    h: f64,
    dims: [i64; 3],
    cells: HashMap<[i64; 3], Vec<usize>>,
}
impl Knn {
    pub fn new(pts: Vec<Vec3>) -> Self {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in &pts {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let vol: f64 = (0..3).map(|k| (hi[k] - lo[k]).max(1e-9)).product();
        let h = (vol / pts.len().max(1) as f64 * 4.0).cbrt().max(1e-12);
        let dims = [0, 1, 2].map(|k| ((hi[k] - lo[k]) / h).floor() as i64 + 1);
        let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
        for (i, p) in pts.iter().enumerate() {
            cells.entry(Self::key_of(*p, lo, h)).or_default().push(i);
        }
        Knn { pts, lo, h, dims, cells }
    }
    fn key_of(p: Vec3, lo: Vec3, h: f64) -> [i64; 3] {
        [0, 1, 2].map(|k| ((p[k] - lo[k]) / h).floor() as i64)
    }
    /// (distance, index), nearest first
    pub fn nearest(&self, x: Vec3, k: usize) -> Vec<(f64, usize)> {
        let c = Self::key_of(x, self.lo, self.h);
        let mut found: Vec<(f64, usize)> = vec![];
        let maxr = self.dims.iter().copied().max().unwrap_or(1) + 1;
        for r in 0..=maxr {
            // shell r
            for dx in -r..=r {
                for dy in -r..=r {
                    for dz in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        if let Some(v) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                            for &i in v {
                                found.push((norm(sub(self.pts[i], x)), i));
                            }
                        }
                    }
                }
            }
            if found.len() >= k {
                found.sort_by(|a, b| a.0.total_cmp(&b.0));
                // everything within r·h is final
                if found[k - 1].0 <= r as f64 * self.h {
                    found.truncate(k);
                    return found;
                }
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.truncate(k);
        found
    }
}

/// Face values of the part's patch sample (raw: x y z value per face).
pub fn read_part_faces(case: &Path) -> Vec<(Vec3, f64)> {
    let Some(dir) = latest_time_dir(case, "partSurface") else { return vec![] };
    let mut out = vec![];
    for f in files_starting(&dir, "") {
        if !f.to_string_lossy().ends_with(".raw") {
            continue;
        }
        if let Ok(rows) = read_raw(&f) {
            out.extend(rows.into_iter().filter(|r| r.len() == 4).map(|r| ([r[0], r[1], r[2]], r[3])));
        }
    }
    out
}

/// Fills a slice's fields from sampled values (p kinematic → Pa).
pub fn fill_slice(s: &mut RefSlice, p: &[Option<f64>], u: &[Option<Vec3>], rho: f64) {
    let speed: Vec<f32> = u.iter().map(|x| x.map(|u| norm(u) as f32).unwrap_or(f32::NAN)).collect();
    let pres: Vec<f32> = p.iter().map(|x| x.map(|p| (p * rho) as f32).unwrap_or(f32::NAN)).collect();
    let mm = |v: &[f32]| {
        let f: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
        (f.iter().copied().fold(f32::INFINITY, f32::min), f.iter().copied().fold(f32::NEG_INFINITY, f32::max))
    };
    let (smin, smax) = mm(&speed);
    let (pmin, pmax) = mm(&pres);
    s.fields = vec![
        SliceField { name: "speed".into(), label: "Air speed".into(), unit: "m/s".into(), min: smin, max: smax, values: speed },
        SliceField { name: "pressure".into(), label: "Pressure".into(), unit: "Pa".into(), min: pmin, max: pmax, values: pres },
    ];
}

/// Mean of a sequence of vectors.
pub fn mean3(v: &[Vec3]) -> Vec3 {
    let mut s = [0.0; 3];
    for x in v {
        s = add(s, *x);
    }
    scale(s, 1.0 / v.len().max(1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knn_matches_brute_force() {
        let mut pts = vec![];
        let mut s = 12345u64;
        let mut rnd = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..2000 {
            pts.push([rnd(), rnd() * 0.3, rnd() * 2.0]);
        }
        let knn = Knn::new(pts.clone());
        for _ in 0..50 {
            let x = [rnd() * 1.2 - 0.1, rnd() * 0.5, rnd() * 2.0];
            let got = knn.nearest(x, 4);
            let mut all: Vec<(f64, usize)> = pts.iter().enumerate().map(|(i, p)| (norm(sub(*p, x)), i)).collect();
            all.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(got.iter().map(|g| g.1).collect::<Vec<_>>(), all[..4].iter().map(|g| g.1).collect::<Vec<_>>());
        }
    }

    #[test]
    fn cloud_rows_match_points_in_any_order() {
        let dir = std::env::temp_dir().join(format!("simlab-ref-cloud-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pts = vec![[0.0, 0.0, 0.0], [0.001, 0.0, 0.0], [0.002, 0.5, 0.25]];
        std::fs::write(dir.join("s_p.xy"), "# x y z p\n2.0000000e-03 5.0000000e-01 2.5000000e-01 7\n0 0 0 3\n").unwrap();
        std::fs::write(dir.join("s_U.xy"), "1.0000000e-03 0 0 1 2 2\n").unwrap();
        let (p, u) = read_cloud(&dir, "s", &pts).unwrap();
        assert_eq!(p, vec![Some(3.0), None, Some(7.0)]);
        assert_eq!(u[1], Some([1.0, 2.0, 2.0]));
        let mut sl = RefSlice { name: "s".into(), label: "".into(), origin_m: [0.0; 3], u_m: [1.0, 0.0, 0.0], v_m: [0.0, 1.0, 0.0], nu: 3, nv: 1, fields: vec![] };
        fill_slice(&mut sl, &p, &u, 2.0);
        assert_eq!(sl.fields[1].values[0], 6.0);
        assert!(sl.fields[0].values[0].is_nan());
        assert_eq!(sl.fields[0].values[1], 3.0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
