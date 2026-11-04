//! Elastic constants and strength for printed parts.
//!
//! # Stiffness: transversely isotropic about the print direction
//!
//! FDM parts are isotropic within a layer (the raster alternates direction
//! from layer to layer, so on average there is no preferred in-plane
//! direction) and different across layers. The data sheet gives E_xy, E_z
//! and one Poisson's ratio ν. We complete the five independent constants as
//! follows (1, 2 = in the layer plane, 3 = `print_up`):
//!
//! * E₁ = E₂ = E_xy, E₃ = E_z, ν₁₂ = ν, G₁₂ = E_xy / (2(1 + ν)) (in-plane
//!   isotropy requires exactly this G₁₂).
//! * Major Poisson ratio across the layers = ν for the *stiffer* direction:
//!   if E_xy ≥ E_z then ν₁₃ = ν (load in-plane, contraction across) and
//!   ν₃₁ = ν·E_z/E_xy by symmetry; otherwise ν₃₁ = ν and ν₁₃ = ν·E_xy/E_z.
//!   This keeps the compliance positive definite for any ν < 0.5 and any
//!   modulus ratio; measured FDM Poisson ratios (Casavola 2016, Song 2017)
//!   are all ~0.3–0.4, consistent with this.
//! * Interlayer shear modulus by the Saint-Venant (Huber) estimate
//!   1/G₁₃ = 1/E₁ + 1/E₃ + 2ν₁₃/E₁, which lies between the in-plane and
//!   cross-layer "isotropic" values and reduces exactly to E/(2(1+ν)) when
//!   E_xy = E_z. (Measured FDM interlayer shear moduli are rarely published;
//!   this is the standard estimate when only E₁, E₃, ν are known.)
//!
//! With E_xy = E_z the matrix is exactly isotropic.
//!
//! # Strength: 3D Tsai-Hill in the print frame
//!
//! With σ in the print frame (1, 2 in-plane, 3 = up), X = uts_xy, Z = uts_z:
//!
//! ```text
//! FI = (σ11² + σ22² − σ11σ22)/X² + σ33²/Z² − (σ11 + σ22)σ33 / max(X,Z)²
//!      + σ12²/S12² + (σ23² + σ31²)/S13²
//! S12 = X/√3,   S13 = min(X, Z)/√3
//! ```
//!
//! This is Tsai's form of Hill's criterion with the interaction term taken
//! from the dominant strength (as in Ansys ACP's "Tsai-Hill 3D"), written for
//! a material isotropic in the 1-2 plane. Properties:
//! * uniaxial along a layer fails at X, across layers at Z, equibiaxial
//!   in-plane at X;
//! * invariant under rotation about the print axis (the in-plane part is the
//!   2D von Mises form, which fixes S12 = X/√3);
//! * reduces to (σ_vonMises / UTS)² for isotropic materials, so the safety
//!   factor is then exactly UTS / σ_vM;
//! * convex (positive semi-definite) for every X/Z ratio. Hill's original
//!   pressure-independent form is *not* convex when Z < X/2, which is common
//!   for FDM layer adhesion, so it cannot be used here.
//!
//! Interlayer shear strength S13 = min(X,Z)/√3: the layer bond is treated as
//! a von Mises material of strength Z. This is conservative relative to the
//! few published FDM interlaminar shear tests (≈0.6–1.0 × Z).
//!
//! Compression uses the tensile strengths (the data sheets give no
//! compressive values; printed parts are stronger in compression, so this is
//! conservative).
//!
//! Failure index FI is quadratic in stress, so the safety factor is
//! 1/√FI and the load factor to first failure equals the safety factor.

use anyhow::{Result, bail};
use simlab_core::{Material, Vec3, cross, dot, normalize};

/// Linear elastic constants in the print frame (SI), and the frame itself.
#[derive(Clone, Debug)]
pub struct Elastic {
    pub e_xy: f64,
    pub e_z: f64,
    pub nu_xy: f64,
    /// Poisson ratio for in-plane load, contraction across layers (ν₁₃)
    pub nu_xz: f64,
    pub g_xy: f64,
    pub g_xz: f64,
    /// kg/m³
    pub density: f64,
    /// strength along the layers (Pa)
    pub x_strength: f64,
    /// strength across the layers (Pa)
    pub z_strength: f64,
    /// print frame: rows are local axes e1, e2, e3 (e3 = print_up), global coords
    pub frame: [Vec3; 3],
    /// 6×6 stiffness in global axes, Voigt order xx yy zz xy yz zx, engineering shear
    pub d: [[f64; 6]; 6],
}

/// Voigt index pairs
const VOIGT: [(usize, usize); 6] = [(0, 0), (1, 1), (2, 2), (0, 1), (1, 2), (2, 0)];

impl Elastic {
    /// From a SimLab material (MPa, kg/m³) and the print direction in model axes.
    pub fn from_material(m: &Material, print_up: Vec3) -> Result<Elastic> {
        Self::new(m.e_xy_mpa * 1e6, m.e_z_mpa * 1e6, m.nu, m.density, m.uts_xy_mpa * 1e6, m.uts_z_mpa * 1e6, print_up)
    }

    pub fn isotropic(e: f64, nu: f64, density: f64, strength: f64) -> Elastic {
        Self::new(e, e, nu, density, strength, strength, [0.0, 0.0, 1.0]).expect("valid isotropic material")
    }

    /// All values SI. See the module docs for how the missing constants are chosen.
    pub fn new(e_xy: f64, e_z: f64, nu: f64, density: f64, x_strength: f64, z_strength: f64, print_up: Vec3) -> Result<Elastic> {
        if !(e_xy > 0.0 && e_z > 0.0) {
            bail!("the material's stiffness must be above zero");
        }
        if !(nu > -1.0 && nu < 0.5) {
            bail!("the material's Poisson's ratio must be between -1 and 0.5");
        }
        if !(density >= 0.0) {
            bail!("the material's density cannot be negative");
        }
        if !(x_strength > 0.0 && z_strength > 0.0) {
            bail!("the material's strength must be above zero");
        }
        let nu_xz = if e_xy >= e_z { nu } else { nu * e_xy / e_z };
        let g_xy = e_xy / (2.0 * (1.0 + nu));
        let g_xz = 1.0 / (1.0 / e_xy + 1.0 / e_z + 2.0 * nu_xz / e_xy);
        let frame = print_frame(print_up)?;
        let mut el = Elastic { e_xy, e_z, nu_xy: nu, nu_xz, g_xy, g_xz, density, x_strength, z_strength, frame, d: [[0.0; 6]; 6] };
        el.d = rotate_stiffness(&el.local_stiffness(), &frame);
        Ok(el)
    }

    pub fn is_isotropic(&self) -> bool {
        (self.e_xy - self.e_z).abs() <= 1e-12 * self.e_xy
    }

    /// ν₃₁ (load across layers, contraction in-plane)
    pub fn nu_zx(&self) -> f64 {
        self.nu_xz * self.e_z / self.e_xy
    }

    /// 6×6 stiffness in the print frame (Voigt 11 22 33 12 23 31).
    pub fn local_stiffness(&self) -> [[f64; 6]; 6] {
        // normal block compliance
        let (e1, e3, n12, n13) = (self.e_xy, self.e_z, self.nu_xy, self.nu_xz);
        let s = [[1.0 / e1, -n12 / e1, -n13 / e1], [-n12 / e1, 1.0 / e1, -n13 / e1], [-n13 / e1, -n13 / e1, 1.0 / e3]];
        let c = inv3(&s);
        let mut d = [[0.0; 6]; 6];
        for i in 0..3 {
            for j in 0..3 {
                d[i][j] = c[i][j];
            }
        }
        d[3][3] = self.g_xy;
        d[4][4] = self.g_xz;
        d[5][5] = self.g_xz;
        d
    }

    /// Stress tensor (global Voigt) rotated into the print frame (Voigt 11 22 33 12 23 31).
    pub fn to_print_frame(&self, s: &[f64; 6]) -> [f64; 6] {
        let t = voigt_to_tensor(s);
        let q = &self.frame;
        let mut out = [0.0; 6];
        for (v, &(p, r)) in VOIGT.iter().enumerate() {
            let mut acc = 0.0;
            for i in 0..3 {
                for j in 0..3 {
                    acc += q[p][i] * t[i][j] * q[r][j];
                }
            }
            out[v] = acc;
        }
        out
    }

    /// Tsai-Hill failure index (≥ 1 means the strength is reached). See module docs.
    pub fn failure_index(&self, s_global: &[f64; 6]) -> f64 {
        let s = self.to_print_frame(s_global);
        let (x, z) = (self.x_strength, self.z_strength);
        let (ix2, iz2) = (1.0 / (x * x), 1.0 / (z * z));
        let im2 = 1.0 / (x.max(z) * x.max(z));
        let s13 = x.min(z) / 3f64.sqrt();
        let i13 = 1.0 / (s13 * s13);
        (s[0] * s[0] + s[1] * s[1] - s[0] * s[1]) * ix2 + s[2] * s[2] * iz2 - (s[0] + s[1]) * s[2] * im2
            + 3.0 * s[3] * s[3] * ix2
            + (s[4] * s[4] + s[5] * s[5]) * i13
    }
}

/// Orthonormal print frame with e3 = print_up (e1 is arbitrary in the layer
/// plane, which is fine for a transversely isotropic material).
pub fn print_frame(up: Vec3) -> Result<[Vec3; 3]> {
    if !(simlab_core::norm(up) > 0.0) || up.iter().any(|v| !v.is_finite()) {
        bail!("the print direction is not set");
    }
    let e3 = normalize(up);
    // pick the global axis least aligned with e3
    let mut k = 0;
    for i in 1..3 {
        if e3[i].abs() < e3[k].abs() {
            k = i;
        }
    }
    let mut a = [0.0; 3];
    a[k] = 1.0;
    let e1 = normalize(simlab_core::sub(a, simlab_core::scale(e3, dot(a, e3))));
    let e2 = cross(e3, e1);
    Ok([e1, e2, e3])
}

pub fn voigt_to_tensor(s: &[f64; 6]) -> [[f64; 3]; 3] {
    [[s[0], s[3], s[5]], [s[3], s[1], s[4]], [s[5], s[4], s[2]]]
}

/// Rotates a Voigt stiffness (engineering shear strain) from the local frame
/// (rows of `q` are local axes in global coordinates) to global axes.
pub fn rotate_stiffness(dl: &[[f64; 6]; 6], q: &[Vec3; 3]) -> [[f64; 6]; 6] {
    // C_ijkl(global) = Q_pi Q_qj Q_rk Q_sl C'_pqrs with Q_pi = q[p][i]
    let tensor = |c: &[[f64; 6]; 6], i: usize, j: usize, k: usize, l: usize| -> f64 {
        let v = |a: usize, b: usize| -> usize {
            match (a.min(b), a.max(b)) {
                (0, 0) => 0,
                (1, 1) => 1,
                (2, 2) => 2,
                (0, 1) => 3,
                (1, 2) => 4,
                _ => 5,
            }
        };
        c[v(i, j)][v(k, l)]
    };
    let mut out = [[0.0; 6]; 6];
    for (a, &(i, j)) in VOIGT.iter().enumerate() {
        for (b, &(k, l)) in VOIGT.iter().enumerate() {
            let mut acc = 0.0;
            for p in 0..3 {
                for qq in 0..3 {
                    let f1 = q[p][i] * q[qq][j];
                    if f1 == 0.0 {
                        continue;
                    }
                    for r in 0..3 {
                        for s in 0..3 {
                            acc += f1 * q[r][k] * q[s][l] * tensor(dl, p, qq, r, s);
                        }
                    }
                }
            }
            out[a][b] = acc;
        }
    }
    out
}

pub fn inv3(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let id = 1.0 / det;
    [
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * id, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * id, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * id],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * id, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * id, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * id],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * id, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * id, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * id],
    ]
}

/// von Mises equivalent stress.
pub fn von_mises(s: &[f64; 6]) -> f64 {
    let (a, b, c) = (s[0] - s[1], s[1] - s[2], s[2] - s[0]);
    (0.5 * (a * a + b * b + c * c) + 3.0 * (s[3] * s[3] + s[4] * s[4] + s[5] * s[5])).sqrt()
}

/// Principal stresses, largest first (closed-form for symmetric 3×3).
pub fn principal(s: &[f64; 6]) -> [f64; 3] {
    let [xx, yy, zz, xy, yz, zx] = *s;
    let p1 = xy * xy + yz * yz + zx * zx;
    let q = (xx + yy + zz) / 3.0;
    if p1 <= 1e-30 * (xx * xx + yy * yy + zz * zz + 1e-300) {
        let mut v = [xx, yy, zz];
        v.sort_by(|a, b| b.partial_cmp(a).unwrap());
        return v;
    }
    let p2 = (xx - q).powi(2) + (yy - q).powi(2) + (zz - q).powi(2) + 2.0 * p1;
    let p = (p2 / 6.0).sqrt();
    let b = [[(xx - q) / p, xy / p, zx / p], [xy / p, (yy - q) / p, yz / p], [zx / p, yz / p, (zz - q) / p]];
    let detb = b[0][0] * (b[1][1] * b[2][2] - b[1][2] * b[2][1]) - b[0][1] * (b[1][0] * b[2][2] - b[1][2] * b[2][0])
        + b[0][2] * (b[1][0] * b[2][1] - b[1][1] * b[2][0]);
    let r = (detb / 2.0).clamp(-1.0, 1.0);
    let phi = r.acos() / 3.0;
    let e1 = q + 2.0 * p * phi.cos();
    let e3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();
    let e2 = 3.0 * q - e1 - e3;
    [e1, e2, e3]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isotropic_matches_lame() {
        let el = Elastic::isotropic(200e9, 0.3, 7800.0, 400e6);
        let (e, nu) = (200e9, 0.3);
        let lam = e * nu / ((1.0 + nu) * (1.0 - 2.0 * nu));
        let mu = e / (2.0 * (1.0 + nu));
        for i in 0..3 {
            for j in 0..3 {
                let want = lam + if i == j { 2.0 * mu } else { 0.0 };
                assert!((el.d[i][j] - want).abs() < 1e-6 * e);
            }
            assert!((el.d[3 + i][3 + i] - mu).abs() < 1e-6 * e);
        }
        // isotropic: any print direction gives the same matrix
        let el2 = Elastic::new(e, e, nu, 1.0, 1.0, 1.0, [1.0, 2.0, -0.5]).unwrap();
        for i in 0..6 {
            for j in 0..6 {
                assert!((el.d[i][j] - el2.d[i][j]).abs() < 1e-6 * e);
            }
        }
        // Tsai-Hill reduces to von Mises
        let s = [10e6, -3e6, 4e6, 2e6, -1e6, 5e6];
        let fi = el.failure_index(&s);
        assert!((fi - (von_mises(&s) / 400e6).powi(2)).abs() < 1e-12);
    }

    #[test]
    fn transversely_isotropic_uniaxial() {
        // print_up along x: a uniaxial stress along x is across the layers
        let el = Elastic::new(3.5e9, 2.0e9, 0.35, 1240.0, 50e6, 20e6, [1.0, 0.0, 0.0]).unwrap();
        // compliance check: strain under σxx = 1 is 1/E_z along x
        let mut s = [[0.0; 6]; 6];
        s.copy_from_slice(&el.d);
        let eps = solve6(&el.d, &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((eps[0] * 2.0e9 - 1.0).abs() < 1e-9);
        assert!((el.failure_index(&[20e6, 0., 0., 0., 0., 0.]) - 1.0).abs() < 1e-12);
        assert!((el.failure_index(&[0., 50e6, 0., 0., 0., 0.]) - 1.0).abs() < 1e-12);
        assert!((el.failure_index(&[0., 50e6, 50e6, 0., 0., 0.]) - 1.0).abs() < 1e-12);
        let p = principal(&[1.0, 2.0, 3.0, 0.5, 0.2, -0.3]);
        let tr: f64 = p.iter().sum();
        assert!((tr - 6.0).abs() < 1e-12 && p[0] >= p[1] && p[1] >= p[2]);
    }

    fn solve6(a: &[[f64; 6]; 6], b: &[f64; 6]) -> [f64; 6] {
        let mut m = *a;
        let mut x = *b;
        for k in 0..6 {
            let p = (k..6).max_by(|&i, &j| m[i][k].abs().partial_cmp(&m[j][k].abs()).unwrap()).unwrap();
            m.swap(k, p);
            x.swap(k, p);
            for i in k + 1..6 {
                let f = m[i][k] / m[k][k];
                for j in k..6 {
                    m[i][j] -= f * m[k][j];
                }
                x[i] -= f * x[k];
            }
        }
        for k in (0..6).rev() {
            let mut s = x[k];
            for j in k + 1..6 {
                s -= m[k][j] * x[j];
            }
            x[k] = s / m[k][k];
        }
        x
    }
}
