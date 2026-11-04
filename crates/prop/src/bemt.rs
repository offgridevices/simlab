//! Blade element momentum theory for propellers, solved with Ning's
//! one-unknown residual so every blade element always converges.
//!
//! Ning, S. A., "A simple solution method for the blade element momentum
//! equations with guaranteed convergence", Wind Energy 17(9), 1327-1345, 2014
//! (doi:10.1002/we.1636); the propeller and hover forms follow Ning, "Using
//! blade element momentum methods with gradient-based design optimization",
//! Struct. Multidisc. Optim. 64, 2021 (CCBlade.jl).
//!
//! Unknown: the inflow angle phi between the rotation plane and the relative
//! wind. With
//!   sigma' = B c / (2 pi r),  k = sigma' cn / (4 F sin^2 phi),
//!   k' = sigma' ct / (4 F sin phi cos phi),
//!   a = k / (1 - k)  (axial induction, V (1 + a) through the disc),
//!   a' = k' / (1 + k')  (swirl, omega r (1 - a') in the plane),
//! the momentum and blade-element balance tan(phi) = V (1 + a) / (omega r (1 - a'))
//! becomes, multiplied through by sin(phi) (1 - k) ... :
//!
//!   R(phi) = sin^2 phi - sigma' cn / (4F) - lambda_r (sin phi cos phi + sigma' ct / (4F)),
//!   lambda_r = V / (omega r).
//!
//! This is smooth in V and stays valid in hover (V = 0, where it reduces to
//! k = 1). For normal propeller operation R < 0 as phi -> 0+ and R > 0 at
//! phi = pi/2, so Brent's method on (0, pi/2] always finds the root.
//! Prandtl tip and hub losses: F = (2/pi) acos(exp(-B (R - r) / (2 r sin phi))).

use crate::airfoil::{SectionState, coefficients};
use crate::geom::BladeGeometry;
use serde::{Deserialize, Serialize};
use simlab_core::Air;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

pub const G0: f64 = 9.80665;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ModelOptions {
    /// blade elements from root to tip (cosine-spaced toward the tip)
    pub elements: usize,
    pub tip_loss: bool,
    pub hub_loss: bool,
    /// Snel's stall-delay correction near the hub
    pub rotational_augmentation: bool,
    /// Prandtl-Glauert on the lift slope
    pub compressibility: bool,
    /// m/s (343 at 20 C)
    pub speed_of_sound: f64,
}

impl Default for ModelOptions {
    fn default() -> Self {
        ModelOptions {
            elements: 40,
            tip_loss: true,
            hub_loss: true,
            // validated: brings static CT of the UIUC 3D-printed DA4002 / DA4022
            // (known SDA1075 airfoil) from -19 % / -11 % to -7 % / 0 %
            rotational_augmentation: true,
            compressibility: true,
            speed_of_sound: 343.2,
        }
    }
}

/// Load and flow on one blade element (one radial band, all blades).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default)]
pub struct ElementLoad {
    /// band centre and edges, m
    pub r: f64,
    pub r_lo: f64,
    pub r_hi: f64,
    /// thrust per metre of radius, all blades, N/m
    pub dt_dr: f64,
    /// torque per metre of radius, all blades, N m / m
    pub dq_dr: f64,
    pub alpha_deg: f64,
    pub phi_deg: f64,
    pub cl: f64,
    pub cd: f64,
    pub reynolds: f64,
    pub mach: f64,
    /// axial and swirl induction factors
    pub a: f64,
    pub a_prime: f64,
    /// relative wind speed, m/s
    pub w: f64,
    /// Prandtl loss factor
    pub f: f64,
    pub converged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OperatingPoint {
    pub rpm: f64,
    pub airspeed_m_s: f64,
    pub thrust_n: f64,
    pub thrust_g: f64,
    pub torque_nm: f64,
    pub power_w: f64,
    /// UIUC / Brandt & Selig definitions: T / (rho n^2 D^4), P / (rho n^3 D^5)
    pub ct: f64,
    pub cp: f64,
    /// V / (n D)
    pub advance_ratio: f64,
    /// J CT / CP when moving forward and pushing
    pub efficiency: Option<f64>,
    /// hover figure of merit, T^1.5 / (P sqrt(2 rho A))
    pub figure_of_merit: Option<f64>,
    pub tip_mach: f64,
    /// Reynolds number at 75 % radius
    pub reynolds_75: f64,
    /// spanwise loads, root to tip
    pub elements: Vec<ElementLoad>,
    /// elements whose solution needed a fallback (should be empty)
    pub unconverged: usize,
}

/// Element layout: edges from root to tip, sine-spaced so they cluster at the tip.
pub fn element_edges(r_root: f64, r_tip: f64, n: usize) -> Vec<f64> {
    let n = n.max(4);
    (0..=n).map(|k| r_root + (r_tip - r_root) * (FRAC_PI_2 * k as f64 / n as f64).sin()).collect()
}

fn prandtl(b: f64, dist: f64, r_ref: f64, sphi: f64) -> f64 {
    let s = sphi.abs().max(1e-9);
    let f = 0.5 * b * dist / (r_ref * s);
    if f > 50.0 {
        return 1.0;
    }
    (2.0 / PI) * (-f).exp().clamp(0.0, 1.0).acos()
}

struct ElementCtx {
    b: f64,
    r: f64,
    chord: f64,
    beta: f64,
    sigma: f64,
    r_tip: f64,
    r_hub: f64,
    omega: f64,
    v: f64,
    nu: f64,
    state: SectionState,
    opts: ModelOptions,
}

struct Eval {
    res: f64,
    cl: f64,
    cd: f64,
    cn: f64,
    ct: f64,
    f: f64,
}

impl ElementCtx {
    fn eval(&self, phi: f64) -> Eval {
        let (s, c) = phi.sin_cos();
        let alpha = self.beta - phi;
        let w = self.omega * self.r / c.abs().max(0.1);
        let re = w * self.chord / self.nu;
        let mach = if self.opts.compressibility { w / self.opts.speed_of_sound } else { 0.0 };
        let mut st = self.state;
        st.reynolds = re;
        st.mach = mach;
        let co = coefficients(alpha, &st);
        let cn = co.cl * c - co.cd * s;
        let ct = co.cl * s + co.cd * c;
        let mut f = 1.0;
        if self.opts.tip_loss {
            f *= prandtl(self.b, self.r_tip - self.r, self.r, s);
        }
        if self.opts.hub_loss && self.r_hub > 1e-6 {
            f *= prandtl(self.b, self.r - self.r_hub, self.r_hub, s);
        }
        let f = f.max(1e-6);
        let lam = self.v / (self.omega * self.r);
        let q = self.sigma / (4.0 * f);
        let res = s * s - q * cn - lam * (s * c + q * ct);
        Eval { res, cl: co.cl, cd: co.cd, cn, ct, f }
    }
}

/// Brent's root finder on [a, b] with f(a), f(b) of opposite sign.
pub fn brent(mut f: impl FnMut(f64) -> f64, mut a: f64, mut b: f64, tol: f64, max_iter: usize) -> f64 {
    let mut fa = f(a);
    let mut fb = f(b);
    if fa == 0.0 {
        return a;
    }
    if fb == 0.0 {
        return b;
    }
    let mut c = a;
    let mut fc = fa;
    let mut d = b - a;
    let mut e = d;
    for _ in 0..max_iter {
        if fb * fc > 0.0 {
            c = a;
            fc = fa;
            d = b - a;
            e = d;
        }
        if fc.abs() < fb.abs() {
            a = b;
            b = c;
            c = a;
            fa = fb;
            fb = fc;
            fc = fa;
        }
        let tol1 = 2.0 * f64::EPSILON * b.abs() + 0.5 * tol;
        let xm = 0.5 * (c - b);
        if xm.abs() <= tol1 || fb == 0.0 {
            return b;
        }
        if e.abs() >= tol1 && fa.abs() > fb.abs() {
            let s = fb / fa;
            let (mut p, mut q);
            if a == c {
                p = 2.0 * xm * s;
                q = 1.0 - s;
            } else {
                let qq = fa / fc;
                let r = fb / fc;
                p = s * (2.0 * xm * qq * (qq - r) - (b - a) * (r - 1.0));
                q = (qq - 1.0) * (r - 1.0) * (s - 1.0);
            }
            if p > 0.0 {
                q = -q;
            }
            p = p.abs();
            let min1 = 3.0 * xm * q - (tol1 * q).abs();
            let min2 = (e * q).abs();
            if 2.0 * p < min1.min(min2) {
                e = d;
                d = p / q;
            } else {
                d = xm;
                e = d;
            }
        } else {
            d = xm;
            e = d;
        }
        a = b;
        fa = fb;
        b += if d.abs() > tol1 { d } else { tol1 * xm.signum() };
        fb = f(b);
    }
    b
}

/// Solves one operating point. `rpm` > 0; `airspeed` >= 0 is the axial
/// inflow (climb or a forward-flying airplane prop).
pub fn solve(geom: &BladeGeometry, rpm: f64, airspeed: f64, air: &Air, opts: &ModelOptions) -> OperatingPoint {
    let omega = rpm.abs() * TAU / 60.0;
    let v = airspeed.max(0.0);
    let b = geom.blades.max(1) as f64;
    let r_tip = geom.radius;
    let r_root = geom.hub_radius.max(geom.stations.first().map(|s| s.r).unwrap_or(0.0)).min(0.9 * r_tip);
    let edges = element_edges(r_root, r_tip, opts.elements);
    let ar = geom.aspect_ratio();
    let mut elements = Vec::with_capacity(edges.len() - 1);
    let mut thrust = 0.0;
    let mut torque = 0.0;
    let mut unconverged = 0;
    for w in edges.windows(2) {
        let (r_lo, r_hi) = (w[0], w[1]);
        let r = 0.5 * (r_lo + r_hi);
        let dr = r_hi - r_lo;
        let st = geom.at(r);
        let mut load = ElementLoad { r, r_lo, r_hi, converged: true, ..Default::default() };
        if omega <= 0.0 || st.chord <= 0.0 {
            elements.push(load);
            continue;
        }
        let ctx = ElementCtx {
            b,
            r,
            chord: st.chord,
            beta: st.beta,
            sigma: b * st.chord / (TAU * r),
            r_tip,
            r_hub: r_root,
            omega,
            v,
            nu: air.kinematic_viscosity,
            state: SectionState {
                shape: st.shape(),
                reynolds: 0.0,
                mach: 0.0,
                aspect_ratio: ar,
                c_over_r: st.chord / r,
                rotational_augmentation: opts.rotational_augmentation,
            },
            opts: *opts,
        };
        let (phi, ok) = find_phi(&ctx);
        let e = ctx.eval(phi);
        let (s, c) = phi.sin_cos();
        let kp = ctx.sigma * e.ct / (4.0 * e.f * s * c);
        let ap = if (1.0 + kp).abs() > 1e-9 { kp / (1.0 + kp) } else { 0.0 };
        let vt = omega * r * (1.0 - ap);
        let va = vt * phi.tan();
        let wrel = (va * va + vt * vt).sqrt();
        let q = 0.5 * air.density * wrel * wrel * st.chord * b;
        load.dt_dr = q * e.cn;
        load.dq_dr = q * e.ct * r;
        load.alpha_deg = (st.beta - phi).to_degrees();
        load.phi_deg = phi.to_degrees();
        load.cl = e.cl;
        load.cd = e.cd;
        load.reynolds = wrel * st.chord / air.kinematic_viscosity;
        load.mach = wrel / opts.speed_of_sound;
        load.a = if v > 1e-9 { va / v - 1.0 } else { f64::INFINITY };
        load.a_prime = ap;
        load.w = wrel;
        load.f = e.f;
        load.converged = ok;
        if !ok {
            unconverged += 1;
        }
        thrust += load.dt_dr * dr;
        torque += load.dq_dr * dr;
        elements.push(load);
    }
    let n = omega / TAU;
    let d = 2.0 * r_tip;
    let rho = air.density;
    let power = torque * omega;
    let (ct, cp, j) = if n > 0.0 {
        (thrust / (rho * n * n * d.powi(4)), power / (rho * n.powi(3) * d.powi(5)), v / (n * d))
    } else {
        (0.0, 0.0, 0.0)
    };
    let efficiency = if v > 1e-9 && thrust > 0.0 && power > 0.0 { Some(j * ct / cp) } else { None };
    let area = PI * r_tip * r_tip;
    let figure_of_merit =
        if v <= 1e-9 && thrust > 0.0 && power > 0.0 { Some(thrust.powf(1.5) / (power * (2.0 * rho * area).sqrt())) } else { None };
    let s75 = geom.at(0.75 * r_tip);
    let w75 = ((omega * 0.75 * r_tip).powi(2) + v * v).sqrt();
    OperatingPoint {
        rpm,
        airspeed_m_s: v,
        thrust_n: thrust,
        thrust_g: thrust / G0 * 1000.0,
        torque_nm: torque,
        power_w: power,
        ct,
        cp,
        advance_ratio: j,
        efficiency,
        figure_of_merit,
        tip_mach: (omega * r_tip).hypot(v) / opts.speed_of_sound,
        reynolds_75: w75 * s75.chord / air.kinematic_viscosity,
        elements,
        unconverged,
    }
}

fn find_phi(ctx: &ElementCtx) -> (f64, bool) {
    let f = |p: f64| ctx.eval(p).res;
    let lo = 1e-7;
    let hi = FRAC_PI_2;
    let (flo, fhi) = (f(lo), f(hi));
    if flo * fhi < 0.0 {
        return (brent(f, lo, hi, 1e-10, 100), true);
    }
    // scan for a sign change: positive (propeller) range first, then the
    // negative range (propeller brake), then the far side (windmill reverse)
    for (a, b) in [(lo, hi), (-FRAC_PI_2, -lo), (hi, PI - lo)] {
        let n = 90;
        let mut x0 = a;
        let mut f0 = f(a);
        for i in 1..=n {
            let x1 = a + (b - a) * i as f64 / n as f64;
            let f1 = f(x1);
            if f0 * f1 < 0.0 {
                return (brent(f, x0, x1, 1e-10, 100), true);
            }
            x0 = x1;
            f0 = f1;
        }
    }
    // no root: take the smallest residual (reported as not converged)
    let mut best = (lo, f64::INFINITY);
    for i in 0..=360 {
        let x = -FRAC_PI_2 + PI * i as f64 / 360.0;
        let v = f(x).abs();
        if v < best.1 {
            best = (x, v);
        }
    }
    (best.0, false)
}

/// The engine's sweep: every RPM step at every airspeed.
pub fn sweep(geom: &BladeGeometry, rpms: &[f64], airspeeds: &[f64], air: &Air, opts: &ModelOptions) -> Vec<OperatingPoint> {
    let mut out = Vec::with_capacity(rpms.len() * airspeeds.len());
    for &v in airspeeds {
        for &n in rpms {
            out.push(solve(geom, n, v, air, opts));
        }
    }
    out
}

/// `steps` RPMs evenly from `min` to `max` (inclusive).
pub fn rpm_steps(min: f64, max: f64, steps: u32) -> Vec<f64> {
    let n = steps.max(1) as usize;
    if n == 1 || (max - min).abs() < 1e-9 {
        return vec![max.max(min)];
    }
    (0..n).map(|i| min + (max - min) * i as f64 / (n - 1) as f64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airfoil::SectionShape;

    fn simple() -> BladeGeometry {
        // a 5-inch, constant-pitch rectangular blade (like the UIUC DA4002)
        let r = 0.0635;
        let xs: Vec<f64> = (0..15).map(|i| 0.3 + 0.05 * i as f64).collect();
        let beta: Vec<f64> = xs.iter().map(|x| (3.75 / (PI * 5.0 * x)).atan().to_degrees()).collect();
        BladeGeometry::from_table(2, r, &xs, &vec![0.18; xs.len()], &beta, |_| SectionShape { thickness: 0.12, camber: 0.02 })
    }

    #[test]
    fn residual_is_zero_at_the_solution_and_momentum_balances() {
        let g = simple();
        let air = Air::default();
        for &v in &[0.0, 5.0, 10.0] {
            let p = solve(&g, 6000.0, v, &air, &ModelOptions::default());
            assert_eq!(p.unconverged, 0);
            for e in &p.elements {
                // blade element thrust == momentum thrust on the annulus
                let va = e.w * e.phi_deg.to_radians().sin();
                let u = va - v;
                let mom = 4.0 * PI * e.r * air.density * va * u * e.f;
                assert!((mom - e.dt_dr).abs() < 1e-6 * (1.0 + e.dt_dr.abs()), "v={v} r={} {} vs {}", e.r, mom, e.dt_dr);
            }
        }
    }

    #[test]
    fn hover_limit_is_continuous() {
        let g = simple();
        let air = Air::default();
        let h = solve(&g, 6000.0, 0.0, &air, &ModelOptions::default());
        let s = solve(&g, 6000.0, 1e-4, &air, &ModelOptions::default());
        assert!((h.ct - s.ct).abs() < 1e-4 * h.ct, "{} {}", h.ct, s.ct);
        assert!((h.cp - s.cp).abs() < 1e-4 * h.cp);
        assert!(h.figure_of_merit.unwrap() > 0.3 && h.figure_of_merit.unwrap() < 0.85);
    }

    #[test]
    fn thrust_falls_with_advance_ratio_and_always_converges() {
        let g = simple();
        let air = Air::default();
        let mut last = f64::INFINITY;
        for i in 0..40 {
            let v = 0.5 * i as f64;
            let p = solve(&g, 5000.0, v, &air, &ModelOptions::default());
            assert_eq!(p.unconverged, 0, "v={v}");
            assert!(p.ct < last + 1e-9, "CT should fall with J (v={v})");
            last = p.ct;
        }
        assert!(last < 0.0, "windmills at high J");
    }
}
