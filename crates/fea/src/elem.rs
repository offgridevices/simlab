//! Quadratic tetrahedron (tet10) and quadratic triangle (tri6): shape
//! functions, quadrature rules and the element stiffness matrix.
//!
//! Node order is CalculiX/Abaqus C3D10 (see `simlab_core::mesh`): corners
//! 0..3, then edge midpoints 4=(0,1) 5=(1,2) 6=(2,0) 7=(0,3) 8=(1,3) 9=(2,3).
//! Reference coordinates are volume (barycentric) coordinates `l[0..4]`,
//! with ξ = l[1], η = l[2], ζ = l[3] and l[0] = 1 − ξ − η − ζ.
//!
//! Elements are isoparametric, so curved (CAD-snapped) midside nodes are
//! handled exactly by the geometry map; quadrature is exact only for
//! straight-sided elements (constant Jacobian).

use simlab_core::Vec3;

/// The six edges of a tet10 as corner pairs, in midside-node order (node 4 + i).
pub const TET_EDGES: [(usize, usize); 6] = [(0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3)];

/// A quadrature point on the reference tetrahedron (volume 1/6).
#[derive(Clone, Copy, Debug)]
pub struct TetPoint {
    /// barycentric coordinates
    pub l: [f64; 4],
    /// weight (weights sum to 1/6)
    pub w: f64,
}

/// A quadrature point on the reference triangle (area 1/2).
#[derive(Clone, Copy, Debug)]
pub struct TriPoint {
    pub l: [f64; 3],
    pub w: f64,
}

/// 4-point rule, exact for polynomials of degree 2: the stiffness of a
/// straight-sided tet10 (B is linear, so BᵀDB is quadratic). Same points as
/// CalculiX's C3D10, which makes Gauss-point output directly comparable.
pub fn tet_rule_4() -> [TetPoint; 4] {
    let a = 0.585_410_196_624_968_5;
    let b = 0.138_196_601_125_010_5;
    let w = 1.0 / 24.0;
    [
        TetPoint { l: [a, b, b, b], w },
        TetPoint { l: [b, a, b, b], w },
        TetPoint { l: [b, b, a, b], w },
        TetPoint { l: [b, b, b, a], w },
    ]
}

/// 14-point rule, exact for degree 5 (Walkington 2000): used for loads and
/// mass properties. NᵢNⱼ (mass) is degree 4; Nᵢ·(linear body force such as
/// centrifugal ρω²r or inertia relief) is degree 3; second moments of
/// inertia over a curved element are approximated well.
pub fn tet_rule_14() -> Vec<TetPoint> {
    let mut v = Vec::with_capacity(14);
    for (a, w) in [(0.092_735_250_310_891_2, 0.012_248_840_519_393_66), (0.310_885_919_263_300_6, 0.018_781_320_953_002_64)] {
        let c = 1.0 - 3.0 * a;
        for k in 0..4 {
            let mut l = [a; 4];
            l[k] = c;
            v.push(TetPoint { l, w });
        }
    }
    let b = 0.045_503_704_125_649_6;
    let c = 0.5 - b;
    let w = 0.007_091_003_462_846_911;
    for (i, j) in [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
        let mut l = [c; 4];
        l[i] = b;
        l[j] = b;
        v.push(TetPoint { l, w });
    }
    v
}

/// 7-point triangle rule, exact for degree 5 (Dunavant): tri6 face loads
/// (pressure on a curved face is degree ≤ 4 in the reference coordinates).
pub fn tri_rule_7() -> [TriPoint; 7] {
    let w0 = 0.225 / 2.0;
    let (a1, b1, w1) = (0.059_715_871_789_770, 0.470_142_064_105_115, 0.132_394_152_788_506 / 2.0);
    let (a2, b2, w2) = (0.797_426_985_353_087, 0.101_286_507_323_456, 0.125_939_180_544_827 / 2.0);
    [
        TriPoint { l: [1.0 / 3.0; 3], w: w0 },
        TriPoint { l: [a1, b1, b1], w: w1 },
        TriPoint { l: [b1, a1, b1], w: w1 },
        TriPoint { l: [b1, b1, a1], w: w1 },
        TriPoint { l: [a2, b2, b2], w: w2 },
        TriPoint { l: [b2, a2, b2], w: w2 },
        TriPoint { l: [b2, b2, a2], w: w2 },
    ]
}

/// tet10 shape functions at barycentric point `l`.
#[inline]
pub fn tet10_shape(l: &[f64; 4]) -> [f64; 10] {
    let mut n = [0.0; 10];
    for i in 0..4 {
        n[i] = l[i] * (2.0 * l[i] - 1.0);
    }
    for (e, &(a, b)) in TET_EDGES.iter().enumerate() {
        n[4 + e] = 4.0 * l[a] * l[b];
    }
    n
}

/// d(barycentric l_i)/d(ξ, η, ζ)
const DL: [[f64; 3]; 4] = [[-1.0, -1.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// tet10 shape function derivatives with respect to (ξ, η, ζ).
#[inline]
pub fn tet10_dshape(l: &[f64; 4]) -> [[f64; 3]; 10] {
    let mut d = [[0.0; 3]; 10];
    for i in 0..4 {
        let f = 4.0 * l[i] - 1.0;
        for k in 0..3 {
            d[i][k] = f * DL[i][k];
        }
    }
    for (e, &(a, b)) in TET_EDGES.iter().enumerate() {
        for k in 0..3 {
            d[4 + e][k] = 4.0 * (l[a] * DL[b][k] + l[b] * DL[a][k]);
        }
    }
    d
}

/// Position at a reference point.
#[inline]
pub fn tet10_point(x: &[Vec3; 10], l: &[f64; 4]) -> Vec3 {
    let n = tet10_shape(l);
    let mut p = [0.0; 3];
    for i in 0..10 {
        for k in 0..3 {
            p[k] += n[i] * x[i][k];
        }
    }
    p
}

#[inline]
fn det3(j: &[[f64; 3]; 3]) -> f64 {
    j[0][0] * (j[1][1] * j[2][2] - j[1][2] * j[2][1]) - j[0][1] * (j[1][0] * j[2][2] - j[1][2] * j[2][0])
        + j[0][2] * (j[1][0] * j[2][1] - j[1][1] * j[2][0])
}

/// Cartesian shape-function gradients dN/dx at `l`, and det(J).
/// `det` ≤ 0 means an inverted element.
#[inline]
pub fn tet10_gradients(x: &[Vec3; 10], l: &[f64; 4]) -> ([[f64; 3]; 10], f64) {
    let dn = tet10_dshape(l);
    // J[r][c] = dx_r / dξ_c
    let mut j = [[0.0; 3]; 3];
    for i in 0..10 {
        for r in 0..3 {
            for c in 0..3 {
                j[r][c] += x[i][r] * dn[i][c];
            }
        }
    }
    let det = det3(&j);
    let inv_det = 1.0 / det;
    // inverse of J (row r of J⁻¹ = dξ_r/dx)
    let ji = [
        [
            (j[1][1] * j[2][2] - j[1][2] * j[2][1]) * inv_det,
            (j[0][2] * j[2][1] - j[0][1] * j[2][2]) * inv_det,
            (j[0][1] * j[1][2] - j[0][2] * j[1][1]) * inv_det,
        ],
        [
            (j[1][2] * j[2][0] - j[1][0] * j[2][2]) * inv_det,
            (j[0][0] * j[2][2] - j[0][2] * j[2][0]) * inv_det,
            (j[0][2] * j[1][0] - j[0][0] * j[1][2]) * inv_det,
        ],
        [
            (j[1][0] * j[2][1] - j[1][1] * j[2][0]) * inv_det,
            (j[0][1] * j[2][0] - j[0][0] * j[2][1]) * inv_det,
            (j[0][0] * j[1][1] - j[0][1] * j[1][0]) * inv_det,
        ],
    ];
    // dN/dx_k = Σ_c dN/dξ_c · dξ_c/dx_k
    let mut g = [[0.0; 3]; 10];
    for i in 0..10 {
        for k in 0..3 {
            g[i][k] = dn[i][0] * ji[0][k] + dn[i][1] * ji[1][k] + dn[i][2] * ji[2][k];
        }
    }
    (g, det)
}

/// Strain (Voigt, engineering shear: xx yy zz xy yz zx) from gradients and
/// nodal displacements.
#[inline]
pub fn strain(g: &[[f64; 3]; 10], u: &[Vec3; 10]) -> [f64; 6] {
    let mut h = [[0.0; 3]; 3]; // h[i][k] = du_i/dx_k
    for a in 0..10 {
        for i in 0..3 {
            for k in 0..3 {
                h[i][k] += u[a][i] * g[a][k];
            }
        }
    }
    [h[0][0], h[1][1], h[2][2], h[0][1] + h[1][0], h[1][2] + h[2][1], h[2][0] + h[0][2]]
}

#[inline]
pub fn mat6_vec(d: &[[f64; 6]; 6], e: &[f64; 6]) -> [f64; 6] {
    let mut s = [0.0; 6];
    for i in 0..6 {
        for j in 0..6 {
            s[i] += d[i][j] * e[j];
        }
    }
    s
}

/// 30×30 element stiffness (row-major, dof = 3·node + component), integrated
/// with the 4-point rule. Returns the element volume (Σ w·detJ); a
/// non-positive value means an inverted element.
pub fn tet10_stiffness(x: &[Vec3; 10], d: &[[f64; 6]; 6], k: &mut [f64; 900]) -> f64 {
    k.fill(0.0);
    let mut vol = 0.0;
    let mut min_det = f64::INFINITY;
    for qp in tet_rule_4() {
        let (g, det) = tet10_gradients(x, &qp.l);
        min_det = min_det.min(det);
        let w = qp.w * det;
        vol += w;
        // DB: 6×30, column 3a+i = D · B_(a,i)
        let mut db = [[0.0f64; 30]; 6];
        for a in 0..10 {
            let [gx, gy, gz] = g[a];
            for r in 0..6 {
                let dr = &d[r];
                // B columns for node a: ux -> [gx,0,0,gy,0,gz], uy -> [0,gy,0,gx,gz,0], uz -> [0,0,gz,0,gy,gx]
                db[r][3 * a] = (dr[0] * gx + dr[3] * gy + dr[5] * gz) * w;
                db[r][3 * a + 1] = (dr[1] * gy + dr[3] * gx + dr[4] * gz) * w;
                db[r][3 * a + 2] = (dr[2] * gz + dr[4] * gy + dr[5] * gx) * w;
            }
        }
        // K += Bᵀ (DB): row 3a+i
        for a in 0..10 {
            let [gx, gy, gz] = g[a];
            for col in 0..30 {
                let r0 = gx * db[0][col] + gy * db[3][col] + gz * db[5][col];
                let r1 = gy * db[1][col] + gx * db[3][col] + gz * db[4][col];
                let r2 = gz * db[2][col] + gy * db[4][col] + gx * db[5][col];
                k[(3 * a) * 30 + col] += r0;
                k[(3 * a + 1) * 30 + col] += r1;
                k[(3 * a + 2) * 30 + col] += r2;
            }
        }
    }
    if min_det <= 0.0 { -vol.abs() } else { vol }
}

/// tri6 shape functions at barycentric `l` (corners a b c, then ab bc ca).
#[inline]
pub fn tri6_shape(l: &[f64; 3]) -> [f64; 6] {
    [
        l[0] * (2.0 * l[0] - 1.0),
        l[1] * (2.0 * l[1] - 1.0),
        l[2] * (2.0 * l[2] - 1.0),
        4.0 * l[0] * l[1],
        4.0 * l[1] * l[2],
        4.0 * l[2] * l[0],
    ]
}

/// Position, and the area-scaled normal (∂x/∂r × ∂x/∂s, length = dA/(dr ds))
/// of a tri6 face at `l` (r = l[1], s = l[2]).
#[inline]
pub fn tri6_point(x: &[Vec3; 6], l: &[f64; 3]) -> (Vec3, Vec3) {
    let n = tri6_shape(l);
    // derivatives wrt r and s, with l0 = 1 - r - s
    let dr = [-(4.0 * l[0] - 1.0), 4.0 * l[1] - 1.0, 0.0, 4.0 * (l[0] - l[1]), 4.0 * l[2], -4.0 * l[2]];
    let ds = [-(4.0 * l[0] - 1.0), 0.0, 4.0 * l[2] - 1.0, -4.0 * l[1], 4.0 * l[1], 4.0 * (l[0] - l[2])];
    let mut p = [0.0; 3];
    let mut tr = [0.0; 3];
    let mut ts = [0.0; 3];
    for i in 0..6 {
        for k in 0..3 {
            p[k] += n[i] * x[i][k];
            tr[k] += dr[i] * x[i][k];
            ts[k] += ds[i] * x[i][k];
        }
    }
    (p, simlab_core::cross(tr, ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(n: u32) -> f64 {
        (1..=n).map(|k| k as f64).product()
    }

    #[test]
    fn tet_rules_integrate_monomials() {
        // ∫ ξ^a η^b ζ^c over the reference tet = a! b! c! / (a+b+c+3)!
        for (rule, deg) in [(tet_rule_4().to_vec(), 2), (tet_rule_14(), 5)] {
            for a in 0..=deg {
                for b in 0..=deg - a {
                    for c in 0..=deg - a - b {
                        let exact = fact(a) * fact(b) * fact(c) / fact(a + b + c + 3);
                        let q: f64 = rule.iter().map(|p| p.w * p.l[1].powi(a as i32) * p.l[2].powi(b as i32) * p.l[3].powi(c as i32)).sum();
                        assert!((q - exact).abs() < 1e-14, "deg {deg} monomial {a}{b}{c}: {q} vs {exact}");
                    }
                }
            }
        }
    }

    #[test]
    fn tri_rule_integrates_monomials() {
        for a in 0..=5u32 {
            for b in 0..=5 - a {
                let exact = fact(a) * fact(b) / fact(a + b + 2);
                let q: f64 = tri_rule_7().iter().map(|p| p.w * p.l[1].powi(a as i32) * p.l[2].powi(b as i32)).sum();
                assert!((q - exact).abs() < 1e-14, "{a}{b}: {q} vs {exact}");
            }
        }
    }

    #[test]
    fn shape_functions_partition_unity_and_kronecker() {
        let nodes_l: [[f64; 4]; 10] = [
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
        for (i, l) in nodes_l.iter().enumerate() {
            let n = tet10_shape(l);
            for j in 0..10 {
                assert!((n[j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-15);
            }
        }
        let d = tet10_dshape(&[0.1, 0.2, 0.3, 0.4]);
        for k in 0..3 {
            assert!(d.iter().map(|r| r[k]).sum::<f64>().abs() < 1e-14);
        }
    }
}

/// Displacement gradient H = ∂u/∂X (H[i][k] = ∂u_i/∂X_k) from gradients and
/// nodal displacements.
#[inline]
pub fn displacement_gradient(g: &[[f64; 3]; 10], u: &[Vec3; 10]) -> [[f64; 3]; 3] {
    let mut h = [[0.0; 3]; 3];
    for a in 0..10 {
        for i in 0..3 {
            for k in 0..3 {
                h[i][k] += u[a][i] * g[a][k];
            }
        }
    }
    h
}

/// Green-Lagrange strain E = ½(H + Hᵀ + HᵀH) (Voigt, engineering shear).
#[inline]
pub fn green_strain(h: &[[f64; 3]; 3]) -> [f64; 6] {
    let c = |i: usize, j: usize| h[0][i] * h[0][j] + h[1][i] * h[1][j] + h[2][i] * h[2][j];
    [
        h[0][0] + 0.5 * c(0, 0),
        h[1][1] + 0.5 * c(1, 1),
        h[2][2] + 0.5 * c(2, 2),
        h[0][1] + h[1][0] + c(0, 1),
        h[1][2] + h[2][1] + c(1, 2),
        h[2][0] + h[0][2] + c(2, 0),
    ]
}

/// Cauchy (true) stress σ = F S Fᵀ / det F from the second Piola-Kirchhoff
/// stress S (Voigt) and the displacement gradient.
#[inline]
pub fn cauchy_stress(h: &[[f64; 3]; 3], s: &[f64; 6]) -> [f64; 6] {
    let f = [[1.0 + h[0][0], h[0][1], h[0][2]], [h[1][0], 1.0 + h[1][1], h[1][2]], [h[2][0], h[2][1], 1.0 + h[2][2]]];
    let st = [[s[0], s[3], s[5]], [s[3], s[1], s[4]], [s[5], s[4], s[2]]];
    let j = f[0][0] * (f[1][1] * f[2][2] - f[1][2] * f[2][1]) - f[0][1] * (f[1][0] * f[2][2] - f[1][2] * f[2][0]) + f[0][2] * (f[1][0] * f[2][1] - f[1][1] * f[2][0]);
    let mut fs = [[0.0; 3]; 3];
    for i in 0..3 {
        for b in 0..3 {
            fs[i][b] = f[i][0] * st[0][b] + f[i][1] * st[1][b] + f[i][2] * st[2][b];
        }
    }
    let c = |i: usize, k: usize| (fs[i][0] * f[k][0] + fs[i][1] * f[k][1] + fs[i][2] * f[k][2]) / j;
    [c(0, 0), c(1, 1), c(2, 2), c(0, 1), c(1, 2), c(2, 0)]
}

/// Nonlinear strain-displacement rows for node `a`: δE = B·δu_a, with
/// F = I + H. Row order xx yy zz xy yz zx, columns ux uy uz.
#[inline]
fn b_nl(g: &[f64; 3], h: &[[f64; 3]; 3]) -> [[f64; 3]; 6] {
    // F[i][k] = δ_ik + H[i][k]; δE_kl = ½(F_ik δH_il + F_il δH_ik), δH_il = δu_i g_l
    let f = |i: usize, k: usize| if i == k { 1.0 + h[i][k] } else { h[i][k] };
    let mut b = [[0.0; 3]; 6];
    for i in 0..3 {
        b[0][i] = f(i, 0) * g[0];
        b[1][i] = f(i, 1) * g[1];
        b[2][i] = f(i, 2) * g[2];
        b[3][i] = f(i, 0) * g[1] + f(i, 1) * g[0];
        b[4][i] = f(i, 1) * g[2] + f(i, 2) * g[1];
        b[5][i] = f(i, 2) * g[0] + f(i, 0) * g[2];
    }
    b
}

/// What goes into an element stiffness matrix.
#[derive(Clone, Copy)]
pub struct Stiffness<'a> {
    pub el: &'a crate::material::Elastic,
    /// geometrically nonlinear (total Lagrangian, St Venant-Kirchhoff, the
    /// formulation of CalculiX's `NLGEOM`): the tangent at this total
    /// displacement. Material part with the deformed B, plus the initial
    /// stress (stress-stiffening) part ∫ ∇Nₐᵀ S ∇N_b dV · I.
    pub large_u: Option<&'a [f64]>,
    /// (small-displacement element only) initial stress from this
    /// displacement's small-strain stress: K + ∫ ∇Nₐᵀ σ ∇N_b dV · I. The
    /// nonlinear solve's first guess.
    pub prestress_u: Option<&'a [f64]>,
    /// spin softening for each spin (ρω², unit axis): −∫ ρω² Nₐ N_b (I − a aᵀ) dV,
    /// because the centrifugal force grows as the part moves outward
    pub spin: &'a [(f64, Vec3)],
}

impl<'a> Stiffness<'a> {
    pub fn plain(el: &'a crate::material::Elastic) -> Self {
        Stiffness { el, large_u: None, prestress_u: None, spin: &[] }
    }

    pub fn is_plain(&self) -> bool {
        self.large_u.is_none() && self.prestress_u.is_none() && self.spin.is_empty()
    }

    fn element_u(&self, mesh: &simlab_core::TetMesh, e: usize, u: &[f64]) -> [Vec3; 10] {
        let t = &mesh.tets[e];
        std::array::from_fn(|i| {
            let v = 3 * t[i] as usize;
            [u[v], u[v + 1], u[v + 2]]
        })
    }

    /// Element matrix of element `e`; returns the element volume (≤ 0: inverted).
    pub fn element(&self, mesh: &simlab_core::TetMesh, e: usize, k: &mut [f64; 900]) -> f64 {
        let x = crate::topo::tet_coords(mesh, e);
        let vol = match (self.large_u, self.prestress_u) {
            (Some(u), _) => tet10_tangent(&x, &self.el.d, &self.element_u(mesh, e, u), k),
            (None, Some(p)) => {
                let v = tet10_stiffness(&x, &self.el.d, k);
                add_initial_stress(&x, &self.el.d, &self.element_u(mesh, e, p), k);
                v
            }
            (None, None) => tet10_stiffness(&x, &self.el.d, k),
        };
        if !self.spin.is_empty() {
            self.add_spin(&x, k);
        }
        vol
    }

    /// −∫ ρω² Nₐ N_b P dV (14-point rule, exact for straight elements)
    fn add_spin(&self, x: &[Vec3; 10], k: &mut [f64; 900]) {
        let mut p = [[0.0; 3]; 3];
        for (rw2, ax) in self.spin {
            for i in 0..3 {
                for j in 0..3 {
                    p[i][j] += rw2 * (if i == j { 1.0 } else { 0.0 } - ax[i] * ax[j]);
                }
            }
        }
        for q in tet_rule_14() {
            let (_, det) = tet10_gradients(x, &q.l);
            let w = q.w * det;
            let n = tet10_shape(&q.l);
            for a in 0..10 {
                for b in 0..10 {
                    let nn = w * n[a] * n[b];
                    for i in 0..3 {
                        for j in 0..3 {
                            k[(3 * a + i) * 30 + 3 * b + j] -= nn * p[i][j];
                        }
                    }
                }
            }
        }
    }

    /// Internal nodal forces of element `e` at displacement `u`, minus the
    /// part of the spin load that follows the displacement: K u for a linear
    /// element; ∫ Bᵀ S dV − ∫ ρω² N P u dV for a nonlinear one. Equilibrium
    /// is `element forces = applied loads` (the loads at the undeformed shape).
    pub fn element_force(&self, mesh: &simlab_core::TetMesh, e: usize, u: &[f64]) -> [f64; 30] {
        let ue = self.element_u(mesh, e, u);
        let uf: [f64; 30] = std::array::from_fn(|i| ue[i / 3][i % 3]);
        let mut fe = [0.0; 30];
        match self.large_u {
            None => {
                let mut ke = [0.0f64; 900];
                self.element(mesh, e, &mut ke);
                for i in 0..30 {
                    fe[i] = (0..30).map(|j| ke[i * 30 + j] * uf[j]).sum();
                }
            }
            Some(_) => {
                let x = crate::topo::tet_coords(mesh, e);
                tet10_internal_force(&x, &self.el.d, &ue, &mut fe);
                if !self.spin.is_empty() {
                    let mut ks = [0.0f64; 900];
                    self.add_spin(&x, &mut ks);
                    for i in 0..30 {
                        fe[i] += (0..30).map(|j| ks[i * 30 + j] * uf[j]).sum::<f64>();
                    }
                }
            }
        }
        fe
    }
}

/// Total Lagrangian tangent of a tet10 at displacement `u` (4-point rule,
/// as CalculiX's C3D10): ∫ B_NLᵀ D B_NL + ∇Nᵀ S ∇N · I dV. Returns the
/// element volume (≤ 0: inverted).
pub fn tet10_tangent(x: &[Vec3; 10], d: &[[f64; 6]; 6], u: &[Vec3; 10], k: &mut [f64; 900]) -> f64 {
    k.fill(0.0);
    let mut vol = 0.0;
    let mut min_det = f64::INFINITY;
    for qp in tet_rule_4() {
        let (g, det) = tet10_gradients(x, &qp.l);
        min_det = min_det.min(det);
        let w = qp.w * det;
        vol += w;
        let h = displacement_gradient(&g, u);
        let s = mat6_vec(d, &green_strain(&h));
        let b: [[[f64; 3]; 6]; 10] = std::array::from_fn(|a| b_nl(&g[a], &h));
        // DB for every node: 6 × 30
        let mut db = [[0.0f64; 30]; 6];
        for a in 0..10 {
            for i in 0..3 {
                for r in 0..6 {
                    db[r][3 * a + i] = w * (0..6).map(|c| d[r][c] * b[a][c][i]).sum::<f64>();
                }
            }
        }
        let sig = [[s[0], s[3], s[5]], [s[3], s[1], s[4]], [s[5], s[4], s[2]]];
        for a in 0..10 {
            let sa: [f64; 3] = std::array::from_fn(|l| (0..3).map(|m| g[a][m] * sig[m][l]).sum());
            for i in 0..3 {
                let row = 3 * a + i;
                for col in 0..30 {
                    k[row * 30 + col] += (0..6).map(|r| b[a][r][i] * db[r][col]).sum::<f64>();
                }
            }
            for bn in 0..10 {
                let v = w * (sa[0] * g[bn][0] + sa[1] * g[bn][1] + sa[2] * g[bn][2]);
                for i in 0..3 {
                    k[(3 * a + i) * 30 + 3 * bn + i] += v;
                }
            }
        }
    }
    if min_det <= 0.0 { -vol.abs() } else { vol }
}

/// Adds ∫ ∇Nₐᵀ σ ∇N_b dV · I with σ the small-strain stress of `u` (4-point rule).
fn add_initial_stress(x: &[Vec3; 10], d: &[[f64; 6]; 6], u: &[Vec3; 10], k: &mut [f64; 900]) {
    for qp in tet_rule_4() {
        let (g, det) = tet10_gradients(x, &qp.l);
        let w = qp.w * det;
        let s = mat6_vec(d, &strain(&g, u));
        let sig = [[s[0], s[3], s[5]], [s[3], s[1], s[4]], [s[5], s[4], s[2]]];
        for a in 0..10 {
            let sa: [f64; 3] = std::array::from_fn(|l| (0..3).map(|m| g[a][m] * sig[m][l]).sum());
            for b in 0..10 {
                let v = w * (sa[0] * g[b][0] + sa[1] * g[b][1] + sa[2] * g[b][2]);
                for i in 0..3 {
                    k[(3 * a + i) * 30 + 3 * b + i] += v;
                }
            }
        }
    }
}

/// Total Lagrangian internal force ∫ B_NLᵀ S dV of a tet10 (4-point rule).
pub fn tet10_internal_force(x: &[Vec3; 10], d: &[[f64; 6]; 6], u: &[Vec3; 10], f: &mut [f64; 30]) {
    f.fill(0.0);
    for qp in tet_rule_4() {
        let (g, det) = tet10_gradients(x, &qp.l);
        let w = qp.w * det;
        let h = displacement_gradient(&g, u);
        let s = mat6_vec(d, &green_strain(&h));
        for a in 0..10 {
            let b = b_nl(&g[a], &h);
            for i in 0..3 {
                f[3 * a + i] += w * (0..6).map(|r| b[r][i] * s[r]).sum::<f64>();
            }
        }
    }
}

#[cfg(test)]
mod nl_tests {
    use super::*;

    fn element() -> ([Vec3; 10], [Vec3; 10], [[f64; 6]; 6]) {
        let c = [[0.0, 0.0, 0.0], [1.0e-3, 0.1e-3, 0.0], [0.2e-3, 0.9e-3, 0.1e-3], [0.1e-3, 0.2e-3, 1.1e-3]];
        let x: [Vec3; 10] = std::array::from_fn(|i| if i < 4 { c[i] } else {
            let (a, b) = TET_EDGES[i - 4];
            std::array::from_fn(|k| 0.5 * (c[a][k] + c[b][k]))
        });
        let u: [Vec3; 10] = std::array::from_fn(|i| [1e-5 * (i as f64).sin(), 2e-5 * (i as f64 * 0.7).cos(), -1.5e-5 * (i as f64 * 1.3).sin()]);
        let el = crate::material::Elastic::new(2.1e9, 1.8e9, 0.38, 1250.0, 50e6, 23e6, [0.3, 0.1, 1.0]).unwrap();
        (x, u, el.d)
    }

    /// The tangent is the derivative of the internal force (central differences).
    #[test]
    fn tangent_is_derivative_of_internal_force() {
        let (x, u, d) = element();
        let mut k = [0.0; 900];
        tet10_tangent(&x, &d, &u, &mut k);
        let mut worst: f64 = 0.0;
        let kmax = k.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        for j in 0..30 {
            let eps = 1e-9;
            let mut up = u;
            let mut um = u;
            up[j / 3][j % 3] += eps;
            um[j / 3][j % 3] -= eps;
            let (mut fp, mut fm) = ([0.0; 30], [0.0; 30]);
            tet10_internal_force(&x, &d, &up, &mut fp);
            tet10_internal_force(&x, &d, &um, &mut fm);
            for i in 0..30 {
                worst = worst.max(((fp[i] - fm[i]) / (2.0 * eps) - k[i * 30 + j]).abs());
            }
        }
        assert!(worst < 1e-6 * kmax, "tangent off by {worst:e} (max entry {kmax:e})");
    }

    /// At zero displacement the tangent is the linear stiffness; a rigid
    /// rotation (even a large one) gives no internal force.
    #[test]
    fn small_and_rigid_limits() {
        let (x, _, d) = element();
        let (mut k0, mut kl) = ([0.0; 900], [0.0; 900]);
        tet10_tangent(&x, &d, &[[0.0; 3]; 10], &mut k0);
        tet10_stiffness(&x, &d, &mut kl);
        let kmax = kl.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(k0.iter().zip(&kl).all(|(a, b)| (a - b).abs() < 1e-12 * kmax));
        let th: f64 = 0.7;
        let r = [[th.cos(), -th.sin(), 0.0], [th.sin(), th.cos(), 0.0], [0.0, 0.0, 1.0]];
        let u: [Vec3; 10] = std::array::from_fn(|i| std::array::from_fn(|k| (0..3).map(|m| r[k][m] * x[i][m]).sum::<f64>() - x[i][k]));
        let mut f = [0.0; 30];
        tet10_internal_force(&x, &d, &u, &mut f);
        assert!(f.iter().all(|v| v.abs() < 1e-9 * kmax * 1e-3), "{f:?}");
    }
}
