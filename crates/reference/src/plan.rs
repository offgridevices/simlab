//! Turns a part and a wind-tunnel config into the numbers a case needs:
//! the domain, cell sizes, refinement, the spinning zone, boundary and
//! turbulence settings. Everything here is pure arithmetic, so it is easy
//! to test without OpenFOAM.
//!
//! ## Two kinds of case
//!
//! **Wind** (no spin, or spin with the wind blowing). A box aligned with the
//! wind, like a wind tunnel's test section, sized from L = the part's
//! largest size across:
//!
//! | margin | size |
//! |---|---|
//! | upstream of the part | 4 L |
//! | downstream | 10 L |
//! | each side, above, below | 4 L |
//!
//! That keeps the blockage (frontal area over the tunnel section) near 1 %,
//! so the walls change drag by well under 2 %. Inlet: fixed wind speed;
//! outlet: fixed pressure; sides: slip walls (a wind tunnel without
//! boundary layers on its walls). Background cells are L/5; the surface
//! is refined to L/80 (Draft), L/160 (Normal) or L/320 (Fine), with finer
//! cells where it curves sharply.
//!
//! **Spinning part in still air** (spin, speed 0: a hovering propeller). The
//! proven setup from the user's 3-inch propeller study (OpenFOAM v2506,
//! 23 runs), scaled by the rotor radius R: a box 10.5 R wide, 3.9 R
//! above the rotor and 11.8 R below it on the wake side, background cells
//! 0.26 R, an MRF zone (a cylinder 1.085 R in radius, 0.1 R past the part
//! axially), near-rotor and wake refinement cylinders, the blade surface at
//! level 5/6/6–7. Every outer face is open (totalPressure p0 = 0 with
//! pressureInletOutletVelocity), so the propeller draws air in and the jet
//! leaves freely. The wake side comes from the blade shape (geom::jet_sign).
//!
//! ## Walls and y+
//!
//! Drone parts are small and slow: chord Reynolds numbers of 10⁴–10⁵, so a
//! first cell of L/160 sits at y+ of roughly 1–30 (the propeller study
//! measured y+ 0.3 / 23 / 113, min/avg/max). No single wall treatment is
//! right for all of that, so the case uses the y+-insensitive set:
//! nutUSpaldingWallFunction (Spalding's law, valid from the viscous
//! sublayer to the log layer), omegaWallFunction (blends the viscous and log
//! values of ω) and kqRWallFunction. Wind cases add 3 (Normal) or 5 (Fine)
//! prism layers on the part (relative thickness 0.4 of the surface cell,
//! growth 1.2) so the wall gradient is resolved where snappyHexMesh can grow
//! them; spinning cases add none, as in the proven propeller runs, because
//! layers collapse at thin blade edges and degrade the mesh there.
//!
//! ## Turbulence
//!
//! k-ω SST, except when the part's Reynolds number (U L / ν) is below 300
//! with no spin: there the flow round a part is laminar with a steady wake
//! (a sphere sheds vortices only above Re ≈ 270), and SST would only smear
//! it. Above that the real wake is unsteady; a steady laminar solve then
//! never settles (sphere at Re 1000: Cd swung ±17 %), while SST settles to
//! within 3 % of the measured Cd, so SST is used. Inlet turbulence is low,
//! like calm air or a good wind tunnel: intensity 0.5 % and eddy viscosity
//! equal to the air's. Hover cases keep the propeller study's values
//! (k 0.06 m²/s², ω 400 1/s).

use crate::geom::{Frame, extents_in, jet_sign, open_edges, projected_area, radial_extent};
use simlab_core::{GridQuality, TriMesh, Vec3, WindConfig, add, norm, normalize, scale, sub};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// part held still in a wind
    Wind,
    /// spinning part (MRF), still air: every outer face open
    SpinHover,
    /// spinning part (MRF) in a wind: inlet, outlet, slip sides
    SpinWind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Turbulence {
    Laminar,
    KOmegaSst { k: f64, omega: f64 },
    /// k-ω SST with the Langtry–Menter γ–Reθ transition model: the
    /// boundary layer stays laminar until it is tripped or separates, as on a
    /// small part in calm air
    KOmegaSstLm { k: f64, omega: f64, re_theta: f64 },
}

impl Turbulence {
    /// free-stream k and ω (None for laminar)
    pub fn k_omega(&self) -> Option<(f64, f64)> {
        match *self {
            Turbulence::Laminar => None,
            Turbulence::KOmegaSst { k, omega } | Turbulence::KOmegaSstLm { k, omega, .. } => Some((k, omega)),
        }
    }
}

/// A time-accurate run (pimpleFoam): a steady start, then the flow followed
/// in time (unsteady RANS) and the forces averaged once the start-up is over.
#[derive(Clone, Debug, PartialEq)]
pub struct Transient {
    /// steady iterations (same model) that give the time-accurate run its start
    pub init_iterations: u32,
    /// one flow pass over the part, L / U (s)
    pub flow_time: f64,
    /// the run stops here at the latest (s)
    pub end_time: f64,
    /// forces are averaged from here on, leaving out the start-up (s)
    pub average_from: f64,
    /// the shortest averaging time before the run may stop early (s)
    pub min_average: f64,
    /// largest Courant number OpenFOAM sizes the time step for
    pub max_co: f64,
    /// first time step (s); OpenFOAM then grows it up to max_co
    pub dt0: f64,
    /// the time step max_co gives in the smallest cells near the part (s), for estimates
    pub dt_estimate: f64,
    /// PIMPLE outer correctors per time step
    pub outer_correctors: u32,
}

impl Transient {
    /// time steps to the end time (for estimates)
    pub fn steps(&self) -> u64 {
        (self.end_time / self.dt_estimate.max(1e-12)).ceil() as u64
    }
}

#[derive(Clone, Debug)]
pub struct Rotor {
    /// unit vector of the angular velocity (right-hand rule), model axes
    pub axis: Vec3,
    /// rad/s, ≥ 0 (the sign is folded into `axis`)
    pub omega: f64,
    pub center: Vec3,
    /// MRF zone cylinder (m)
    pub zone_p1: Vec3,
    pub zone_p2: Vec3,
    pub zone_radius: f64,
    /// the part's radius about the axis
    pub radius: f64,
    /// unit vector the wake leaves along (−thrust direction)
    pub jet_dir: Vec3,
    /// how clearly the blade shape says which way it blows (0..1)
    pub jet_vote: f64,
    /// refinement level of the zone's surface
    pub level: u32,
}

#[derive(Clone, Debug)]
pub enum Region {
    /// refine everything inside a cylinder to `level`
    Cylinder { name: String, p1: Vec3, p2: Vec3, radius: f64, level: u32 },
    /// refine within these distances of the part: (distance m, level)
    NearPart { levels: Vec<(f64, u32)> },
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub kind: Kind,
    /// the domain's frame: e1 = flow (wind, or the wake for hover)
    pub frame: Frame,
    /// origin of frame coordinates: spin centre, or the part's bbox centre
    pub center: Vec3,
    /// box corners in frame coordinates (m)
    pub lo: Vec3,
    pub hi: Vec3,
    /// background cells along e1, e2, e3
    pub cells: [u32; 3],
    /// background cell size (m)
    pub h0: f64,
    /// part surface level (min, max): cell = h0 / 2^level
    pub surface_level: (u32, u32),
    pub feature_level: u32,
    pub layers: u32,
    pub regions: Vec<Region>,
    pub rotor: Option<Rotor>,
    /// m/s, model axes
    pub u_inf: Vec3,
    pub speed: f64,
    pub nu: f64,
    pub rho: f64,
    pub turbulence: Turbulence,
    /// None: steady (simpleFoam); Some: time-accurate (pimpleFoam), forces averaged over time
    pub transient: Option<Transient>,
    /// steady runs: the iteration cap
    pub max_iterations: u32,
    pub min_iterations: u32,
    pub location_in_mesh: Vec3,
    /// force decomposition axes (model axes, unit)
    pub drag_dir: Vec3,
    pub lift_dir: Vec3,
    pub side_dir: Vec3,
    /// moments are taken about this point
    pub cof_r: Vec3,
    /// projected area normal to the wind (m²) and the reference length (m)
    pub ref_area: f64,
    pub ref_length: f64,
    pub reynolds: f64,
    /// plain-words notes found while planning
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn surface_cell(&self) -> f64 {
        self.h0 / f64::powi(2.0, self.surface_level.0 as i32)
    }
    pub fn background_cells(&self) -> u64 {
        self.cells.iter().map(|&n| n as u64).product()
    }
    pub fn turbulence_name(&self) -> &'static str {
        match self.turbulence {
            Turbulence::Laminar => "laminar",
            Turbulence::KOmegaSst { .. } => "k-omega SST",
            Turbulence::KOmegaSstLm { .. } => "k-omega SST with Langtry-Menter transition",
        }
    }
    /// "steady" or "time-accurate"
    pub fn solver_name(&self) -> &'static str {
        if self.transient.is_some() { "time-accurate (pimpleFoam), forces averaged over time" } else { "steady (simpleFoam)" }
    }
}

const TAU: f64 = std::f64::consts::TAU;

/// `surface` in metres; `model_unit_m` turns the config's model coordinates
/// (`center_mm`) into metres (0.001 when the model is in millimetres).
pub fn plan(surface: &TriMesh, cfg: &WindConfig, model_unit_m: f64) -> anyhow::Result<Plan> {
    if surface.triangles.is_empty() {
        anyhow::bail!("the part has no surface");
    }
    let wind = normalize(cfg.wind_dir);
    if norm(cfg.wind_dir) == 0.0 || !wind.iter().all(|x| x.is_finite()) {
        anyhow::bail!("the wind direction is zero");
    }
    if cfg.speed_m_s < 0.0 || !cfg.speed_m_s.is_finite() {
        anyhow::bail!("the wind speed must be zero or more");
    }
    let spin = cfg.spin.filter(|s| s.rpm != 0.0);
    if cfg.speed_m_s == 0.0 && spin.is_none() {
        anyhow::bail!("with no wind and no spin there is nothing to simulate");
    }
    let nu = cfg.air.kinematic_viscosity;
    let rho = cfg.air.density;
    let mut warnings = vec![];
    let open = open_edges(surface);
    if open > 0 {
        warnings.push(format!(
            "The surface has {open} open or doubled edges; OpenFOAM's mesher needs a closed part, so air may leak inside it."
        ));
    }
    let (blo, bhi) = surface.bounds();
    let bbox_c = scale(add(blo, bhi), 0.5);
    let size = (0..3).map(|k| bhi[k] - blo[k]).fold(0.0, f64::max);

    let (lift_dir, side_dir) = {
        let f = Frame::along(wind);
        (f.e3, f.e2)
    };
    let ref_area = projected_area(surface, wind, 600);
    let speed = cfg.speed_m_s;
    let u_inf = scale(wind, speed);

    // quality → levels
    let q = &cfg.quality;

    if let Some(s) = spin {
        let axis_in = normalize(s.axis);
        if norm(s.axis) == 0.0 {
            anyhow::bail!("the spin axis is zero");
        }
        let center = scale(s.center_mm, model_unit_m);
        let w = s.rpm * TAU / 60.0;
        let omega_vec = scale(axis_in, w);
        let axis = scale(axis_in, w.signum());
        let (r, alo, ahi) = radial_extent(surface, center, axis_in);
        if r <= 0.0 {
            anyhow::bail!("the part has no size around the spin axis");
        }
        let (js, vote) = jet_sign(surface, center, omega_vec, axis_in);
        let jet_dir = scale(axis_in, js);
        if vote < 0.05 {
            warnings.push("The part's shape does not clearly push air one way along the spin axis; the wake refinement may be on the wrong side.".into());
        }
        // proven propeller-study proportions (R = 38.245 mm there)
        let zone_radius = 1.085 * r;
        let margin = 0.098 * r;
        let zone_p1 = add(center, scale(axis_in, alo - margin));
        let zone_p2 = add(center, scale(axis_in, ahi + margin));
        let hover = speed == 0.0;
        let flow = if hover { jet_dir } else { wind };
        let frame = Frame::along(flow);
        let h0 = 0.2615 * r;
        let (lo, hi) = ([-3.92 * r, -5.23 * r, -5.23 * r], [11.77 * r, 5.23 * r, 5.23 * r]);
        let (lo, hi, cells) = snap_box(lo, hi, h0);
        let (surface_level, zone_level, near_level, wake_level, max_it) = match q {
            GridQuality::Draft => ((5, 5), 4, 3, 2, 1500),
            GridQuality::Normal => ((6, 6), 4, 4, 3, 2500),
            GridQuality::Fine => ((6, 7), 4, 4, 3, 3000),
            GridQuality::Custom { cells_across } => {
                let l = level_for(h0, 2.0 * r / (*cells_across).max(8) as f64);
                ((l, l), l.saturating_sub(2).max(2), l.saturating_sub(2).max(2), l.saturating_sub(3).max(1), 2500)
            }
        };
        let rotor = Rotor { axis, omega: w.abs(), center, zone_p1, zone_p2, zone_radius, radius: r, jet_dir, jet_vote: vote, level: zone_level };
        let near_margin = 0.242 * r;
        let mut regions = vec![
            Region::Cylinder {
                name: "nearRotor".into(),
                p1: add(center, scale(axis_in, alo - near_margin)),
                p2: add(center, scale(axis_in, ahi + near_margin)),
                radius: 1.229 * r,
                level: near_level,
            },
            Region::Cylinder {
                name: "wake".into(),
                p1: add(center, scale(jet_dir, -0.784 * r)),
                p2: add(center, scale(jet_dir, 4.18 * r)),
                radius: 1.255 * r,
                level: wake_level,
            },
        ];
        if !hover {
            regions.push(Region::Cylinder {
                name: "windWake".into(),
                p1: add(center, scale(wind, -0.784 * r)),
                p2: add(center, scale(wind, 4.18 * r)),
                radius: 1.255 * r,
                level: wake_level,
            });
        }
        let tip_speed = w.abs() * r;
        let reynolds = (speed.max(tip_speed)) * 2.0 * r / nu;
        let turbulence = if hover {
            Turbulence::KOmegaSst { k: 0.06, omega: 400.0 }
        } else {
            freestream_sst(speed, nu)
        };
        let location_in_mesh = frame.to_model(center, [lo[0] + 1.37 * h0, lo[1] + 1.29 * h0, lo[2] + 1.41 * h0]);
        return Ok(Plan {
            kind: if hover { Kind::SpinHover } else { Kind::SpinWind },
            frame,
            center,
            lo,
            hi,
            cells,
            h0,
            surface_level,
            feature_level: surface_level.1,
            layers: 0,
            regions,
            rotor: Some(rotor),
            u_inf,
            speed,
            nu,
            rho,
            turbulence,
            transient: None,
            max_iterations: max_it,
            min_iterations: 400,
            location_in_mesh,
            drag_dir: wind,
            lift_dir,
            side_dir,
            cof_r: center,
            ref_area,
            ref_length: 2.0 * r,
            reynolds,
            warnings,
        });
    }

    // ---- a part held still in a wind
    let l = size;
    let frame = Frame::along(wind);
    let center = bbox_c;
    let (elo, ehi) = extents_in(surface, &frame, center);
    let h0 = l / 5.0;
    let lo = [elo[0] - 4.0 * l, elo[1] - 4.0 * l, elo[2] - 4.0 * l];
    let hi = [ehi[0] + 10.0 * l, ehi[1] + 4.0 * l, ehi[2] + 4.0 * l];
    let (lo, hi, cells) = snap_box(lo, hi, h0);
    let (surface_level, layers, max_it) = match q {
        GridQuality::Draft => ((4, 5), 0, 1000),
        GridQuality::Normal => ((5, 6), 3, 2000),
        GridQuality::Fine => ((6, 7), 5, 3000),
        GridQuality::Custom { cells_across } => {
            let s = level_for(h0, l / (*cells_across).max(8) as f64);
            ((s, s + 1), 3, 2000)
        }
    };
    // above Re 300 the wall is resolved at every quality (the transition
    // model needs the laminar layer; SST at Re 1000 too: +1.5 % with layers
    // at Draft, +2.4 % without)
    let layers = if speed * l / nu >= RE_STEADY { layers.max(3) } else { layers };
    let n = surface_level.0;
    let regions = vec![
        Region::NearPart { levels: vec![(0.15 * l, n.saturating_sub(1).max(1)), (0.5 * l, n.saturating_sub(2).max(1))] },
        Region::Cylinder {
            name: "wake".into(),
            p1: frame.to_model(center, [elo[0] - 0.2 * l, 0.0, 0.0]),
            p2: frame.to_model(center, [ehi[0] + 2.5 * l, 0.0, 0.0]),
            radius: 0.5 * (ehi[1] - elo[1]).max(ehi[2] - elo[2]) + 0.3 * l,
            level: n.saturating_sub(2).max(1),
        },
        Region::Cylinder {
            name: "farWake".into(),
            p1: frame.to_model(center, [elo[0] - 0.5 * l, 0.0, 0.0]),
            p2: frame.to_model(center, [ehi[0] + 6.0 * l, 0.0, 0.0]),
            radius: 0.5 * (ehi[1] - elo[1]).max(ehi[2] - elo[2]) + 0.6 * l,
            level: n.saturating_sub(3).max(1),
        },
    ];
    let reynolds = speed * l / nu;
    let h_surface = h0 / f64::powi(2.0, surface_level.0 as i32);
    let (turbulence, transient) = wind_method(reynolds, speed, nu, l, h_surface, q, cfg.duration);
    let location_in_mesh = frame.to_model(center, [lo[0] + 1.37 * h0, lo[1] + 1.29 * h0, lo[2] + 1.41 * h0]);
    Ok(Plan {
        kind: Kind::Wind,
        frame,
        center,
        lo,
        hi,
        cells,
        h0,
        surface_level,
        feature_level: surface_level.1,
        layers,
        regions,
        rotor: None,
        u_inf,
        speed,
        nu,
        rho,
        turbulence,
        transient,
        max_iterations: max_it,
        min_iterations: 300,
        location_in_mesh,
        drag_dir: wind,
        lift_dir,
        side_dir,
        cof_r: center,
        ref_area,
        ref_length: l,
        reynolds,
        warnings,
    })
}

/// Below this Reynolds number (U L / ν) the flow round a still part is
/// steady and laminar (a sphere's wake starts to shed above Re ≈ 270).
pub const RE_STEADY: f64 = 300.0;

/// Above this Reynolds number a still part gets the Langtry–Menter
/// transition model on top of k-ω SST. Plain SST treats the boundary layer
/// as turbulent from the nose, so on a rounded part it separates too late
/// and drag reads low (sphere: −14 % at Re 2·10⁴, −39 % at 10⁵); with the
/// transition model the layer stays laminar to separation, as it does on a
/// small part in calm air. Below it plain SST is as good or better (sphere
/// Re 1000: +1.5 % vs +3.6 %) and both agree at Re 10⁴ (−1.8 % / +0.7 %).
pub const RE_TRANSITION: f64 = 5000.0;

/// Flow passes (L / U) a time-accurate run lets go by before it starts
/// averaging, the shortest averaging time, and the most it runs, per quality.
pub fn transient_passes(q: &GridQuality) -> (f64, f64, f64) {
    match q {
        GridQuality::Draft => (8.0, 8.0, 24.0),
        GridQuality::Fine => (10.0, 15.0, 40.0),
        _ => (10.0, 10.0, 30.0),
    }
}

/// How a still part in a wind is solved, by its Reynolds number:
///
/// | Re | model | solver |
/// |---|---|---|
/// | < 300 | laminar | steady |
/// | 300 – 5000 | k-ω SST | steady |
/// | ≥ 5000 | k-ω SST + Langtry–Menter transition | steady |
///
/// Steady, because time-accurate runs measured no better on this machine
/// (see the crate docs): unsteady RANS of a sphere settles to the steady
/// answer; LES and SST-IDDES on meshes that run here in under an hour read
/// 9–20 % high on a sphere and 31 % low on a flat plate.
///
/// `duration` (flow passes) asks for a time-accurate run instead: unsteady
/// RANS with the same model, forces averaged once the start-up is over.
/// It shows how much the force swings as the wake sheds, at several times
/// the cost (cylinder at Re 2·10⁴: Cd within the steady answer's 4 %).
///
/// `h_surface` is the cell size on the part: it sets the time step.
pub fn wind_method(re: f64, speed: f64, nu: f64, l: f64, h_surface: f64, q: &GridQuality, duration: Option<f64>) -> (Turbulence, Option<Transient>) {
    if re < RE_STEADY {
        return (Turbulence::Laminar, None);
    }
    let sst = freestream_sst(speed, nu);
    let turbulence = if re >= RE_TRANSITION {
        let (k, omega) = sst.k_omega().unwrap_or((0.0, 0.0));
        Turbulence::KOmegaSstLm { k, omega, re_theta: re_theta_t(FREESTREAM_TU) }
    } else {
        sst
    };
    let Some(passes) = duration.filter(|d| d.is_finite() && *d > 0.0) else {
        return (turbulence, None);
    };
    let flow_time = l / speed;
    let (skip, min_avg, _) = transient_passes(q);
    // a short run averages over its second half
    let skip = skip.min(0.5 * passes);
    // unsteady RANS: Courant 4 with two outer correctors per step
    let max_co = 4.0;
    // measured: the largest Courant number sits where the air speeds up past
    // the part, about 2.3 × the wind speed through a surface cell
    let dt_estimate = max_co * h_surface / (2.3 * speed);
    let t = Transient {
        init_iterations: 300,
        flow_time,
        end_time: passes * flow_time,
        average_from: skip * flow_time,
        min_average: min_avg.min(passes - skip) * flow_time,
        max_co,
        dt0: 0.02 * dt_estimate,
        dt_estimate,
        outer_correctors: 2,
    };
    (turbulence, Some(t))
}

/// Free-stream turbulence intensity at the inlet (calm air, a good tunnel).
pub const FREESTREAM_TU: f64 = 0.005;

/// Langtry–Menter transition-onset momentum-thickness Reynolds number in
/// the free stream, for turbulence intensity `tu` (fraction), zero pressure
/// gradient (Langtry & Menter 2009, eq. for Tu ≤ 1.3 %).
pub fn re_theta_t(tu: f64) -> f64 {
    let t = (tu * 100.0).max(0.027);
    if t <= 1.3 { 1173.51 - 589.428 * t + 0.2196 / (t * t) } else { 331.5 * (t - 0.5658).powf(-0.671) }
}

/// Low free-stream turbulence: intensity 0.5 %, eddy viscosity = ν.
fn freestream_sst(speed: f64, nu: f64) -> Turbulence {
    let k = (1.5 * (FREESTREAM_TU * speed).powi(2)).max(1e-10);
    Turbulence::KOmegaSst { k, omega: k / nu }
}

/// Snaps the box to whole background cells of size `h0` with a grid node
/// at the frame origin (the spin centre or the part's centre), so the mesh
/// sits the same way round the part whatever its size: each face moves to
/// the nearest whole number of cells (at least one) from the origin.
fn snap_box(lo: Vec3, hi: Vec3, h0: f64) -> (Vec3, Vec3, [u32; 3]) {
    let mut cells = [0u32; 3];
    let mut lo2 = lo;
    let mut hi2 = hi;
    for k in 0..3 {
        let nl = (lo[k] / h0).round().min(-1.0);
        let nh = (hi[k] / h0).round().max(1.0);
        lo2[k] = nl * h0;
        hi2[k] = nh * h0;
        cells[k] = (nh - nl) as u32;
    }
    (lo2, hi2, cells)
}

/// The refinement level whose cells are closest to `target` (m).
fn level_for(h0: f64, target: f64) -> u32 {
    (h0 / target).log2().round().clamp(1.0, 10.0) as u32
}

/// A rough cell count, for estimates before running: background plus the
/// shell of surface cells (area / h²) times the cells between levels, plus
/// the refinement regions' volumes.
pub fn estimate_cells(p: &Plan, surface: &TriMesh) -> u64 {
    let area: f64 = (0..surface.triangles.len()).map(|t| surface.triangle_area(t)).sum();
    let mut total = p.background_cells() as f64;
    let hs = p.surface_cell();
    // surface shell: ~4 cells deep at the surface level, then 3 per level outwards
    total += 4.0 * area / (hs * hs);
    for reg in &p.regions {
        if let Region::Cylinder { p1, p2, radius, level, .. } = reg {
            let h = p.h0 / f64::powi(2.0, *level as i32);
            let v = std::f64::consts::PI * radius * radius * norm(sub(*p2, *p1));
            total += v / (h * h * h) * 0.8;
        }
    }
    if let Some(r) = &p.rotor {
        let h = p.h0 / f64::powi(2.0, r.level as i32);
        let v = std::f64::consts::PI * r.zone_radius.powi(2) * norm(sub(r.zone_p2, r.zone_p1));
        total += 0.3 * v / (h * h * h);
    }
    total as u64
}
