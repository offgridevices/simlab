//! Propeller performance for SimLab.
//!
//! 1. [`extract_geometry`] reads blade count, tip and root radius, and chord,
//!    pitch, thickness and camber at ~25 radial stations from the mesh, and
//!    works out which way it must spin to push along `axis`;
//!    [`detect_prop`] guesses the axis, centre and blade count.
//! 2. [`solve`] / [`sweep`] run blade element momentum theory (Ning's
//!    guaranteed-convergence residual, Prandtl tip and hub loss) with a
//!    low-Reynolds airfoil model ([`airfoil`]) for hover and axial flight.
//! 3. [`face_tractions`] turns one operating point's spanwise load into
//!    surface tractions for the stress test.
//!
//! Everything is SI inside (m, N, N m, W); grams-force only where named.

pub mod airfoil;
pub mod bemt;
pub mod extract;
pub mod geom;
pub mod loft;
pub mod traction;

pub use airfoil::{Coeffs, SectionShape, SectionState};
pub use bemt::{ElementLoad, G0, ModelOptions, OperatingPoint, rpm_steps, solve, sweep};
pub use extract::{ExtractOptions, ExtractedProp, Frame, PropGuess, detect_prop, extract_geometry};
pub use geom::{BladeGeometry, Rotation, Station};
pub use traction::{BoundaryFace, face_tractions};

use serde::{Deserialize, Serialize};
use simlab_core::{Cancelled, PropConfig, Progress, TriMesh};

/// The result of a propeller study.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PropRun {
    pub extracted: ExtractedProp,
    pub points: Vec<OperatingPoint>,
    pub warnings: Vec<String>,
}

/// One row of `PropSummary.geometry` (docs/API.md)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeometryRow {
    pub r_mm: f64,
    pub chord_mm: f64,
    pub pitch_deg: f64,
    pub thickness_pct: f64,
    pub camber_pct: f64,
}

/// One row of `PropSummary.points` (docs/API.md)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PointRow {
    pub rpm: f64,
    pub airspeed_m_s: f64,
    pub thrust_n: f64,
    pub thrust_g: f64,
    pub torque_nm: f64,
    pub power_w: f64,
    pub ct: f64,
    pub cp: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<f64>,
}

/// `PropSummary` exactly as docs/API.md describes it (kind = "prop").
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PropSummary {
    pub kind: String,
    pub headline: String,
    pub blades: u32,
    pub radius_mm: f64,
    pub hub_radius_mm: f64,
    pub rotation: Rotation,
    pub geometry: Vec<GeometryRow>,
    pub points: Vec<PointRow>,
    pub warnings: Vec<String>,
}

impl PropRun {
    pub fn summary(&self) -> PropSummary {
        let g = &self.extracted.geometry;
        let top = self.points.iter().filter(|p| p.airspeed_m_s == 0.0).max_by(|a, b| a.rpm.partial_cmp(&b.rpm).unwrap());
        let headline = match top {
            Some(p) => format!(
                "At {} rpm it pushes about {:.0} g and needs {:.0} W at the shaft ({}, {} blades).",
                group_thousands(p.rpm),
                p.thrust_g,
                p.power_w,
                if g.rotation == Rotation::Ccw { "spins counter-clockwise seen from above" } else { "spins clockwise seen from above" },
                g.blades
            ),
            None => format!("{}-blade propeller, {:.1} mm across.", g.blades, 2000.0 * g.radius),
        };
        PropSummary {
            kind: "prop".into(),
            headline,
            blades: g.blades,
            radius_mm: g.radius * 1e3,
            hub_radius_mm: g.hub_radius * 1e3,
            rotation: g.rotation,
            geometry: g
                .stations
                .iter()
                .map(|s| GeometryRow {
                    r_mm: s.r * 1e3,
                    chord_mm: s.chord * 1e3,
                    pitch_deg: s.beta.to_degrees(),
                    thickness_pct: s.thickness * 100.0,
                    camber_pct: s.camber * 100.0,
                })
                .collect(),
            points: self
                .points
                .iter()
                .map(|p| PointRow {
                    rpm: p.rpm,
                    airspeed_m_s: p.airspeed_m_s,
                    thrust_n: p.thrust_n,
                    thrust_g: p.thrust_g,
                    torque_nm: p.torque_nm,
                    power_w: p.power_w,
                    ct: p.ct,
                    cp: p.cp,
                    efficiency: p.efficiency,
                })
                .collect(),
            warnings: self.warnings.clone(),
        }
    }
}

fn group_thousands(x: f64) -> String {
    let n = x.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The whole propeller study: extraction, then the RPM x airspeed sweep.
/// `mesh` is in model units (`model_unit_m` metres each); `cfg.center_mm`
/// is in model millimetres.
pub fn run(mesh: &TriMesh, model_unit_m: f64, cfg: &PropConfig, progress: &dyn Progress) -> anyhow::Result<PropRun> {
    progress.report(0.0, "Reading the blades from the model");
    let center = simlab_core::scale(cfg.center_mm, 1e-3 / model_unit_m);
    let ex = extract_geometry(mesh, model_unit_m, cfg.axis, center, &ExtractOptions { stations: 25, blades: cfg.blades })?;
    if progress.cancelled() {
        return Err(Cancelled.into());
    }
    let mut warnings = ex.warnings.clone();
    progress.report(0.3, &format!("Found {} blades, {:.1} mm across", ex.geometry.blades, ex.geometry.radius * 2000.0));
    let opts = ModelOptions::default();
    let rpms = rpm_steps(cfg.rpm_min, cfg.rpm_max, cfg.rpm_steps);
    let speeds = if cfg.airspeeds_m_s.is_empty() { vec![0.0] } else { cfg.airspeeds_m_s.clone() };
    let mut points = Vec::with_capacity(rpms.len() * speeds.len());
    let total = (rpms.len() * speeds.len()).max(1);
    for &v in &speeds {
        for &n in &rpms {
            if progress.cancelled() {
                return Err(Cancelled.into());
            }
            let p = solve(&ex.geometry, n, v, &cfg.air, &opts);
            progress.metric("thrust_g", points.len() as u64, p.thrust_g);
            points.push(p);
            progress.report(0.3 + 0.7 * points.len() as f32 / total as f32, &format!("Solved {} of {total} operating points", points.len()));
        }
    }
    let mut low_re = false;
    let mut fast_tip = false;
    let mut stalled = false;
    for p in &points {
        low_re |= p.reynolds_75 < 2e4;
        fast_tip |= p.tip_mach > 0.7;
        stalled |= p.elements.iter().filter(|e| e.r > 0.4 * ex.geometry.radius).any(|e| e.alpha_deg > 15.0);
        if p.unconverged > 0 {
            warnings.push(format!("{} blade elements had no exact solution at {:.0} rpm, {:.1} m/s.", p.unconverged, p.rpm, p.airspeed_m_s));
        }
    }
    if low_re {
        warnings.push("Below about 20,000 Reynolds number (very low rpm) the air model is less certain.".into());
    }
    if fast_tip {
        warnings.push("The blade tips pass 70 % of the speed of sound; above this the results are less reliable.".into());
    }
    if stalled {
        warnings.push("Parts of the blades are stalled at some operating points; thrust there is less certain.".into());
    }
    if speeds.iter().any(|&v| v > 0.0) {
        warnings.push("Airspeed is treated as air flowing straight into the propeller (climbing, or an airplane); a quad's sideways flow is not modelled.".into());
    }
    progress.report(1.0, "Done");
    Ok(PropRun { extracted: ex, points, warnings })
}
