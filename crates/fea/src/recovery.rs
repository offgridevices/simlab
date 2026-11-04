//! Stress recovery from the displacement solution.
//!
//! Nodal stresses follow CalculiX exactly: each element's stress at its 4
//! Gauss points is extrapolated linearly to the 4 corners, midside nodes
//! take the mean of their two corners, and every node averages the values
//! of all elements that share it. For a straight-sided tet10 the stress is
//! linear inside the element, so this is also the exact element stress at
//! the nodes.
//!
//! Gauss-point values (4 per element) and the element volume averages are
//! kept for the robust peak measures.

use crate::elem::{mat6_vec, strain, tet_rule_4, tet10_gradients};
use crate::material::Elastic;
use crate::topo::tet_coords;
use rayon::prelude::*;
use simlab_core::{TetMesh, Vec3};

/// Barycentric coordinates of the 10 tet10 nodes.
pub const NODE_BARY: [[f64; 4]; 10] = [
    [1., 0., 0., 0.],
    [0., 1., 0., 0.],
    [0., 0., 1., 0.],
    [0., 0., 0., 1.],
    [0.5, 0.5, 0., 0.],
    [0., 0.5, 0.5, 0.],
    [0.5, 0., 0.5, 0.],
    [0.5, 0., 0., 0.5],
    [0., 0.5, 0., 0.5],
    [0., 0., 0.5, 0.5],
];

pub struct Stresses {
    /// averaged nodal stress, global Voigt xx yy zz xy yz zx (Pa)
    pub nodal: Vec<[f64; 6]>,
    /// volume-averaged stress per element
    pub element_mean: Vec<[f64; 6]>,
    /// stress at the 4 Gauss points of each element
    pub gauss: Vec<[[f64; 6]; 4]>,
    /// Gauss-point volume weights (w·detJ)
    pub gauss_w: Vec<[f64; 4]>,
}

pub fn element_u(u: &[f64], t: &[u32; 10]) -> [Vec3; 10] {
    std::array::from_fn(|i| {
        let v = 3 * t[i] as usize;
        [u[v], u[v + 1], u[v + 2]]
    })
}

pub fn recover(mesh: &TetMesh, el: &Elastic, u: &[f64]) -> Stresses {
    let gp = tet_rule_4();
    type Per = ([[f64; 6]; 10], [[f64; 6]; 4], [f64; 4]);
    let per: Vec<Per> = (0..mesh.tets.len())
        .into_par_iter()
        .map(|e| {
            let x = tet_coords(mesh, e);
            let ue = element_u(u, &mesh.tets[e]);
            let mut gs = [[0.0; 6]; 4];
            let mut gw = [0.0; 4];
            for (k, q) in gp.iter().enumerate() {
                let (g, det) = tet10_gradients(&x, &q.l);
                gs[k] = mat6_vec(&el.d, &strain(&g, &ue));
                gw[k] = q.w * det;
            }
            // linear extrapolation: Gauss point k sits at l = b everywhere except
            // a at corner k, so corner_i = (σ_i − b Σσ) / (a − b)
            let (a, b) = (gp[0].l[0], gp[0].l[1]);
            let mut nodal = [[0.0; 6]; 10];
            for c in 0..6 {
                let sum: f64 = (0..4).map(|k| gs[k][c]).sum();
                for i in 0..4 {
                    nodal[i][c] = (gs[i][c] - b * sum) / (a - b);
                }
                for (e, &(p, q)) in crate::elem::TET_EDGES.iter().enumerate() {
                    nodal[4 + e][c] = 0.5 * (nodal[p][c] + nodal[q][c]);
                }
            }
            (nodal, gs, gw)
        })
        .collect();
    let n = mesh.nodes.len();
    let mut sum = vec![[0.0f64; 6]; n];
    let mut cnt = vec![0u32; n];
    let mut element_mean = Vec::with_capacity(per.len());
    let mut gauss = Vec::with_capacity(per.len());
    let mut gauss_w = Vec::with_capacity(per.len());
    for (e, (nodal, gs, gw)) in per.into_iter().enumerate() {
        let t = &mesh.tets[e];
        for a in 0..10 {
            let v = t[a] as usize;
            for k in 0..6 {
                sum[v][k] += nodal[a][k];
            }
            cnt[v] += 1;
        }
        let vol: f64 = gw.iter().sum();
        let mut m = [0.0; 6];
        for q in 0..4 {
            for k in 0..6 {
                m[k] += gs[q][k] * gw[q] / vol;
            }
        }
        element_mean.push(m);
        gauss.push(gs);
        gauss_w.push(gw);
    }
    for v in 0..n {
        if cnt[v] > 0 {
            for k in 0..6 {
                sum[v][k] /= cnt[v] as f64;
            }
        }
    }
    Stresses { nodal: sum, element_mean, gauss, gauss_w }
}

/// Volume-weighted percentile (0..1) of per-point values with weights.
pub fn weighted_percentile(vals: &mut [(f64, f64)], q: f64) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.par_sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = vals.iter().map(|v| v.1).sum();
    let target = q * total;
    let mut acc = 0.0;
    for v in vals.iter() {
        acc += v.1;
        if acc >= target {
            return v.0;
        }
    }
    vals.last().unwrap().0
}
