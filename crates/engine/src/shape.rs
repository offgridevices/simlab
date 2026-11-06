//! Is a still part rounded where the air leaves it, or sharp-edged?
//!
//! On a sharp-edged part (plate, cube, bar, frame, propeller blade) the air
//! breaks away at the edges whatever its speed, and OpenFOAM's steady model
//! gets the drag within a few percent. On a rounded part (ball, dome, round
//! rod across the wind) the break-away point slides over the smooth surface
//! and the wake swings; above Reynolds number about 10,000 OpenFOAM reads
//! such parts 10–26% low while the GPU tunnel stays within about 3–11%
//! (head-to-head runs, November 2026, docs/VALIDATION.md).
//!
//! The break-away happens on the part's outline as the wind sees it: the
//! line where the surface turns from facing the wind to facing away. That
//! line is found as the mesh edges whose two faces lie on either side of
//! "square to the wind". Each such edge is sharp when the surface turns
//! through a large angle there, or curves with a radius small next to the
//! part's thickness across the wind; otherwise it is rounded. The answer
//! is the share of the outline (its length seen from upwind) that is
//! rounded.

use simlab_core::{add, dot, norm, normalize, scale, sub, TriMesh, Vec3};
use std::collections::HashMap;

/// Turning angle (degrees) above which an outline edge is sharp outright.
const SHARP_DEG: f64 = 50.0;
/// Below this radius of curvature, as a share of the part's thickness
/// across the wind, a turn counts as a sharp edge (a small fillet on a
/// bar behaves like a corner).
const SHARP_RADIUS: f64 = 0.1;
/// Above this radius of curvature, as a share of the thickness, the surface
/// is nearly flat where it lies along the wind (a thin twisted blade seen
/// edge-on): the air does not leave it there, so the edge is not part of
/// the outline that decides the drag.
const FLAT_RADIUS: f64 = 2.0;
/// Share of the outline that must be rounded for the part to count as rounded.
pub const ROUNDED_SHARE: f64 = 0.5;
/// Reynolds number from which OpenFOAM reads rounded parts clearly low
/// (a sphere at 10,000: −8% at Normal; 20,000: −9 to −15%; 100,000: −26%).
pub const RE_ROUNDED_LOW: f64 = 10_000.0;

#[derive(Debug, Clone, Copy)]
pub struct Outline {
    /// share of the outline, by length seen from upwind, that is rounded (0–1)
    pub rounded_share: f64,
    /// outline length seen from upwind, in the mesh's units (0 when none was found)
    pub length: f64,
    /// the part's thickness across the wind (its smaller extent square to
    /// it), in the mesh's units: the size its Reynolds number goes by here
    pub thickness: f64,
}

impl Outline {
    pub fn rounded(&self) -> bool {
        self.length > 0.0 && self.rounded_share >= ROUNDED_SHARE
    }
}

/// The part's outline as the wind (`wind_dir`, the way the air moves) sees it.
pub fn outline(mesh: &TriMesh, wind_dir: Vec3) -> Outline {
    let w = normalize(wind_dir);
    let none = Outline { rounded_share: 0.0, length: 0.0, thickness: 0.0 };
    if mesh.triangles.is_empty() || !w.iter().all(|x| x.is_finite()) {
        return none;
    }
    // weld by position: STL readers may keep one copy of a vertex per triangle
    let (lo, hi) = mesh.bounds();
    let diag = norm(sub(hi, lo)).max(1e-30);
    let q = 1e-7 * diag;
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let vid: Vec<u32> = mesh
        .positions
        .iter()
        .map(|p| {
            let k = [(p[0] / q).round() as i64, (p[1] / q).round() as i64, (p[2] / q).round() as i64];
            let n = ids.len() as u32;
            *ids.entry(k).or_insert(n)
        })
        .collect();

    // the part's thickness across the wind: its smaller extent square to it
    let a = if w[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = normalize(sub(a, scale(w, dot(a, w))));
    let v = simlab_core::cross(w, u);
    let (mut ul, mut uh, mut vl, mut vh) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for p in &mesh.positions {
        let (x, y) = (dot(*p, u), dot(*p, v));
        ul = ul.min(x);
        uh = uh.max(x);
        vl = vl.min(y);
        vh = vh.max(y);
    }
    let thick = (uh - ul).min(vh - vl).max(1e-30);

    let mut normal = Vec::with_capacity(mesh.triangles.len());
    let mut centre = Vec::with_capacity(mesh.triangles.len());
    for t in &mesh.triangles {
        let [p0, p1, p2] = t.map(|i| mesh.positions[i as usize]);
        let c = simlab_core::cross(sub(p1, p0), sub(p2, p0));
        let l = norm(c);
        normal.push(if l > 0.0 { scale(c, 1.0 / l) } else { [f64::NAN; 3] });
        centre.push(scale([p0[0] + p1[0] + p2[0], p0[1] + p1[1] + p2[1], p0[2] + p1[2] + p2[2]], 1.0 / 3.0));
    }
    // each edge (by welded ends): its first face, its second, and its ends
    type EdgeFaces = (u32, Option<u32>, [u32; 2]);
    let mut edges: HashMap<(u32, u32), EdgeFaces> = HashMap::new();
    let mut extra = std::collections::HashSet::new();
    for (f, t) in mesh.triangles.iter().enumerate() {
        for k in 0..3 {
            let (i, j) = (t[k], t[(k + 1) % 3]);
            let (a, b) = (vid[i as usize], vid[j as usize]);
            if a == b {
                continue;
            }
            let key = (a.min(b), a.max(b));
            match edges.get_mut(&key) {
                None => {
                    edges.insert(key, (f as u32, None, [i, j]));
                }
                Some(e) if e.1.is_none() => e.1 = Some(f as u32),
                Some(_) => {
                    extra.insert(key);
                }
            }
        }
    }

    // triangles on a grid (each in every cell its box touches), to find the
    // far side of a wall quickly
    let cell = thick / 16.0;
    let at = |p: Vec3| [(p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64, (p[2] / cell).floor() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    for (f, t) in mesh.triangles.iter().enumerate() {
        if !normal[f][0].is_finite() {
            continue;
        }
        let ps = t.map(|i| mesh.positions[i as usize]);
        let lo = at([0, 1, 2].map(|k| ps.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min)));
        let hi = at([0, 1, 2].map(|k| ps.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max)));
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    grid.entry([x, y, z]).or_default().push(f as u32);
                }
            }
        }
    }
    // the distance from `o` along `d` to triangle `f`, if the line hits it
    let hit = |o: Vec3, d: Vec3, f: usize| -> Option<f64> {
        let [a, b, c] = mesh.triangles[f].map(|i| mesh.positions[i as usize]);
        let (e1, e2) = (sub(b, a), sub(c, a));
        let p = simlab_core::cross(d, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-30 {
            return None;
        }
        let s = sub(o, a);
        let u = dot(s, p) / det;
        let q = simlab_core::cross(s, e1);
        let v = dot(d, q) / det;
        (u >= 0.0 && v >= 0.0 && u + v <= 1.0).then(|| dot(e2, q) / det)
    };
    // Is the part thinner than `depth` behind the surface at `m` (outward
    // normal `n`)? Looks straight in for the far side of the wall (a face
    // turned the other way) closer than `depth`.
    let thin_behind = |m: Vec3, n: Vec3, depth: f64| -> bool {
        let inward = scale(n, -1.0);
        let steps = (depth / cell).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let [x, y, z] = at(sub(m, scale(n, k as f64 * cell)));
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(fs) = grid.get(&[x + dx, y + dy, z + dz]) else { continue };
                        for &f in fs {
                            if dot(normal[f as usize], n) > -0.3 {
                                continue;
                            }
                            if hit(m, inward, f as usize).is_some_and(|t| t > 1e-9 * depth && t <= depth) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    };

    let (mut rounded, mut total) = (0.0, 0.0);
    for (key, (f1, f2, [i, j])) in &edges {
        let Some(f2) = f2 else { continue };
        if extra.contains(key) {
            continue;
        }
        let (n1, n2) = (normal[*f1 as usize], normal[*f2 as usize]);
        if !n1[0].is_finite() || !n2[0].is_finite() {
            continue;
        }
        // on the outline: the faces lie on either side of square-to-the-wind
        if dot(n1, w) * dot(n2, w) > 0.0 {
            continue;
        }
        let turn = dot(n1, n2).clamp(-1.0, 1.0).acos();
        if turn < 1f64.to_radians() {
            continue;
        }
        // its length seen from upwind
        let e = sub(mesh.positions[*j as usize], mesh.positions[*i as usize]);
        let seen = norm(sub(e, scale(w, dot(e, w))));
        if seen <= 0.0 {
            continue;
        }
        // the faces' centres measured across the edge, not along it (long
        // thin triangles on a rod would otherwise look far apart)
        let along = normalize(e);
        let dc = sub(centre[*f2 as usize], centre[*f1 as usize]);
        let gap = norm(sub(dc, scale(along, dot(dc, along))));
        let radius = gap / turn;
        if turn <= SHARP_DEG.to_radians() && radius > FLAT_RADIUS * thick {
            continue;
        }
        let mut sharp = turn > SHARP_DEG.to_radians() || radius < SHARP_RADIUS * thick;
        // a curved stretch of a wall thinner than its own curve (a blade, a
        // shell) is a thin edge to the air, not a rounded body
        if !sharp {
            let m = scale(add(mesh.positions[*i as usize], mesh.positions[*j as usize]), 0.5);
            sharp = thin_behind(m, normalize(add(n1, n2)), radius);
        }
        total += seen;
        if !sharp {
            rounded += seen;
        }
    }
    if total <= 0.0 {
        return Outline { thickness: thick, ..none };
    }
    Outline { rounded_share: rounded / total, length: total, thickness: thick }
}

/// The plain-words advice for a rounded still part at this Reynolds
/// number, when it applies (None for sharp-edged parts and slow air).
pub fn rounded_advice(o: &Outline, reynolds: f64, engine: &str) -> Option<String> {
    // a NaN Reynolds number gives no advice
    if !o.rounded() || reynolds.is_nan() || reynolds < RE_ROUNDED_LOW {
        return None;
    }
    Some(if engine == "native" {
        "This part looks rounded where the air leaves it. At this speed the quick preview has come closer than OpenFOAM on rounded parts (a ball within about 3–11%, where OpenFOAM read 9–26% low), so this number is worth more than a usual preview.".to_string()
    } else {
        "This part looks rounded where the air leaves it. At this speed OpenFOAM reads rounded parts low (a ball 9–26% low), and the quick preview has come closer (within about 3–11%). Run the quick preview too, and if the two differ by more than 10%, trust the preview.".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(r: f64, n: usize) -> TriMesh {
        let mut positions = vec![];
        let mut triangles = vec![];
        let (nt, np) = (n, 2 * n);
        for i in 0..=nt {
            let t = std::f64::consts::PI * i as f64 / nt as f64;
            for j in 0..np {
                let p = 2.0 * std::f64::consts::PI * j as f64 / np as f64;
                positions.push([r * t.sin() * p.cos(), r * t.sin() * p.sin(), r * t.cos()]);
            }
        }
        for i in 0..nt {
            for j in 0..np {
                let a = (i * np + j) as u32;
                let b = (i * np + (j + 1) % np) as u32;
                let c = a + np as u32;
                let d = b + np as u32;
                triangles.push([a, c, b]);
                triangles.push([b, c, d]);
            }
        }
        let patch = vec![0; triangles.len()];
        TriMesh { positions, triangles, patch }
    }

    /// a box, with one copy of each corner per triangle (like an STL)
    fn cuboid(s: Vec3) -> TriMesh {
        let c = |x: usize, y: usize, z: usize| [x as f64 * s[0], y as f64 * s[1], z as f64 * s[2]];
        let quads = [
            [c(0, 0, 0), c(0, 1, 0), c(1, 1, 0), c(1, 0, 0)],
            [c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)],
            [c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)],
            [c(0, 1, 0), c(0, 1, 1), c(1, 1, 1), c(1, 1, 0)],
            [c(0, 0, 0), c(0, 0, 1), c(0, 1, 1), c(0, 1, 0)],
            [c(1, 0, 0), c(1, 1, 0), c(1, 1, 1), c(1, 0, 1)],
        ];
        let mut positions = vec![];
        let mut triangles = vec![];
        for q in quads {
            for t in [[0, 1, 2], [0, 2, 3]] {
                let b = positions.len() as u32;
                for k in t {
                    positions.push(q[k]);
                }
                triangles.push([b, b + 1, b + 2]);
            }
        }
        let patch = vec![0; triangles.len()];
        TriMesh { positions, triangles, patch }
    }

    /// a round rod along z, closed with flat ends
    fn rod(r: f64, l: f64, n: usize) -> TriMesh {
        let mut positions = vec![[0.0, 0.0, 0.0], [0.0, 0.0, l]];
        for z in [0.0, l] {
            for j in 0..n {
                let p = 2.0 * std::f64::consts::PI * j as f64 / n as f64;
                positions.push([r * p.cos(), r * p.sin(), z]);
            }
        }
        let mut triangles = vec![];
        for j in 0..n {
            let (a, b) = ((2 + j) as u32, (2 + (j + 1) % n) as u32);
            let (c, d) = (a + n as u32, b + n as u32);
            triangles.push([0, b, a]);
            triangles.push([1, c, d]);
            triangles.push([a, b, d]);
            triangles.push([a, d, c]);
        }
        let patch = vec![0; triangles.len()];
        TriMesh { positions, triangles, patch }
    }

    #[test]
    fn balls_are_rounded() {
        for n in [12, 24, 64] {
            let o = outline(&sphere(10.0, n), [1.0, 0.0, 0.0]);
            assert!(o.rounded() && o.rounded_share > 0.95, "{n}: {o:?}");
        }
        // any wind direction
        let o = outline(&sphere(10.0, 32), [0.3, -0.5, 0.8]);
        assert!(o.rounded(), "{o:?}");
    }

    #[test]
    fn boxes_plates_and_bars_are_sharp() {
        for (s, w) in [
            ([20.0, 20.0, 20.0], [1.0, 0.0, 0.0]),
            ([20.0, 20.0, 20.0], [1.0, 1.0, 0.3]),
            ([1.0, 20.0, 20.0], [1.0, 0.0, 0.0]),
            ([150.0, 12.0, 6.0], [0.0, 1.0, 0.0]),
            ([150.0, 12.0, 6.0], [0.0, 0.0, 1.0]),
        ] {
            let o = outline(&cuboid(s), w);
            assert!(!o.rounded() && o.rounded_share < 0.05, "{s:?} {w:?}: {o:?}");
        }
    }

    #[test]
    fn a_rod_across_the_wind_is_rounded_but_end_on_it_is_sharp() {
        let m = rod(5.0, 50.0, 32);
        let across = outline(&m, [1.0, 0.0, 0.0]);
        assert!(across.rounded(), "{across:?}");
        let end_on = outline(&m, [0.0, 0.0, 1.0]);
        assert!(!end_on.rounded(), "{end_on:?}");
    }

    #[test]
    fn a_thin_curved_shell_is_sharp() {
        // a 0.5 mm-thick half pipe, curved like a bent blade, edge-on to the wind
        let (r, t, l, n) = (6.0, 0.5, 40.0, 48);
        let mut positions = vec![];
        for rr in [r, r - t] {
            for z in [0.0, l] {
                for j in 0..=n {
                    let p = std::f64::consts::PI * j as f64 / n as f64;
                    positions.push([rr * p.cos(), rr * p.sin(), z]);
                }
            }
        }
        let ring = |side: usize, end: usize, j: usize| (side * 2 * (n + 1) + end * (n + 1) + j) as u32;
        let mut triangles = vec![];
        for j in 0..n {
            // outer, facing out; inner, facing in
            let (a, b, c, d) = (ring(0, 0, j), ring(0, 0, j + 1), ring(0, 1, j), ring(0, 1, j + 1));
            triangles.extend([[a, b, d], [a, d, c]]);
            let (a, b, c, d) = (ring(1, 0, j), ring(1, 0, j + 1), ring(1, 1, j), ring(1, 1, j + 1));
            triangles.extend([[a, d, b], [a, c, d]]);
            // the two ends
            let (a, b, c, d) = (ring(0, 0, j), ring(0, 0, j + 1), ring(1, 0, j), ring(1, 0, j + 1));
            triangles.extend([[a, c, d], [a, d, b]]);
            let (a, b, c, d) = (ring(0, 1, j), ring(0, 1, j + 1), ring(1, 1, j), ring(1, 1, j + 1));
            triangles.extend([[a, d, c], [a, b, d]]);
        }
        let patch = vec![0; triangles.len()];
        let m = TriMesh { positions, triangles, patch };
        let o = outline(&m, [1.0, 0.0, 0.0]);
        assert!(!o.rounded(), "{o:?}");
    }

    #[test]
    fn advice_only_for_rounded_parts_in_fast_air() {
        let ball = outline(&sphere(10.0, 32), [1.0, 0.0, 0.0]);
        let cube = outline(&cuboid([20.0; 3]), [1.0, 0.0, 0.0]);
        assert!(rounded_advice(&ball, 20_000.0, "reference").unwrap().contains("trust the preview"));
        assert!(rounded_advice(&ball, 20_000.0, "native").is_some());
        assert!(rounded_advice(&ball, 2_000.0, "reference").is_none());
        assert!(rounded_advice(&cube, 20_000.0, "reference").is_none());
    }
}

#[cfg(test)]
mod real_parts {
    use super::*;

    /// Real part files: SIMLAB_SHAPES names a folder of STLs.
    /// `SIMLAB_SHAPES=… cargo test -p simlab-engine real_parts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_outlines() {
        let Ok(dir) = std::env::var("SIMLAB_SHAPES") else { return };
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "stl")).collect();
        files.sort();
        for p in files {
            let m = simlab_core::stl::read_stl(&std::fs::read(&p).unwrap()).unwrap();
            let s: Vec<String> = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .iter()
                .map(|w| {
                    let o = outline(&m, *w);
                    format!("{:.2}{}", o.rounded_share, if o.rounded() { " ROUND" } else { "" })
                })
                .collect();
            println!("{:<20} {} tris  x {:<12} y {:<12} z {}", p.file_name().unwrap().to_string_lossy(), m.triangles.len(), s[0], s[1], s[2]);
        }
    }
}
