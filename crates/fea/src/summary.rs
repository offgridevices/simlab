//! The API's `StrengthSummary` (docs/API.md) and the per-node display fields.

use crate::strength::{SAFE_FACTOR, StrengthResult};
use serde::{Deserialize, Serialize};
use simlab_core::{Vec3, norm};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// safety factor ≥ 2 (the usual design rule for printed parts)
    Safe,
    /// 1 ≤ safety factor < 2
    Marginal,
    /// safety factor < 1
    Breaks,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BreaksAt {
    /// multiply every load by this to reach first failure
    pub load_factor: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm: Option<f64>,
    pub text: String,
}

/// Matches `StrengthSummary` in docs/API.md (`kind: "strength"`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrengthSummary {
    pub kind: String,
    pub headline: String,
    pub verdict: Verdict,
    pub safety_factor: f64,
    pub max_von_mises_mpa: f64,
    pub max_principal_mpa: f64,
    pub max_displacement_mm: f64,
    /// model units (mm for millimetre models)
    pub critical_point_mm: Vec3,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical_patch: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breaks_at: Option<BreaksAt>,
    pub mass_g: f64,
    pub elements: usize,
    pub nodes: usize,
    pub dofs: usize,
    pub solver: String,
    pub solve_seconds: f64,
    /// filled in by the engine (the mesher runs outside this crate)
    pub mesh_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactions_n: Option<Vec3>,
    pub warnings: Vec<String>,
}

/// One f32 per mesh node; `TetMesh::display_surface()` keeps the node
/// numbering, so these index its vertices directly.
#[derive(Clone, Debug)]
pub struct NodalField {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub values: Vec<f32>,
    pub min: f32,
    pub max: f32,
    pub diverging: bool,
}

fn fmt_rpm(r: f64) -> String {
    let r = (r / 100.0).round() * 100.0;
    let s = format!("{:.0}", r);
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl StrengthResult {
    pub fn verdict(&self) -> Verdict {
        if self.safety_factor >= SAFE_FACTOR {
            Verdict::Safe
        } else if self.safety_factor >= 1.0 {
            Verdict::Marginal
        } else {
            Verdict::Breaks
        }
    }

    pub fn summary(&self) -> StrengthSummary {
        let sf = self.safety_factor;
        let verdict = self.verdict();
        let breaks_text = match self.break_rpm {
            Some(rpm) => format!("Breaks at about {} rpm", fmt_rpm(rpm)),
            None if sf.is_finite() => format!("Breaks at about {:.1}× this load", sf),
            None => "Does not break under this load".into(),
        };
        let headline = match (verdict, self.break_rpm) {
            (_, Some(brk)) if sf.is_finite() => {
                let studied = brk / sf.sqrt();
                match verdict {
                    Verdict::Breaks => format!("Breaks at about {} rpm, below the {} rpm studied", fmt_rpm(brk), fmt_rpm(studied)),
                    v => format!("{}: breaks at about {} rpm, {:.2}× the {} rpm studied", verdict_word(v), fmt_rpm(brk), sf.sqrt(), fmt_rpm(studied)),
                }
            }
            (Verdict::Safe, _) if sf.is_finite() => format!("Safe: {:.1}× stronger than this load needs", sf),
            (Verdict::Safe, _) => "Safe: this load puts no stress on the part".into(),
            (Verdict::Marginal, _) => format!("Marginal: only {:.1}× stronger than this load needs", sf),
            (Verdict::Breaks, _) => format!("Breaks: this load is {:.1}× more than the part can take", 1.0 / sf),
        };
        let reactions = if self.inertia_relief.is_none() { Some(self.reaction_force_n) } else { None };
        StrengthSummary {
            kind: "strength".into(),
            headline,
            verdict,
            safety_factor: sf,
            max_von_mises_mpa: self.peaks.von_mises / 1e6,
            max_principal_mpa: self.peaks.max_principal / 1e6,
            max_displacement_mm: self.max_displacement_m * 1e3,
            critical_point_mm: self.critical.position_model,
            critical_patch: self.critical.patch,
            breaks_at: if sf.is_finite() { Some(BreaksAt { load_factor: self.load_factor, rpm: self.break_rpm, text: breaks_text }) } else { None },
            mass_g: self.mass_kg * 1e3,
            elements: self.elements,
            nodes: self.nodes,
            dofs: self.solver.dofs,
            solver: format!("{}: {}", self.solver.method, self.solver.detail),
            solve_seconds: self.timings.total_s,
            mesh_seconds: 0.0,
            reactions_n: reactions,
            warnings: self.warnings.clone(),
        }
    }

    /// Display fields, one value per node: von Mises (MPa), max principal
    /// (MPa), failure index, safety factor (capped at 100), displacement (mm).
    pub fn surface_fields(&self) -> Vec<NodalField> {
        fn mk(name: &'static str, label: &'static str, unit: &'static str, v: Vec<f32>, diverging: bool) -> NodalField {
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &x in &v {
                if x.is_finite() {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
            NodalField { name, label, unit, values: v, min: lo, max: hi, diverging }
        }
        vec![
            mk("von_mises", "Stress (von Mises)", "MPa", self.von_mises.iter().map(|v| (v / 1e6) as f32).collect(), false),
            mk("max_principal", "Largest pull (max principal stress)", "MPa", self.max_principal.iter().map(|v| (v / 1e6) as f32).collect(), true),
            mk("failure_index", "Failure index (1 = breaks)", "", self.failure_index.iter().map(|&v| v as f32).collect(), false),
            mk(
                "safety_factor",
                "Safety factor",
                "×",
                self.failure_index.iter().map(|&v| if v > 1e-4 { (1.0 / v.sqrt()) as f32 } else { 100.0 }).collect(),
                false,
            ),
            mk("displacement", "Movement", "mm", self.displacement.iter().map(|d| (norm(*d) * 1e3) as f32).collect(), false),
        ]
    }
}

fn verdict_word(v: Verdict) -> &'static str {
    match v {
        Verdict::Safe => "Safe",
        Verdict::Marginal => "Marginal",
        Verdict::Breaks => "Breaks",
    }
}
