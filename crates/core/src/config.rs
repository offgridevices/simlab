//! Study configurations, exactly as the UI, the database and MCP store them
//! (JSON). Positions and sizes the user picks on the model are in model
//! millimetres (`_mm`); everything else is SI unless the name says otherwise.

use crate::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StudyConfig {
    Strength(StrengthConfig),
    Wind(WindConfig),
    Prop(PropConfig),
}

// ---------------------------------------------------------------- strength

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StrengthConfig {
    pub material_id: String,
    /// the printer's build direction (layer stacking, "up") in model axes
    #[serde(default = "z_up")]
    pub print_up: Vec3,
    pub support: Support,
    pub loads: Vec<Load>,
    #[serde(default)]
    pub quality: MeshQuality,
    /// areas picked by shape instead of by face, e.g. "the hub: everything
    /// within 4 mm of the spin axis" or "where I clicked, 3 mm around".
    /// Each has an id ≥ 1,000,000 that supports and loads list like a patch.
    #[serde(default)]
    pub regions: Vec<Region>,
}

/// First id given to a region (patch ids from the mesher are far below).
pub const REGION_ID_BASE: u32 = 1_000_000;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Region {
    pub id: u32,
    #[serde(default)]
    pub label: String,
    pub shape: RegionShape,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RegionShape {
    /// surface inside a ball (a click and a radius)
    Sphere { center_mm: Vec3, radius_mm: f64 },
    /// surface within `radius_mm` of a line, optionally only within
    /// `half_length_mm` along it from `center_mm` (a hub, a bolt hole)
    Cylinder { axis: Vec3, center_mm: Vec3, radius_mm: f64, #[serde(default)] half_length_mm: Option<f64> },
    /// surface inside an axis-aligned box
    Box { min_mm: Vec3, max_mm: Vec3 },
}

impl RegionShape {
    pub fn contains(&self, p_mm: Vec3) -> bool {
        use crate::{dot, norm, normalize, sub};
        match self {
            RegionShape::Sphere { center_mm, radius_mm } => norm(sub(p_mm, *center_mm)) <= *radius_mm,
            RegionShape::Cylinder { axis, center_mm, radius_mm, half_length_mm } => {
                let a = normalize(*axis);
                let d = sub(p_mm, *center_mm);
                let along = dot(d, a);
                let radial = norm(sub(d, crate::scale(a, along)));
                radial <= *radius_mm && half_length_mm.map(|h| along.abs() <= h).unwrap_or(true)
            }
            RegionShape::Box { min_mm, max_mm } => (0..3).all(|k| p_mm[k] >= min_mm[k] && p_mm[k] <= max_mm[k]),
        }
    }
}

pub fn z_up() -> Vec3 {
    [0.0, 0.0, 1.0]
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Support {
    /// these surface patches are held still (bolted, clamped, glued)
    Fixed { patches: Vec<u32> },
    /// nothing holds the part: it is flying, and the loads accelerate it
    /// (inertia relief). The right choice for a frame in the air.
    FreeFlying,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Load {
    /// a total force spread evenly over the patches' area
    Force { patches: Vec<u32>, force_n: Vec3, #[serde(default)] label: String },
    /// pressure pushing into the surface (negative pulls)
    Pressure { patches: Vec<u32>, pressure_pa: f64, #[serde(default)] label: String },
    /// a motor bolted to these patches (its mount holes or pad): thrust along
    /// `axis`, reaction torque about it, its weight under `accel_g`
    Motor {
        patches: Vec<u32>,
        thrust_n: f64,
        torque_nm: f64,
        axis: Vec3,
        #[serde(default)]
        motor_mass_g: f64,
        #[serde(default)]
        label: String,
    },
    /// the whole part accelerates (in g; [0,0,-1] is plain gravity; a crash
    /// landing might be [0,0,-30])
    Acceleration { g: Vec3, #[serde(default)] label: String },
    /// the part spins (centrifugal load), e.g. a propeller
    Spin { rpm: f64, axis: Vec3, center_mm: Vec3, #[serde(default)] label: String },
    /// air load on a propeller at this RPM and airspeed, from the blade
    /// element model, applied to the blades' faces
    PropAero {
        rpm: f64,
        #[serde(default)]
        airspeed_m_s: f64,
        axis: Vec3,
        center_mm: Vec3,
        #[serde(default)]
        label: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeshQuality {
    Draft,
    #[default]
    Normal,
    Fine,
    /// element size in millimetres
    Custom { size_mm: f64 },
}

// ---------------------------------------------------------------- wind

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Air {
    /// kg/m³
    pub density: f64,
    /// m²/s
    pub kinematic_viscosity: f64,
}
impl Default for Air {
    /// sea level, 20 °C
    fn default() -> Self {
        Air { density: 1.204, kinematic_viscosity: 1.516e-5 }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WindConfig {
    /// the direction the air moves, in model axes
    pub wind_dir: Vec3,
    /// m/s; 0 is allowed with `spin` (a hovering propeller)
    pub speed_m_s: f64,
    #[serde(default)]
    pub air: Air,
    #[serde(default)]
    pub spin: Option<Spin>,
    #[serde(default)]
    pub quality: GridQuality,
    /// how long to simulate: flow passes over the part (still air: propeller
    /// revolutions). None = the engine's default for the quality.
    #[serde(default)]
    pub duration: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Spin {
    pub rpm: f64,
    /// right-hand rule: positive rpm turns counter-clockwise looking down `axis`
    pub axis: Vec3,
    pub center_mm: Vec3,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GridQuality {
    Draft,
    #[default]
    Normal,
    Fine,
    /// grid cells across the part's largest size
    Custom { cells_across: u32 },
}

// ---------------------------------------------------------------- propeller

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PropConfig {
    /// the direction the propeller pushes the drone (thrust), model axes
    pub axis: Vec3,
    pub center_mm: Vec3,
    pub rpm_min: f64,
    pub rpm_max: f64,
    #[serde(default = "default_steps")]
    pub rpm_steps: u32,
    /// forward speeds to sweep; [0] is hover
    #[serde(default = "hover")]
    pub airspeeds_m_s: Vec<f64>,
    #[serde(default)]
    pub air: Air,
    /// None = count blades from the model
    #[serde(default)]
    pub blades: Option<u32>,
}
fn default_steps() -> u32 {
    12
}
fn hover() -> Vec<f64> {
    vec![0.0]
}

// ---------------------------------------------------------------- motors

/// A motor preset; thrust from the maker's or a bench table at full throttle
/// on the given prop and voltage.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Motor {
    pub id: String,
    pub name: String,
    pub kv: f64,
    pub mass_g: f64,
    pub max_thrust_g: f64,
    pub max_current_a: f64,
    pub voltage_v: f64,
    pub prop: String,
    /// bolt pattern in mm (e.g. 9, 12, 16, 19)
    pub mount_mm: f64,
    #[serde(default)]
    pub notes: String,
}

impl Motor {
    pub fn max_thrust_n(&self) -> f64 {
        self.max_thrust_g * 9.80665e-3
    }
    /// shaft torque at full throttle, from electrical power with a typical
    /// 80 % efficiency over the shaft speed (RPM = kv × V × ~0.75 loaded)
    pub fn est_torque_nm(&self) -> f64 {
        let p_shaft = 0.8 * self.max_current_a * self.voltage_v;
        let rpm = self.kv * self.voltage_v * 0.75;
        p_shaft / (rpm * std::f64::consts::TAU / 60.0)
    }
}
