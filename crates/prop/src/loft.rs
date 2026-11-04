//! Builds a 3-D propeller surface (TriMesh) from blade geometry: each
//! station's airfoil is wrapped onto its cylinder r = const at the station's
//! pitch angle, and the stations are lofted into a closed blade. Used to
//! check the geometry extraction, and handy for previews.

use crate::extract::Frame;
use crate::geom::BladeGeometry;
use simlab_core::{TriMesh, Vec3};
use std::f64::consts::{PI, TAU};

/// Section outline in chord coordinates (x: 0 at the leading edge, 1 at the
/// trailing edge; y toward the thrust side).
#[derive(Clone, Debug)]
pub enum Airfoil {
    /// parabolic camber line with a NACA 4-digit thickness distribution
    /// (closed trailing edge), scaled to the station's thickness and camber
    Cambered,
    /// Selig-format coordinates (upper surface TE -> LE, then lower LE -> TE),
    /// used at every station as given
    Coordinates(Vec<[f64; 2]>),
}

impl Airfoil {
    /// Parses a Selig .dat file (first line is the name).
    pub fn parse_dat(text: &str) -> Airfoil {
        let pts = text
            .lines()
            .filter_map(|l| {
                let v: Vec<f64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() == 2 { Some([v[0], v[1]]) } else { None }
            })
            .collect();
        Airfoil::Coordinates(pts)
    }
}

pub struct LoftOptions {
    /// points around each section
    pub section_points: usize,
    /// radial rings (stations are resampled to this many)
    pub rings: usize,
    /// chordwise position of the stacking (pitch) axis
    pub stack_x: f64,
    /// hub cylinder: (outer radius, half height, bore radius), metres
    pub hub: Option<(f64, f64, f64)>,
    /// blade root starts this far inside the hub radius (buries the root)
    pub root_overlap: f64,
}

impl Default for LoftOptions {
    fn default() -> Self {
        LoftOptions { section_points: 80, rings: 70, stack_x: 0.4, hub: None, root_overlap: 0.0 }
    }
}

/// closed outline, `n` points: upper TE -> LE, lower LE -> TE (cosine spacing)
fn outline(af: &Airfoil, t: f64, f: f64, n: usize) -> Vec<[f64; 2]> {
    let half = n / 2;
    let xs: Vec<f64> = (0..=half).map(|i| 0.5 * (1.0 - (PI * i as f64 / half as f64).cos())).collect();
    match af {
        Airfoil::Cambered => {
            let yt = |x: f64| 5.0 * t * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x.powi(3) - 0.1036 * x.powi(4));
            let surf = |x: f64, up: bool| -> [f64; 2] {
                let yc = 4.0 * f * x * (1.0 - x);
                let th = (4.0 * f * (1.0 - 2.0 * x)).atan();
                let s = if up { 1.0 } else { -1.0 };
                [x - s * yt(x) * th.sin(), yc + s * yt(x) * th.cos()]
            };
            let mut out = vec![];
            for &x in xs.iter().rev() {
                out.push(surf(x, true));
            }
            for &x in xs.iter().skip(1).take(half - 1) {
                out.push(surf(x, false));
            }
            out
        }
        Airfoil::Coordinates(p) => {
            // split at the leading edge (min x), resample both sides
            let ile = (0..p.len()).min_by(|&a, &b| p[a][0].partial_cmp(&p[b][0]).unwrap()).unwrap_or(0);
            let mut up: Vec<[f64; 2]> = p[..=ile].to_vec();
            up.reverse();
            let lo: Vec<[f64; 2]> = p[ile..].to_vec();
            let interp = |c: &[[f64; 2]], x: f64| -> f64 {
                for w in c.windows(2) {
                    if (w[0][0] - x) * (w[1][0] - x) <= 0.0 && w[0][0] != w[1][0] {
                        return w[0][1] + (w[1][1] - w[0][1]) * (x - w[0][0]) / (w[1][0] - w[0][0]);
                    }
                }
                if x <= c[0][0] { c[0][1] } else { c[c.len() - 1][1] }
            };
            let mut out = vec![];
            for &x in xs.iter().rev() {
                out.push([x, interp(&up, x)]);
            }
            for &x in xs.iter().skip(1).take(half - 1) {
                out.push([x, interp(&lo, x)]);
            }
            out
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn add_cap(tris: &mut Vec<[u32; 3]>, pos: &mut Vec<Vec3>, ring: &[u32], flip: bool) {
    let mut c = [0.0; 3];
    for &i in ring {
        for k in 0..3 {
            c[k] += pos[i as usize][k] / ring.len() as f64;
        }
    }
    let ci = pos.len() as u32;
    pos.push(c);
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        tris.push(if flip { [ci, b, a] } else { [ci, a, b] });
    }
}

#[allow(clippy::needless_range_loop)]
/// The blades (and optional hub) as one triangle soup of closed shells.
/// `frame` places the prop (metres); blades turn in `geom.rotation`.
pub fn loft(geom: &BladeGeometry, af: &Airfoil, frame: &Frame, o: &LoftOptions) -> TriMesh {
    let mut pos: Vec<Vec3> = vec![];
    let mut tris: Vec<[u32; 3]> = vec![];
    let rot = geom.rotation.sign();
    let r0 = geom.stations.first().map(|s| s.r).unwrap_or(geom.hub_radius) - o.root_overlap;
    let r1 = geom.radius;
    let n = o.section_points.max(8) & !1;
    let rings = o.rings.max(3);
    let place = |r: f64, theta: f64, z: f64| -> Vec3 {
        let (s, c) = theta.sin_cos();
        let mut p = frame.center;
        for k in 0..3 {
            p[k] += r * (c * frame.e1[k] + s * frame.e2[k]) + z * frame.axis[k];
        }
        p
    };
    for blade in 0..geom.blades {
        let th0 = TAU * blade as f64 / geom.blades as f64;
        let base = pos.len() as u32;
        for j in 0..rings {
            // rings clustered toward the tip
            let u = j as f64 / (rings - 1) as f64;
            let r = r0 + (r1 - r0) * (0.5 * PI * u).sin();
            let st = geom.at(r.max(geom.stations[0].r));
            let chord = st.chord.max(1e-4 * r1);
            let ol = outline(af, st.thickness, st.camber, n);
            let (sb, cb) = st.beta.sin_cos();
            for p in &ol {
                let xr = (p[0] - o.stack_x) * chord;
                let yr = p[1] * chord;
                // unwrapped plane, blade moving toward +s for rot = +1
                let s = -xr * cb - yr * sb;
                let z = -xr * sb + yr * cb;
                pos.push(place(r, th0 + rot * s / r, z));
            }
        }
        for j in 0..rings - 1 {
            for i in 0..n {
                let a = base + (j * n + i) as u32;
                let b = base + (j * n + (i + 1) % n) as u32;
                let c = a + n as u32;
                let d = b + n as u32;
                tris.push([a, b, d]);
                tris.push([a, d, c]);
            }
        }
        let root: Vec<u32> = (0..n as u32).map(|i| base + i).collect();
        let tip: Vec<u32> = (0..n as u32).map(|i| base + ((rings - 1) * n) as u32 + i).collect();
        add_cap(&mut tris, &mut pos, &root, true);
        add_cap(&mut tris, &mut pos, &tip, false);
    }
    // orient blades outward
    let mut m = TriMesh { positions: pos, triangles: tris, patch: vec![] };
    if m.volume() < 0.0 {
        for t in &mut m.triangles {
            t.swap(1, 2);
        }
    }
    if let Some((rh, hh, rb)) = o.hub {
        let k = 64;
        let base = m.positions.len() as u32;
        for &z in &[-hh, hh] {
            for &r in &[rb, rh] {
                for i in 0..k {
                    let th = TAU * i as f64 / k as f64;
                    m.positions.push(place(r, th, z));
                }
            }
        }
        // rings: 0 = bottom bore, 1 = bottom outer, 2 = top bore, 3 = top outer
        let ring = |q: u32, i: usize| base + q * k as u32 + (i % k) as u32;
        let mut hub_tris = vec![];
        for i in 0..k {
            let quad = |hub_tris: &mut Vec<[u32; 3]>, a: u32, b: u32, c: u32, d: u32| {
                hub_tris.push([a, b, c]);
                hub_tris.push([a, c, d]);
            };
            quad(&mut hub_tris, ring(1, i), ring(1, i + 1), ring(3, i + 1), ring(3, i)); // outer wall
            quad(&mut hub_tris, ring(0, i + 1), ring(0, i), ring(2, i), ring(2, i + 1)); // bore
            quad(&mut hub_tris, ring(2, i), ring(3, i), ring(3, i + 1), ring(2, i + 1)); // top
            quad(&mut hub_tris, ring(0, i + 1), ring(1, i + 1), ring(1, i), ring(0, i)); // bottom
        }
        let hm = TriMesh { positions: m.positions.clone(), triangles: hub_tris.clone(), patch: vec![] };
        if hm.volume() < 0.0 {
            for t in &mut hub_tris {
                t.swap(1, 2);
            }
        }
        m.triangles.extend(hub_tris);
    }
    m.patch = vec![0; m.triangles.len()];
    m
}
