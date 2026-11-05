//! Simple test shapes as triangle meshes (metres or any unit): boxes,
//! spheres and a parametric propeller.

use simlab_core::{TriMesh, Vec3};
use std::collections::HashMap;
use std::f64::consts::PI;

/// Axis-aligned box, outward-facing triangles.
pub fn cuboid(lo: Vec3, hi: Vec3) -> TriMesh {
    let p = |i: usize| -> Vec3 { [if i & 1 != 0 { hi[0] } else { lo[0] }, if i & 2 != 0 { hi[1] } else { lo[1] }, if i & 4 != 0 { hi[2] } else { lo[2] }] };
    let positions: Vec<Vec3> = (0..8).map(p).collect();
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut triangles = Vec::new();
    for q in quads {
        triangles.push([q[0], q[1], q[2]]);
        triangles.push([q[0], q[2], q[3]]);
    }
    let patch = vec![0; triangles.len()];
    TriMesh { positions, triangles, patch }
}

/// Thin circular disk of diameter `d` and thickness `t`, axis along x.
pub fn disk(center: Vec3, d: f64, t: f64) -> TriMesh {
    let n = 96usize;
    let mut m = TriMesh::default();
    for side in [-0.5, 0.5] {
        for k in 0..n {
            let a = 2.0 * PI * k as f64 / n as f64;
            m.positions.push([center[0] + side * t, center[1] + 0.5 * d * a.cos(), center[2] + 0.5 * d * a.sin()]);
        }
        m.positions.push([center[0] + side * t, center[1], center[2]]);
    }
    let (c0, c1) = (n as u32, 2 * n as u32 + 1);
    for k in 0..n as u32 {
        let k1 = (k + 1) % n as u32;
        let (a2, b2) = (k + n as u32 + 1, k1 + n as u32 + 1);
        m.triangles.push([c0, k1, k]);
        m.triangles.push([c1, a2, b2]);
        m.triangles.push([k, k1, b2]);
        m.triangles.push([k, b2, a2]);
    }
    m.patch = vec![0; m.triangles.len()];
    fix_orientation(&mut m);
    m
}

/// Geodesic sphere (subdivided icosahedron), outward-facing.
pub fn sphere(center: Vec3, radius: f64, subdiv: u32) -> TriMesh {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = vec![
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ];
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for p in v.iter_mut() {
        *p = simlab_core::normalize(*p);
    }
    for _ in 0..subdiv {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut nf = Vec::with_capacity(f.len() * 4);
        let mut get = |a: u32, b: u32, v: &mut Vec<Vec3>| -> u32 {
            let k = (a.min(b), a.max(b));
            *mid.entry(k).or_insert_with(|| {
                let m = simlab_core::normalize(simlab_core::add(v[a as usize], v[b as usize]));
                v.push(m);
                (v.len() - 1) as u32
            })
        };
        for [a, b, c] in f {
            let ab = get(a, b, &mut v);
            let bc = get(b, c, &mut v);
            let ca = get(c, a, &mut v);
            nf.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = nf;
    }
    let positions = v.iter().map(|p| simlab_core::add(center, simlab_core::scale(*p, radius))).collect();
    let patch = vec![0; f.len()];
    TriMesh { positions, triangles: f, patch }
}

/// A simple fixed-pitch propeller, axis +z through the origin, closed and
/// outward-facing. Blades: elliptic-ish chord, constant geometric pitch,
/// flat-bottom section with a thickness of `thick` × chord; a cylindrical hub.
/// Rotating with positive rpm about +z (counter-clockwise seen from +z), the
/// blades push air toward -z, so the thrust on the part is along +z.
#[derive(Clone, Debug)]
pub struct PropSpec {
    pub radius: f64,
    pub hub_radius: f64,
    pub hub_height: f64,
    pub blades: u32,
    /// geometric pitch (advance per revolution), same unit as radius
    pub pitch: f64,
    /// chord at 75 % radius
    pub chord: f64,
    /// section thickness / chord
    pub thick: f64,
}

impl PropSpec {
    /// A 5-inch-class prop (5x4.3 pitch) in metres.
    pub fn five_inch() -> PropSpec {
        PropSpec { radius: 0.0635, hub_radius: 0.0075, hub_height: 0.008, blades: 2, pitch: 0.109, chord: 0.0145, thick: 0.10 }
    }

    pub fn chord_at(&self, r: f64) -> f64 {
        let x = (r / self.radius).clamp(0.0, 1.0);
        // grows from the root, peaks near 0.6 R, rounds off to the tip
        let shape = (1.0 - x * x).max(0.0).sqrt() * (0.55 + 0.9 * x - 0.4 * x * x);
        let s75 = (1.0 - 0.5625f64).sqrt() * (0.55 + 0.9 * 0.75 - 0.4 * 0.5625);
        (self.chord * shape / s75).max(0.25 * self.chord)
    }
    pub fn pitch_angle(&self, r: f64) -> f64 {
        (self.pitch / (2.0 * PI * r)).atan()
    }

    pub fn mesh(&self) -> TriMesh {
        let mut m = TriMesh::default();
        // hub cylinder
        let ns = 24;
        let (h0, h1) = (-self.hub_height / 2.0, self.hub_height / 2.0);
        let base = m.positions.len() as u32;
        for k in 0..ns {
            let a = 2.0 * PI * k as f64 / ns as f64;
            let (c, s) = (a.cos() * self.hub_radius, a.sin() * self.hub_radius);
            m.positions.push([c, s, h0]);
            m.positions.push([c, s, h1]);
        }
        let cb = m.positions.len() as u32;
        m.positions.push([0.0, 0.0, h0]);
        m.positions.push([0.0, 0.0, h1]);
        for k in 0..ns {
            let a0 = base + 2 * k;
            let a1 = base + 2 * ((k + 1) % ns);
            m.triangles.push([a0, a1, a1 + 1]);
            m.triangles.push([a0, a1 + 1, a0 + 1]);
            m.triangles.push([cb, a1, a0]);
            m.triangles.push([cb + 1, a0 + 1, a1 + 1]);
        }
        // blades: sections from just inside the hub to the tip
        let nr = 24;
        let nc = 16;
        for b in 0..self.blades {
            let phase = 2.0 * PI * b as f64 / self.blades as f64;
            let first = m.positions.len() as u32;
            let r0 = self.hub_radius * 0.8;
            for j in 0..=nr {
                let tt = j as f64 / nr as f64;
                let r = r0 + (self.radius - r0) * (1.0 - (1.0 - tt) * (1.0 - tt) * 0.0) * tt.max(0.0);
                let r = if j == nr { self.radius * 0.999 } else { r };
                let c = self.chord_at(r);
                let beta = self.pitch_angle(r);
                // section in (chordwise s, thickness t) then placed on the
                // cylinder: leading edge ahead in the +rotation direction (+phi)
                for i in 0..nc {
                    // upper surface 0..nc/2, lower surface back
                    let (s, t) = section_point(i, nc, self.thick);
                    // s in [0,1] from leading edge; t thickness offset (fraction of chord)
                    let sc = (0.35 - s) * c; // chordwise, + toward leading edge, pivot at 35 %
                    let tc = t * c;
                    // rotate by pitch angle: along the rotation direction (y local)
                    // and the axial direction (z): leading edge sits higher (+z)
                    let along = sc * beta.cos() - tc * beta.sin();
                    let up = sc * beta.sin() + tc * beta.cos();
                    let phi = phase + along / r;
                    m.positions.push([r * phi.cos(), r * phi.sin(), up]);
                }
            }
            for j in 0..nr {
                for i in 0..nc {
                    let a = first + (j * nc + i) as u32;
                    let b2 = first + (j * nc + (i + 1) % nc) as u32;
                    let c2 = a + nc as u32;
                    let d = b2 + nc as u32;
                    m.triangles.push([a, c2, d]);
                    m.triangles.push([a, d, b2]);
                }
            }
            // caps
            for (ring, flip) in [(0u32, true), (nr as u32, false)] {
                let r0i = first + ring * nc as u32;
                for i in 1..(nc as u32 - 1) {
                    if flip {
                        m.triangles.push([r0i, r0i + i, r0i + i + 1]);
                    } else {
                        m.triangles.push([r0i, r0i + i + 1, r0i + i]);
                    }
                }
            }
        }
        // orient every closed piece outward (volume positive)
        m.patch = vec![0; m.triangles.len()];
        fix_orientation(&mut m);
        m
    }
}

/// Points around a flat-bottomed, rounded-top section: index 0 at the
/// leading edge, then over the top to the trailing edge, back along the bottom.
fn section_point(i: usize, nc: usize, thick: f64) -> (f64, f64) {
    let half = nc / 2;
    if i <= half {
        let s = 0.5 - 0.5 * (PI * i as f64 / half as f64).cos();
        let t = thick * 2.6 * s.sqrt() * (1.0 - s) * 0.77 + 0.004;
        (s, t)
    } else {
        let k = i - half;
        let s = 0.5 + 0.5 * (PI * k as f64 / half as f64).cos();
        (s, -0.004)
    }
}

/// Flips connected components whose signed volume is negative.
pub fn fix_orientation(m: &mut TriMesh) {
    let n = m.positions.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let nx = p[y];
            p[y] = r;
            y = nx;
        }
        r
    }
    for t in &m.triangles {
        let a = find(&mut parent, t[0] as usize);
        for &v in &t[1..] {
            let b = find(&mut parent, v as usize);
            parent[b] = a;
        }
    }
    let mut vol: HashMap<usize, f64> = HashMap::new();
    let roots: Vec<usize> = m.triangles.iter().map(|t| find(&mut parent, t[0] as usize)).collect();
    for (t, &r) in m.triangles.iter().zip(&roots) {
        let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
        *vol.entry(r).or_default() += simlab_core::dot(a, simlab_core::cross(b, c)) / 6.0;
    }
    for (t, r) in m.triangles.iter_mut().zip(&roots) {
        if vol[r] < 0.0 {
            t.swap(1, 2);
        }
    }
}
