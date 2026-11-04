//! Printed-part materials. FDM parts are weaker across layers (Z) than along
//! them (XY); the stress test treats them as transversely isotropic about the
//! print direction. Resin parts are close to isotropic.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Process {
    Fdm,
    Resin,
    /// metals, carbon plate, anything not printed
    Other,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Material {
    pub id: String,
    pub name: String,
    pub process: Process,
    /// kg/m³
    pub density: f64,
    /// Young's modulus along the layers (MPa)
    pub e_xy_mpa: f64,
    /// Young's modulus across the layers (MPa); equals e_xy for isotropic
    pub e_z_mpa: f64,
    pub nu: f64,
    /// tensile strength along the layers (MPa)
    pub uts_xy_mpa: f64,
    /// tensile strength across the layers, i.e. layer adhesion (MPa)
    pub uts_z_mpa: f64,
    /// the maker's own across-layer test value, when `uts_z_mpa` was lowered
    /// by `z_knockdown` for real-world prints
    #[serde(default)]
    pub uts_z_measured_mpa: Option<f64>,
    #[serde(default)]
    pub z_knockdown: Option<f64>,
    #[serde(default)]
    pub flex_strength_mpa: Option<f64>,
    #[serde(default)]
    pub elongation_pct: Option<f64>,
    #[serde(default)]
    pub hdt_c: Option<f64>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub estimated_fields: Vec<String>,
}

impl Material {
    pub fn isotropic(id: &str, name: &str, density: f64, e_mpa: f64, nu: f64, uts_mpa: f64) -> Material {
        Material {
            id: id.into(),
            name: name.into(),
            process: Process::Other,
            density,
            e_xy_mpa: e_mpa,
            e_z_mpa: e_mpa,
            nu,
            uts_xy_mpa: uts_mpa,
            uts_z_mpa: uts_mpa,
            uts_z_measured_mpa: None,
            z_knockdown: None,
            flex_strength_mpa: None,
            elongation_pct: None,
            hdt_c: None,
            notes: String::new(),
            sources: vec![],
            estimated_fields: vec![],
        }
    }
    pub fn is_isotropic(&self) -> bool {
        (self.e_xy_mpa - self.e_z_mpa).abs() < 1e-9 * self.e_xy_mpa && (self.uts_xy_mpa - self.uts_z_mpa).abs() < 1e-9 * self.uts_xy_mpa
    }
}

/// The built-in library, generated from researched data sheets (see
/// `materials.json` next to this file for the sources of every value).
pub fn builtin_materials() -> Vec<Material> {
    serde_json::from_str(include_str!("materials.json")).expect("built-in materials.json is valid")
}

#[cfg(test)]
mod tests {
    #[test]
    fn library_loads() {
        let m = super::builtin_materials();
        assert!(m.len() >= 20);
        for x in &m {
            assert!(x.e_xy_mpa > 0.0 && x.e_z_mpa > 0.0 && x.uts_z_mpa > 0.0 && x.density > 0.0, "{}", x.id);
            assert!(x.uts_z_mpa <= x.uts_xy_mpa + 1e-9, "{}", x.id);
        }
    }
}
