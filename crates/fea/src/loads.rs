//! Consistent nodal loads: surface tractions on tri6 faces and body forces
//! on tet10 elements, plus mass properties.

use crate::elem::{tet_rule_14, tet10_gradients, tet10_point, tet10_shape, tri_rule_7, tri6_point, tri6_shape};
use crate::topo::{BFace, face_coords, tet_coords};
use rayon::prelude::*;
use simlab_core::{TetMesh, Vec3, norm, scale};

/// Adds ∫ Nᵢ t(x, n̂) dA over the given faces to `f` (length 3·nodes).
/// `t` receives the point and the outward unit normal and returns the
/// traction in Pa.
pub fn add_traction<F>(mesh: &TetMesh, faces: &[BFace], which: &[usize], t: F, f: &mut [f64])
where
    F: Fn(Vec3, Vec3) -> Vec3 + Sync,
{
    let parts: Vec<(usize, [Vec3; 6])> = which
        .par_iter()
        .map(|&i| {
            let fc = &faces[i];
            let x = face_coords(mesh, fc);
            let mut fe = [[0.0; 3]; 6];
            for qp in tri_rule_7() {
                let (p, na) = tri6_point(&x, &qp.l);
                let da = norm(na);
                if da == 0.0 {
                    continue;
                }
                let tr = t(p, scale(na, 1.0 / da));
                let n = tri6_shape(&qp.l);
                for a in 0..6 {
                    for k in 0..3 {
                        fe[a][k] += qp.w * da * n[a] * tr[k];
                    }
                }
            }
            (i, fe)
        })
        .collect();
    for (i, fe) in parts {
        for a in 0..6 {
            let v = faces[i].nodes[a] as usize;
            for k in 0..3 {
                f[3 * v + k] += fe[a][k];
            }
        }
    }
}

/// Area and area-weighted centroid of a set of faces.
pub fn faces_area(mesh: &TetMesh, faces: &[BFace], which: &[usize]) -> (f64, Vec3) {
    let mut a = 0.0;
    let mut c = [0.0; 3];
    for &i in which {
        let x = face_coords(mesh, &faces[i]);
        for qp in tri_rule_7() {
            let (p, na) = tri6_point(&x, &qp.l);
            let da = norm(na) * qp.w;
            a += da;
            for k in 0..3 {
                c[k] += p[k] * da;
            }
        }
    }
    if a > 0.0 {
        c = scale(c, 1.0 / a);
    }
    (a, c)
}

/// ∫ g(x) dA over faces (for torque distributions): returns Σ ∫ g.
pub fn faces_integral<G: Fn(Vec3) -> f64>(mesh: &TetMesh, faces: &[BFace], which: &[usize], g: G) -> f64 {
    let mut s = 0.0;
    for &i in which {
        let x = face_coords(mesh, &faces[i]);
        for qp in tri_rule_7() {
            let (p, na) = tri6_point(&x, &qp.l);
            s += g(p) * norm(na) * qp.w;
        }
    }
    s
}

/// Adds ∫ Nᵢ ρ b(x) dV to `f`, where `b` is a force per unit mass (m/s²).
pub fn add_body_force<F>(mesh: &TetMesh, density: f64, b: F, f: &mut [f64])
where
    F: Fn(Vec3) -> Vec3 + Sync,
{
    let rule = tet_rule_14();
    let parts: Vec<[Vec3; 10]> = (0..mesh.tets.len())
        .into_par_iter()
        .map(|e| {
            let x = tet_coords(mesh, e);
            let mut fe = [[0.0; 3]; 10];
            for qp in &rule {
                let (_, det) = tet10_gradients(&x, &qp.l);
                let p = tet10_point(&x, &qp.l);
                let bv = b(p);
                let n = tet10_shape(&qp.l);
                let w = qp.w * det * density;
                for a in 0..10 {
                    for k in 0..3 {
                        fe[a][k] += w * n[a] * bv[k];
                    }
                }
            }
            fe
        })
        .collect();
    for (e, fe) in parts.iter().enumerate() {
        let t = &mesh.tets[e];
        for a in 0..10 {
            for k in 0..3 {
                f[3 * t[a] as usize + k] += fe[a][k];
            }
        }
    }
}

/// Mass, first moment ∫ρx and second moment ∫ρ x xᵀ of the mesh.
#[derive(Clone, Copy, Debug, Default)]
pub struct MassProps {
    pub volume: f64,
    pub mass: f64,
    pub first: Vec3,
    pub second: [[f64; 3]; 3],
}

impl MassProps {
    pub fn center(&self) -> Vec3 {
        if self.mass > 0.0 { scale(self.first, 1.0 / self.mass) } else { [0.0; 3] }
    }
}

pub fn mass_props(mesh: &TetMesh, density: f64) -> MassProps {
    let rule = tet_rule_14();
    (0..mesh.tets.len())
        .into_par_iter()
        .map(|e| {
            let x = tet_coords(mesh, e);
            let mut m = MassProps::default();
            for qp in &rule {
                let (_, det) = tet10_gradients(&x, &qp.l);
                let p = tet10_point(&x, &qp.l);
                let w = qp.w * det;
                m.volume += w;
                m.mass += w * density;
                for i in 0..3 {
                    m.first[i] += w * density * p[i];
                    for j in 0..3 {
                        m.second[i][j] += w * density * p[i] * p[j];
                    }
                }
            }
            m
        })
        .reduce(MassProps::default, |mut a, b| {
            a.volume += b.volume;
            a.mass += b.mass;
            for i in 0..3 {
                a.first[i] += b.first[i];
                for j in 0..3 {
                    a.second[i][j] += b.second[i][j];
                }
            }
            a
        })
}
