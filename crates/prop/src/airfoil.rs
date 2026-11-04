//! Lift and drag of one blade section at low Reynolds number (Re ~ 10k-500k),
//! the regime of small printed propellers.
//!
//! The model is a small set of smooth formulas whose constants come from
//! published wind-tunnel data, so it works from the only things we can
//! measure on a mesh: thickness ratio t/c and maximum camber f/c.
//!
//! * Thin sections (t/c <= 3 %) behave like cambered plates. Constants for
//!   their zero-lift angle, base drag, camber drag and the loss of
//!   leading-edge suction come from Balla & Vad, "Measurement-based models
//!   for estimation of lift and drag coefficients for low-Reynolds-number
//!   cambered plates", AIAA Journal 60(9), 2022, doi:10.2514/1.J061579
//!   (circular-arc plates, 3 % thick, h/c 0-8 %, Re 40k-140k, alpha 0-8 deg).
//!   Their measured drag at alpha > 0 is close to Cl*tan(alpha) + Cd0, i.e. the
//!   section force stays normal to the chord, which is what this model uses.
//! * Thick sections (t/c >= 8 %) behave like conventional low-Re airfoils:
//!   lift-curve slope and zero-lift angle fitted to UIUC wind-tunnel data for
//!   the Clark Y and SD7037 at Re 60k and 100k (Selig et al., "Summary of
//!   Low-Speed Airfoil Data", vols. 1-5, SoarTech/UIUC 1995-2012): slope
//!   4.8-6.1 /rad, zero-lift angle about 0.6x the thin-airfoil value, minimum
//!   drag falling as Re^-0.65 below Re 100k (laminar separation bubble).
//!   Thickness drag uses Hoerner's form factor (1 + 2 t/c + 60 (t/c)^4).
//! * Between 3 % and 8 % thickness the two are blended linearly.
//! * Schmitz, "Aerodynamik des Flugmodells" (1942; NASA/Redstone translation
//!   RSIC-721, 1967) and Mueller & DeLaurier (Annu. Rev. Fluid Mech. 35, 2003)
//!   show the same picture: below Re ~ 70k thin cambered plates keep their
//!   lift while thick airfoils lose it to laminar separation.
//! * Stall is soft (thin sections stall gradually), with Cl_max rising with
//!   camber and Reynolds number; past stall the Viterna-Corrigan extrapolation
//!   (Viterna & Corrigan, "Fixed pitch rotor performance of large horizontal
//!   axis wind turbines", NASA CP-2230, 1982) takes the curves smoothly to a
//!   flat plate at 90 deg, with Cd_max = 1.11 + 0.018 AR.
//! * Optional rotational augmentation near the hub (Snel, Houwink & Bosschers,
//!   "Sectional prediction of lift coefficients on rotating wind turbine blades
//!   in stall", ECN-C--93-052, 1994): Cl += 3 (c/r)^2 (Cl_linear - Cl).
//! * Compressibility: Prandtl-Glauert on the lift slope (tip Mach of FPV props
//!   reaches 0.4-0.6), capped at M = 0.7.

use std::f64::consts::{FRAC_PI_2, PI};

/// What the section looks like, as measured from the mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionShape {
    /// maximum thickness / chord
    pub thickness: f64,
    /// maximum camber / chord; positive bulges toward the thrust side
    pub camber: f64,
}

impl Default for SectionShape {
    /// a typical thin moulded small-prop section
    fn default() -> Self {
        SectionShape { thickness: 0.08, camber: 0.04 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coeffs {
    pub cl: f64,
    pub cd: f64,
}

/// Everything the polar needs besides the angle.
#[derive(Clone, Copy, Debug)]
pub struct SectionState {
    pub shape: SectionShape,
    pub reynolds: f64,
    pub mach: f64,
    /// blade aspect ratio (span / mean chord), for the post-stall drag
    pub aspect_ratio: f64,
    /// chord / radius, used only when rotational augmentation is on
    pub c_over_r: f64,
    pub rotational_augmentation: bool,
}

// ------------------------------------------------------------ constants

/// zero-lift angle = -2 * KAPPA * camber (thin-airfoil value is KAPPA = 1).
/// Balla & Vad's plates give Cl(alpha=0) = 2 pi eta * 2 KAPPA f with
/// eta*KAPPA = 0.55-0.64 for h/c 2-8 %; Clark Y / SD7037 at Re 60-100k give
/// KAPPA 0.55-0.65.
const KAPPA: f64 = 0.62;

/// 0 at Re 20k (and below), 1 at Re 100k (and above), linear in log(Re).
fn re_weight(re: f64) -> f64 {
    ((re.max(1.0).ln() - 20e3f64.ln()) / (100e3f64.ln() - 20e3f64.ln())).clamp(0.0, 1.0)
}

/// 1 for a plate (t/c <= 3 %), 0 for an airfoil (t/c >= 8 %).
pub fn thin_weight(t: f64) -> f64 {
    ((0.08 - t) / 0.05).clamp(0.0, 1.0)
}

/// Cl / Cl_max -> smooth saturation, linear for small x, 1 for large x.
fn soft_sat(x: f64) -> f64 {
    x / (1.0 + x.abs().powi(8)).powf(1.0 / 8.0)
}

/// where the soft lift curve hands over to Viterna, in units of Cl_max/slope
const STALL_X: f64 = 1.3;

/// The polar's derived parameters for one (shape, Re, M).
#[derive(Clone, Copy, Debug)]
pub struct PolarParams {
    pub slope: f64,
    pub alpha0: f64,
    pub cl_max: f64,
    pub cl_min: f64,
    pub cd_max: f64,
    w_thin: f64,
    cd_min: f64,
    k2: f64,
    cl_md: f64,
}

impl PolarParams {
    pub fn new(shape: SectionShape, re: f64, mach: f64, aspect_ratio: f64) -> Self {
        let t = shape.thickness.clamp(0.0, 0.4);
        let f = shape.camber.clamp(-0.15, 0.15);
        let s = re_weight(re);
        let w_thin = thin_weight(t);
        let re = re.clamp(5e3, 2e6);

        // lift slope: plates keep ~0.92-0.95 of 2 pi at these Re (UIUC flat
        // plate 7.1-7.4/rad near 0 deg, Balla's plates 5.7/rad secant to 8
        // deg); thick airfoils fall from 0.97 at 100k to ~0.8 at 20k.
        let eta_thin = 0.90 + 0.05 * s;
        let eta_thick = 0.80 + 0.17 * s;
        let eta = w_thin * eta_thin + (1.0 - w_thin) * eta_thick;
        let m = mach.clamp(0.0, 0.7);
        let pg = 1.0 / (1.0 - m * m).sqrt();
        let slope = 2.0 * PI * eta * pg;
        let alpha0 = -2.0 * KAPPA * f;

        // Cl_max: ~0.9 for a flat plate, +0.08 per % camber, lower at low Re
        let g = 0.85 + 0.15 * s;
        let cl_max = ((0.9 + 8.0 * f.max(0.0)) * g).max(0.5);
        let cl_min = (-(0.55 + 0.25 * s) + 4.0 * f.max(0.0) - 4.0 * (-f).max(0.0)).min(-0.25);

        // minimum (profile) drag
        let cd0_plate = 0.019 * (re / 7e4).powf(-0.5);
        let ff = |t: f64| 1.0 + 2.0 * t + 60.0 * t.powi(4);
        let cd0_air = 0.0115 * (re / 1e5).powf(-0.65) * ff(t) / ff(0.09);
        let cd_camber = w_thin * 12.0 * (f.abs() - 0.025).max(0.0).powi(2);
        let cd_min = w_thin * cd0_plate + (1.0 - w_thin) * cd0_air + cd_camber;
        // drag-bucket curvature of a thick airfoil (SD7037 / Clark Y at 60-100k)
        let k2 = (1.0 - w_thin) * 0.04 * (re / 1e5).powf(-0.3) + w_thin * 0.01;
        let cl_md = cl_max * soft_sat(slope * (0.0 - alpha0) / cl_max);

        let ar = aspect_ratio.clamp(1.0, 50.0);
        let cd_max = 1.11 + 0.018 * ar;
        PolarParams { slope, alpha0, cl_max, cl_min, cd_max, w_thin, cd_min, k2, cl_md }
    }

    pub fn stall_angle_pos(&self) -> f64 {
        (self.alpha0 + STALL_X * self.cl_max / self.slope).clamp(0.05, 1.2)
    }
    pub fn stall_angle_neg(&self) -> f64 {
        (self.alpha0 + STALL_X * self.cl_min / self.slope).clamp(-1.2, -0.05)
    }

    /// attached-flow (pre-stall) coefficients; also returns the linear lift
    fn attached(&self, alpha: f64) -> (Coeffs, f64) {
        let cl_lin = self.slope * (alpha - self.alpha0);
        let cl = if cl_lin >= 0.0 {
            self.cl_max * soft_sat(cl_lin / self.cl_max)
        } else {
            -self.cl_min * soft_sat(cl_lin / -self.cl_min)
        };
        let ta = alpha.tan().abs().min(3.0);
        // thin plates lose leading-edge suction: force stays normal to chord
        let cd_le = self.w_thin * cl.abs() * ta;
        // thick airfoils: lift lost to stall turns into pressure drag
        let cd_stall = (1.0 - self.w_thin) * 0.5 * (cl_lin.abs() - cl.abs()).max(0.0) * (alpha - self.alpha0).tan().abs().min(3.0);
        let cd = self.cd_min + self.k2 * (cl - self.cl_md).powi(2) + cd_le + cd_stall;
        (Coeffs { cl, cd }, cl_lin)
    }

    /// Viterna-Corrigan between the stall angle `a_s` (> 0) and 90 deg,
    /// matching Cl and Cd at the stall angle; `a` is in [a_s, pi/2].
    fn viterna(&self, a: f64, a_s: f64, cl_s: f64, cd_s: f64) -> Coeffs {
        let cdm = self.cd_max.max(cd_s);
        let (ss, cs) = a_s.sin_cos();
        let kl = (cl_s - cdm * ss * cs) * ss / (cs * cs);
        let kd = (cd_s - cdm * ss * ss) / cs;
        let (sa, ca) = a.sin_cos();
        Coeffs { cl: cdm * sa * ca + kl * ca * ca / sa, cd: cdm * sa * sa + kd * ca }
    }

    /// Cl, Cd at any angle (radians, wrapped to -pi..pi), without
    /// rotational augmentation. Second value: the linear (unstalled) lift.
    pub fn eval(&self, alpha: f64) -> (Coeffs, f64) {
        let mut a = alpha;
        if !a.is_finite() {
            return (Coeffs { cl: 0.0, cd: self.cd_max }, 0.0);
        }
        a = (a + PI).rem_euclid(2.0 * PI) - PI;
        let a_sp = self.stall_angle_pos();
        let a_sn = self.stall_angle_neg();
        if a > FRAC_PI_2 || a < -FRAC_PI_2 {
            // reversed flow: a flat plate, force normal to the chord
            let cn = self.cd_max * a.sin();
            let cd = (cn * a.sin()).max(self.cd_min);
            return (Coeffs { cl: cn * a.cos(), cd }, 0.0);
        }
        if a >= a_sn && a <= a_sp {
            return self.attached(a);
        }
        if a > a_sp {
            let (s, _) = self.attached(a_sp);
            (self.viterna(a, a_sp, s.cl, s.cd), self.slope * (a - self.alpha0))
        } else {
            let (s, _) = self.attached(a_sn);
            let c = self.viterna(-a, -a_sn, -s.cl, s.cd);
            (Coeffs { cl: -c.cl, cd: c.cd }, self.slope * (a - self.alpha0))
        }
    }
}

/// Section lift and drag at angle of attack `alpha` (radians).
pub fn coefficients(alpha: f64, st: &SectionState) -> Coeffs {
    let p = PolarParams::new(st.shape, st.reynolds, st.mach, st.aspect_ratio);
    let (mut c, cl_lin) = p.eval(alpha);
    if st.rotational_augmentation && alpha.abs() < FRAC_PI_2 {
        // Snel et al. 1994: only adds lift back toward the linear value
        let k = 3.0 * st.c_over_r.clamp(0.0, 1.0).powi(2);
        let dcl = cl_lin - c.cl;
        if dcl * c.cl > 0.0 || (c.cl.abs() < 1e-9 && dcl != 0.0) {
            c.cl += (k * dcl).clamp(-1.0, 1.0);
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(t: f64, f: f64, re: f64) -> SectionState {
        SectionState {
            shape: SectionShape { thickness: t, camber: f },
            reynolds: re,
            mach: 0.0,
            aspect_ratio: 6.0,
            c_over_r: 0.2,
            rotational_augmentation: false,
        }
    }

    #[test]
    fn continuous_and_finite_everywhere() {
        for &(t, f) in &[(0.02, 0.0), (0.03, 0.06), (0.12, 0.02), (0.08, 0.04), (0.2, -0.05)] {
            for &re in &[8e3, 3e4, 1e5, 4e5] {
                let s = st(t, f, re);
                let mut prev = coefficients(-PI, &s);
                for i in 1..=3600 {
                    let a = -PI + 2.0 * PI * i as f64 / 3600.0;
                    let c = coefficients(a, &s);
                    assert!(c.cl.is_finite() && c.cd.is_finite() && c.cd > 0.0, "{t} {f} {re} {a}: {c:?}");
                    assert!((c.cl - prev.cl).abs() < 0.08, "lift jump at {a} ({t},{f},{re}): {prev:?} -> {c:?}");
                    assert!((c.cd - prev.cd).abs() < 0.08, "drag jump at {a} ({t},{f},{re}): {prev:?} -> {c:?}");
                    prev = c;
                }
            }
        }
    }

    /// Against Balla & Vad (2022) Eq. 10-14, Tables 3-6, inside their range.
    #[test]
    fn cambered_plate_close_to_balla_vad() {
        const A: [[f64; 3]; 3] = [[7.25e-3, 1.45e-1, -5.76e-3], [7.16e-2, -2.58e-2, 2.28e-3], [6.39e-4, 3.74e-3, -3.59e-4]];
        const B: [[f64; 3]; 3] = [[-3.07e-7, 3.62e-7, -3.59e-8], [-8.72e-8, 6.53e-9, 1.00e-8], [5.23e-8, -9.12e-9, -8.08e-10]];
        const C: [[f64; 3]; 3] = [[1.78e-2, 5.38e-3, 1.05e-3], [-2.17e-3, -2.31e-4, 3.94e-4], [8.41e-4, 1.42e-4, -5.03e-5]];
        const D: [[f64; 3]; 3] = [[-1.41e-7, 1.46e-8, -6.09e-10], [5.30e-8, 1.69e-9, 1.04e-9], [-7.43e-9, -1.61e-9, 1.67e-11]];
        // row j = power of h/c (in %), column i = power of alpha (deg)
        let poly = |t: &[[f64; 3]; 3], hc: f64, a: f64| -> f64 {
            let mut s = 0.0;
            for (j, row) in t.iter().enumerate() {
                for (i, v) in row.iter().enumerate() {
                    s += v * hc.powi(j as i32) * a.powi(i as i32);
                }
            }
            s
        };
        let mut worst_cl: f64 = 0.0;
        let mut worst_cd: f64 = 0.0;
        let (mut ss, mut n) = (0.0, 0.0);
        for hc in [0.0, 2.0, 4.0, 6.0, 8.0] {
            for re in [40e3, 70e3, 140e3] {
                for a in [2.0, 4.0, 6.0] {
                    let cl_m = poly(&A, hc, a) - poly(&B, hc, a) * (1e5 - re);
                    let cd_m = poly(&C, hc, a) - poly(&D, hc, a) * (1e5 - re);
                    let c = coefficients(a.to_radians(), &st(0.03, hc / 100.0, re));
                    if std::env::var("BALLA").is_ok() { eprintln!("hc={hc} re={re} a={a}: cl {:.3} vs {:.3}  cd {:.4} vs {:.4}", c.cl, cl_m, c.cd, cd_m); }
                    worst_cl = worst_cl.max((c.cl - cl_m).abs());
                    ss += (c.cl - cl_m).powi(2);
                    n += 1.0;
                    worst_cd = worst_cd.max((c.cd - cd_m).abs() / cd_m);
                }
            }
        }
        let rms = (ss / n).sqrt();
        eprintln!("cambered plate vs Balla & Vad: rms dCl = {rms:.3}, worst |dCl| = {worst_cl:.3}, worst dCd/Cd = {:.0}%", worst_cd * 100.0);
        // worst case: 8 % camber at Re 140k, whose measured lift rises faster with Re
        assert!(rms < 0.09, "{rms}");
        assert!(worst_cl < 0.25, "{worst_cl}");
        assert!(worst_cd < 0.45, "{worst_cd}");
    }

    /// UIUC SD7037 (9.2 % thick, ~3 % camber) at Re ~60k: Cl ~0.80 at 6 deg,
    /// minimum drag ~0.016 (Selig et al. vol. 1).
    #[test]
    fn thick_airfoil_low_re() {
        let s = st(0.092, 0.03, 60e3);
        let c = coefficients(6f64.to_radians(), &s);
        assert!((c.cl - 0.80).abs() < 0.12, "{c:?}");
        let p = PolarParams::new(s.shape, 60e3, 0.0, 6.0);
        assert!((p.cd_min - 0.016).abs() < 0.004, "{}", p.cd_min);
    }
}
