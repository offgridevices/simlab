//! Small geometry helpers for setting up a case: frames, projected area,
//! watertightness, which way a spinning part blows, vertex normals, and a
//! test sphere.

use simlab_core::{TriMesh, Vec3, add, cross, dot, norm, normalize, scale, sub};
use std::collections::HashMap;

/// A right-handed orthonormal frame. `e1` is the flow direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub e1: Vec3,
    pub e2: Vec3,
    pub e3: Vec3,
}

impl Frame {
    /// `e1` along `dir`; `e3` as close to model +z ("up") as possible, so for
    /// a horizontal wind e3 is the lift direction and e2 = e3 × e1 is the
    /// side direction (to the left, looking downstream).
    pub fn along(dir: Vec3) -> Frame {
        let e1 = normalize(dir);
        let up = if dot(e1, [0.0, 0.0, 1.0]).abs() < 0.95 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };
        let e3 = normalize(sub(up, scale(e1, dot(up, e1))));
        let e2 = cross(e3, e1);
        Frame { e1, e2, e3 }
    }
    /// frame coordinates → model coordinates, about `origin`
    pub fn to_model(&self, origin: Vec3, p: Vec3) -> Vec3 {
        add(origin, add(add(scale(self.e1, p[0]), scale(self.e2, p[1])), scale(self.e3, p[2])))
    }
    pub fn to_frame(&self, origin: Vec3, p: Vec3) -> Vec3 {
        let d = sub(p, origin);
        [dot(d, self.e1), dot(d, self.e2), dot(d, self.e3)]
    }
}

/// Extents of the surface in a frame, relative to `origin`: (lo, hi).
pub fn extents_in(m: &TriMesh, f: &Frame, origin: Vec3) -> (Vec3, Vec3) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for &p in &m.positions {
        let q = f.to_frame(origin, p);
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    (lo, hi)
}

/// Area of the part's shadow on the plane normal to `dir` (m² for a mesh in
/// metres), by rasterising every triangle on a grid of about `res` pixels
/// across. Overlaps count once, as a frontal area should.
pub fn projected_area(m: &TriMesh, dir: Vec3, res: usize) -> f64 {
    let f = Frame::along(dir);
    let pts: Vec<[f64; 2]> = m.positions.iter().map(|&p| [dot(p, f.e2), dot(p, f.e3)]).collect();
    if pts.is_empty() {
        return 0.0;
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-30);
    let h = span / res as f64;
    let nx = ((hi[0] - lo[0]) / h).ceil() as usize + 1;
    let ny = ((hi[1] - lo[1]) / h).ceil() as usize + 1;
    let mut px = vec![false; nx * ny];
    for t in &m.triangles {
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        let d = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if d.abs() < 1e-30 {
            continue;
        }
        let i0 = ((a[0].min(b[0]).min(c[0]) - lo[0]) / h).floor().max(0.0) as usize;
        let i1 = (((a[0].max(b[0]).max(c[0]) - lo[0]) / h).ceil() as usize).min(nx - 1);
        let j0 = ((a[1].min(b[1]).min(c[1]) - lo[1]) / h).floor().max(0.0) as usize;
        let j1 = (((a[1].max(b[1]).max(c[1]) - lo[1]) / h).ceil() as usize).min(ny - 1);
        for j in j0..=j1 {
            let y = lo[1] + (j as f64 + 0.5) * h;
            for i in i0..=i1 {
                let x = lo[0] + (i as f64 + 0.5) * h;
                let w1 = ((b[0] - x) * (c[1] - y) - (b[1] - y) * (c[0] - x)) / d;
                let w2 = ((c[0] - x) * (a[1] - y) - (c[1] - y) * (a[0] - x)) / d;
                let w3 = 1.0 - w1 - w2;
                if w1 >= 0.0 && w2 >= 0.0 && w3 >= 0.0 {
                    px[j * nx + i] = true;
                }
            }
        }
    }
    px.iter().filter(|&&b| b).count() as f64 * h * h
}

/// Edges used by other than exactly two triangles (0 for a closed surface).
pub fn open_edges(m: &TriMesh) -> usize {
    let mut count: HashMap<(u32, u32), u32> = HashMap::with_capacity(m.triangles.len() * 2);
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    count.values().filter(|&&n| n != 2).count()
}

/// Area-weighted vertex normals (unit; outward for an outward-facing surface).
pub fn vertex_normals(m: &TriMesh) -> Vec<Vec3> {
    let mut n = vec![[0.0; 3]; m.positions.len()];
    for t in &m.triangles {
        let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
        let w = cross(sub(b, a), sub(c, a));
        for &i in t {
            n[i as usize] = add(n[i as usize], w);
        }
    }
    n.into_iter().map(normalize).collect()
}

/// Which way along `axis` a part spinning about `center` with angular
/// velocity vector `omega` (rad/s, right-hand rule) throws the air: +1 or −1.
///
/// A crude impact estimate: every face moving into the air (n·v > 0) pushes
/// it along its outward normal n, with a weight of area·(n·v)². For a
/// propeller this is its pressure side, so the sign is the wake direction;
/// the thrust on the part is the other way. Returns the sign and the
/// strength of the vote (|axial| / total, 0..1), so a weak vote can be
/// flagged.
pub fn jet_sign(m: &TriMesh, center: Vec3, omega: Vec3, axis: Vec3) -> (f64, f64) {
    let a = normalize(axis);
    let (mut axial, mut total) = (0.0, 0.0);
    for (i, t) in m.triangles.iter().enumerate() {
        let (p, q, r) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
        let c = scale(add(add(p, q), r), 1.0 / 3.0);
        let area = m.triangle_area(i);
        let n = m.triangle_normal(i);
        let v = cross(omega, sub(c, center));
        let vn = dot(n, v);
        if vn > 0.0 {
            let w = area * vn * vn;
            axial += w * dot(n, a);
            total += w;
        }
    }
    if total <= 0.0 {
        return (1.0, 0.0);
    }
    (if axial >= 0.0 { 1.0 } else { -1.0 }, axial.abs() / total)
}

/// Largest distance of any vertex from the axis through `center`, and the
/// axial extent (lo, hi) along `axis` relative to `center`.
pub fn radial_extent(m: &TriMesh, center: Vec3, axis: Vec3) -> (f64, f64, f64) {
    let a = normalize(axis);
    let (mut r, mut lo, mut hi) = (0.0f64, f64::INFINITY, f64::NEG_INFINITY);
    for &p in &m.positions {
        let d = sub(p, center);
        let z = dot(d, a);
        r = r.max(norm(sub(d, scale(a, z))));
        lo = lo.min(z);
        hi = hi.max(z);
    }
    (r, lo, hi)
}

/// A sphere of `radius` at `center`: an icosahedron subdivided
/// `levels` times (20·4^levels triangles), outward facing.
pub fn icosphere(center: Vec3, radius: f64, levels: u32) -> TriMesh {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut pos: Vec<Vec3> = [
        [-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0],
        [0.0, -1.0, t], [0.0, 1.0, t], [0.0, -1.0, -t], [0.0, 1.0, -t],
        [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|&p| normalize(p))
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    for _ in 0..levels {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut m = |a: u32, b: u32, pos: &mut Vec<Vec3>| -> u32 {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                pos.push(normalize(scale(add(pos[a as usize], pos[b as usize]), 0.5)));
                (pos.len() - 1) as u32
            })
        };
        for [a, b, c] in tris {
            let ab = m(a, b, &mut pos);
            let bc = m(b, c, &mut pos);
            let ca = m(c, a, &mut pos);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    let positions = pos.into_iter().map(|p| add(center, scale(p, radius))).collect();
    let patch = vec![0; tris.len()];
    TriMesh { positions, triangles: tris, patch }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_is_closed_and_outward() {
        let s = icosphere([0.0; 3], 1.0, 3);
        assert_eq!(open_edges(&s), 0);
        let v = s.volume();
        assert!((v - 4.0 / 3.0 * std::f64::consts::PI).abs() / v < 0.02, "volume {v}");
    }

    #[test]
    fn sphere_shadow_is_a_disc() {
        let s = icosphere([0.3, 0.1, 0.0], 0.5, 4);
        let a = projected_area(&s, [1.0, 0.2, 0.0], 800);
        let want = std::f64::consts::PI * 0.25;
        assert!((a - want).abs() / want < 0.01, "{a} vs {want}");
    }

    #[test]
    fn frame_is_right_handed() {
        for d in [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.3, -0.7, 0.2]] {
            let f = Frame::along(d);
            let c = cross(f.e1, f.e2);
            assert!(norm(sub(c, f.e3)) < 1e-12);
            let p = [0.1, 0.2, 0.3];
            let q = f.to_model([1.0, 2.0, 3.0], f.to_frame([1.0, 2.0, 3.0], p));
            assert!(norm(sub(p, q)) < 1e-12);
        }
        let f = Frame::along([1.0, 0.0, 0.0]);
        assert!(norm(sub(f.e3, [0.0, 0.0, 1.0])) < 1e-12);
        assert!(norm(sub(f.e2, [0.0, 1.0, 0.0])) < 1e-12);
    }

    /// A flat plate tilted like a blade: moving +y at x>0, its front face
    /// faces +y and −z, so it throws the air down (−z).
    #[test]
    fn tilted_paddle_blows_down() {
        // two blades, each a thin closed box pitched 20°, spinning about +z
        let mut m = TriMesh::default();
        for side in [1.0f64, -1.0] {
            let pitch = 20f64.to_radians();
            // leading direction at this blade: +y for side=+1 (ω = +z, r = +x)
            let lead = [0.0, side, 0.0];
            // chord tilted: the leading edge is higher (+z) than the trailing edge
            let chord = normalize(add(scale(lead, pitch.cos()), [0.0, 0.0, pitch.sin()]));
            let thick = cross(chord, [side, 0.0, 0.0]);
            let mut corners = vec![];
            for &r in &[0.02, 0.1] {
                for &c in &[-0.01, 0.01] {
                    for &t in &[-0.001, 0.001] {
                        corners.push(add(add([side * r, 0.0, 0.0], scale(chord, c)), scale(thick, t)));
                    }
                }
            }
            let o = m.positions.len() as u32;
            m.positions.extend(corners);
            // box faces (indices r*4 + c*2 + t)
            let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
            for q in quads {
                m.triangles.push([o + q[0], o + q[1], o + q[2]]);
                m.triangles.push([o + q[0], o + q[2], o + q[3]]);
            }
        }
        // make every triangle face outward from its blade's centre
        let n_tri = m.triangles.len();
        for i in 0..n_tri {
            let t = m.triangles[i];
            let c = scale(add(add(m.positions[t[0] as usize], m.positions[t[1] as usize]), m.positions[t[2] as usize]), 1.0 / 3.0);
            let blade_c = [if c[0] > 0.0 { 0.06 } else { -0.06 }, 0.0, 0.0];
            if dot(m.triangle_normal(i), sub(c, blade_c)) < 0.0 {
                m.triangles[i].swap(1, 2);
            }
        }
        m.patch = vec![0; m.triangles.len()];
        let (s, vote) = jet_sign(&m, [0.0; 3], [0.0, 0.0, 100.0], [0.0, 0.0, 1.0]);
        assert_eq!(s, -1.0, "vote {vote}");
        let (s2, _) = jet_sign(&m, [0.0; 3], [0.0, 0.0, -100.0], [0.0, 0.0, 1.0]);
        assert_eq!(s2, 1.0);
    }
}
