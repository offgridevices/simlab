//! Reference cases with known answers, shared by the tests and the
//! `validate` example.

use crate::shapes::{self, PropSpec};
use crate::tunnel::{SolverOptions, WindResult, run_with};
use anyhow::Result;
use simlab_core::{Air, GridQuality, Progress, Spin, TriMesh, WindConfig};

/// Sphere drag, Clift, Grace & Weber (1978) correlations (Re < 4.4e4).
pub fn sphere_cd_cgw(re: f64) -> f64 {
    let w = re.log10();
    if re <= 20.0 {
        24.0 / re * (1.0 + 0.1315 * re.powf(0.82 - 0.05 * w))
    } else if re <= 260.0 {
        24.0 / re * (1.0 + 0.1935 * re.powf(0.6305))
    } else if re <= 1500.0 {
        10f64.powf(1.6435 - 1.1242 * w + 0.1558 * w * w)
    } else if re <= 1.2e4 {
        10f64.powf(-2.4571 + 2.5558 * w - 0.9295 * w * w + 0.1049 * w * w * w)
    } else {
        10f64.powf(-1.9181 + 0.6370 * w - 0.0636 * w * w)
    }
}

/// Schiller & Naumann (1933), Re < 800.
pub fn sphere_cd_sn(re: f64) -> f64 {
    24.0 / re * (1.0 + 0.15 * re.powf(0.687))
}

pub const SPHERE_D: f64 = 0.05;
pub const SPHERE_U: f64 = 10.0;

/// A sphere of 50 mm in a 10 m/s wind along +x with the viscosity set for `re`.
pub fn sphere_case(re: f64, cells_per_d: u32) -> (WindConfig, TriMesh) {
    let m = shapes::sphere([0.0; 3], SPHERE_D / 2.0, 5);
    let cfg = WindConfig {
        wind_dir: [1.0, 0.0, 0.0],
        speed_m_s: SPHERE_U,
        air: Air { density: 1.2, kinematic_viscosity: SPHERE_U * SPHERE_D / re },
        spin: None,
        quality: GridQuality::Custom { cells_across: cells_per_d },
        duration: None,
    };
    (cfg, m)
}

pub fn sphere_drag(re: f64, cells_per_d: u32, duration: f64, opt: &SolverOptions, progress: &dyn Progress) -> Result<(f64, WindResult)> {
    let (mut cfg, m) = sphere_case(re, cells_per_d);
    cfg.duration = Some(duration);
    let r = run_with(&cfg, &m, 1e-3, opt, progress)?;
    // Cd with the exact sphere area (the rasterized one is within 0.1 %)
    let a = std::f64::consts::PI * SPHERE_D * SPHERE_D / 4.0;
    let cd = r.summary.drag_n / (0.5 * cfg.air.density * SPHERE_U * SPHERE_U * a);
    Ok((cd, r))
}

/// Torque on a sphere spinning in still air at rotational Reynolds number
/// `re = omega R^2 / nu`, against Stokes' 8 pi mu R^3 omega.
pub fn rotating_sphere(re: f64, cells_per_d: u32, revs: f64, opt: &SolverOptions, progress: &dyn Progress) -> Result<(f64, f64, WindResult)> {
    let r = 0.025;
    let m = shapes::sphere([0.0; 3], r, 5);
    let rpm = 600.0;
    let omega = rpm * 2.0 * std::f64::consts::PI / 60.0;
    let nu = omega * r * r / re;
    let air = Air { density: 1.2, kinematic_viscosity: nu };
    let cfg = WindConfig {
        wind_dir: [0.0, 0.0, 1.0],
        speed_m_s: 0.0,
        air,
        spin: Some(Spin { rpm, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3] }),
        quality: GridQuality::Custom { cells_across: cells_per_d },
        duration: Some(revs),
    };
    let res = run_with(&cfg, &m, 1e-3, opt, progress)?;
    let stokes = 8.0 * std::f64::consts::PI * air.density * nu * r * r * r * omega;
    Ok((res.summary.torque_nm.unwrap_or(0.0), stokes, res))
}

/// The built-in test propeller spinning in still air.
pub fn prop_hover(spec: &PropSpec, rpm: f64, cells_across: u32, revs: f64, opt: &SolverOptions, progress: &dyn Progress) -> Result<WindResult> {
    let m = spec.mesh();
    let cfg = WindConfig {
        wind_dir: [0.0, 0.0, 1.0],
        speed_m_s: 0.0,
        air: Air::default(),
        spin: Some(Spin { rpm, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3] }),
        quality: GridQuality::Custom { cells_across },
        duration: Some(revs),
    };
    run_with(&cfg, &m, 1e-3, opt, progress)
}
