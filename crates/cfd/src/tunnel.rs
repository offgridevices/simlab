//! The wind tunnel: from a `WindConfig` and a surface in metres to a lattice,
//! a run with convergence checks, and results in SI units.
//!
//! Frames. The lattice x axis is the wind direction (still air with spin:
//! the spin axis), lattice z is the model's +z made perpendicular to it (if
//! the wind is nearly vertical, model +x is used), lattice y = z × x.
//! Lift is along lattice z, side force along lattice y, drag along x.
//!
//! Domain (defaults, in units of the frontal size D_f = the larger of the
//! part's two extents across the wind): 2.5 D_f upstream, 5 D_f downstream,
//! 1.5 D_f on every side (a sphere blocks 4.9 % of the cross-section). The
//! inlet holds the free stream's total pressure with the velocity taken from
//! inside (so the air ahead can slow down as it would in open air, instead of
//! being forced to the wind speed); the sides and the outlet are open
//! (far-field pressure, velocity from inside), which lets the displaced air
//! escape. With these, the sphere's drag at Re = 100 moved by less than
//! 0.4 % when any margin was doubled, so no blockage correction is applied;
//! the blockage ratio is reported.
//! Still air with spin: 2.5 rotor diameters above and below the rotor,
//! 1 diameter of margin around the disk, open on every face.
//! Viscosity sponges plus absorbing layers (density pulled to the far field)
//! line the faces, thicker at the outlet, and damp eddies and sound before
//! they reach the boundaries.
//!
//! Units: the cell size is the part's largest size (spinning: the swept
//! disk) over the quality's cells across; the time step keeps the fastest
//! speed (wind + blade tip) at 0.075 (wind) or 0.10 (spinning) lattice units,
//! i.e. lattice Mach 0.13 / 0.17, and is shortened further if the viscosity
//! would push the relaxation time past 1.
//!
//! Averaging and convergence: the first 8 flow passes (3 revolutions in
//! still air; at most 35 % of the run) are skipped. Forces, surface pressure
//! and slices are then averaged over a window; the run stops when the mean
//! drag (spinning: thrust and torque) of the window's two halves agree to
//! `tolerance` (|m2 - m1| / (|m1| + |m2|), default 0.5 %) and the
//! statistical error of the window mean (from 8 block means) is below the
//! same tolerance, after at least 10 passes (2 revolutions). If the halves
//! still differ by more than 3x the tolerance and by far more than their
//! noise (a real drift), the window restarts. Otherwise the run ends at
//! the duration and `converged` is false.

use crate::lattice::{FaceBc, Geometry, IDENTITY, Lattice, LatticeSetup, Probes, StepIn, Storage};
use crate::mlattice::{LevelBox, MGeometry, MProbes, MultiLattice, MultiSetup};
use crate::shader::SliceDef;
use crate::voxel::{Surface, voxelize_spin, voxelize_static};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use simlab_core::{Cancelled, GridQuality, Progress, TriMesh, Vec3, WindConfig, cross, dot, norm, normalize, scale, sub};
use std::f64::consts::PI;
use std::time::Instant;

/// Cells across the part's largest size for each quality (on the finest
/// grid level near a still part). Draft (64) uses two levels, Normal (128)
/// and Fine (192) three. Spinning propellers are not converged at any of
/// these (a rough preview only).
pub fn cells_across(q: &GridQuality) -> u32 {
    match q {
        GridQuality::Draft => 64,
        GridQuality::Normal => 128,
        GridQuality::Fine => 192,
        GridQuality::Custom { cells_across } => (*cells_across).clamp(8, 4096),
    }
}

/// Default duration: flow passes over the part (wind) or revolutions (still air).
pub fn default_duration(q: &GridQuality, still_spin: bool) -> f64 {
    let (d, n, f) = if still_spin { (8.0, 12.0, 16.0) } else { (20.0, 40.0, 60.0) };
    match q {
        GridQuality::Draft => d,
        GridQuality::Normal | GridQuality::Custom { .. } => n,
        GridQuality::Fine => f,
    }
}

enum Geo {
    Old(Geometry),
    New(MGeometry),
}

/// The solver behind a run: one uniform D3Q19 grid, or the refined D3Q27 one.
enum Engine {
    One(Lattice),
    Multi(MultiLattice),
}
impl Engine {
    fn run(&mut self, s: &[StepIn]) -> Result<Vec<[f64; 6]>> {
        match self {
            Engine::One(l) => l.run(s),
            Engine::Multi(l) => l.run(s),
        }
    }
    fn reset_samples(&mut self) {
        match self {
            Engine::One(l) => l.reset_samples(),
            Engine::Multi(l) => l.reset_samples(),
        }
    }
    fn vertex_means(&self) -> Result<Vec<f32>> {
        match self {
            Engine::One(l) => l.vertex_means(),
            Engine::Multi(l) => l.vertex_means(),
        }
    }
    fn slice_means(&self) -> Result<Vec<[f32; 4]>> {
        match self {
            Engine::One(l) => l.slice_means(),
            Engine::Multi(l) => l.slice_means(),
        }
    }
    fn sub_steps(&self) -> u64 {
        match self {
            Engine::One(_) => 1,
            Engine::Multi(l) => l.sub_steps() as u64,
        }
    }
    fn storage(&self) -> Storage {
        match self {
            Engine::One(l) => l.setup.storage,
            Engine::Multi(l) => l.setup.storage,
        }
    }
    fn memory_bytes(&self) -> u64 {
        match self {
            Engine::One(l) => l.memory_bytes,
            Engine::Multi(l) => l.memory_bytes,
        }
    }
}

/// Most cells one lattice can hold whatever the memory: every population
/// direction must fit half a GPU buffer (buffers stay under 4 GiB and at
/// most 12 of them hold the 19 directions).
pub fn max_cells_layout(s: Storage) -> u64 {
    ((1u64 << 32) - 256) / 2 / s.pop_bytes()
}

/// Measured on an M4 Max (40-core GPU); used for time estimates.
pub const MLUPS_FP32: f64 = 2800.0;
pub const MLUPS_FP16: f64 = 4500.0;
/// The refined D3Q27 solver (cell updates per second, all levels), M4 Max.
pub const MLUPS_REFINED: f64 = 1400.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Margins {
    /// wind: in frontal sizes; still air: in rotor diameters (upstream = above/below)
    pub upstream: f64,
    pub downstream: f64,
    pub side: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolverOptions {
    /// overrides the quality's cells across the largest size
    pub cells_across: Option<u32>,
    /// None: FP32 when it fits `memory_budget`, else FP16
    pub storage: Option<Storage>,
    pub memory_budget: Option<u64>,
    /// highest lattice speed (wind; Mach = this × √3)
    pub max_lattice_speed: f64,
    /// highest lattice speed of a spinning part's tip plus the wind
    pub max_tip_speed: f64,
    /// highest relaxation time; lower viscosity flows slow the lattice instead
    pub max_tau: f64,
    pub wind_margins: Margins,
    pub still_margins: Margins,
    pub smagorinsky: f64,
    /// convergence: |mean(second half) - mean(first half)| / (|first| + |second|)
    /// of the averaging window must fall below this (0.005: the halves agree
    /// to about 1 %)
    pub tolerance: f64,
    /// revolutions over which a spinning part speeds up from rest
    pub spin_ramp_revs: f64,
    /// how far from the surface pressure is read, in cells
    pub vertex_offset: f64,
    pub side_bc: FaceBcChoice,
    /// absorbing-layer strength at the faces (per step)
    pub sponge_sigma: f64,
    /// sponge thickness: inlet, outlet, sides (frontal sizes or rotor diameters)
    pub sponge_widths: [f64; 3],
    /// absorbing layers also pull the velocity toward the free stream (wind only)
    pub absorb_velocity: bool,
    /// inlet holds the free stream's total pressure rather than its velocity
    pub total_pressure_inlet: bool,
    /// Spalding wall model in the first fluid cell (for boundary layers thinner than a cell)
    pub wall_model: bool,
    /// still parts: the refined D3Q27 cumulant solver (false: one uniform D3Q19 grid)
    pub refine: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FaceBcChoice {
    Open,
    Velocity,
}

impl Default for SolverOptions {
    fn default() -> Self {
        SolverOptions {
            cells_across: None,
            storage: None,
            memory_budget: None,
            max_lattice_speed: 0.075,
            max_tip_speed: 0.10,
            max_tau: 1.0,
            wind_margins: Margins { upstream: 2.5, downstream: 5.0, side: 1.5 },
            still_margins: Margins { upstream: 2.5, downstream: 2.5, side: 1.0 },
            smagorinsky: 0.10,
            tolerance: 0.005,
            spin_ramp_revs: 0.5,
            vertex_offset: 1.0,
            side_bc: FaceBcChoice::Open,
            sponge_sigma: 0.1,
            sponge_widths: [0.4, 1.0, 0.4],
            absorb_velocity: false,
            total_pressure_inlet: true,
            // off by default: on a NACA 0012 at Re 1e5 it lifted Cl only from
            // 0.18 to 0.21 (reference 0.44), and it destabilised a 3-inch prop
            // at 128 cells after ~3 revolutions
            wall_model: false,
            refine: true,
        }
    }
}

/// One flow slice, as the API's `Slice` (plus the data).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlowSlice {
    pub name: String,
    pub label: String,
    /// corner of the first pixel, model mm; u_mm/v_mm span the whole slice
    pub origin_mm: Vec3,
    pub u_mm: Vec3,
    pub v_mm: Vec3,
    pub nu: u32,
    pub nv: u32,
    pub fields: Vec<SliceField>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SliceField {
    pub name: String,
    pub label: String,
    pub unit: String,
    pub min: f32,
    pub max: f32,
    /// row-major in v then u (index = iv * nu + iu); NaN inside the part
    #[serde(skip)]
    pub data: Vec<f32>,
}

/// `WindSummary` of docs/API.md, plus run details.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindSummary {
    pub kind: String,
    pub headline: String,
    pub force_n: Vec3,
    /// about the part's bounding-box centre (spinning: the spin centre)
    pub moment_nm: Vec3,
    pub drag_n: f64,
    pub lift_n: f64,
    pub side_n: f64,
    pub cd: Option<f64>,
    pub ref_area_mm2: f64,
    /// force along the spin axis (signed: + pushes the part toward +axis)
    pub thrust_n: Option<f64>,
    /// aerodynamic moment about the spin axis (signed, about +axis); a
    /// driven propeller has it opposite to its rotation
    pub torque_nm: Option<f64>,
    /// shaft power the motor supplies (positive for a driven propeller)
    pub power_w: Option<f64>,
    pub reynolds: f64,
    pub cells: u64,
    pub steps: u64,
    pub mlups: f64,
    pub seconds: f64,
    pub converged: bool,
    pub warnings: Vec<String>,
    // details
    pub grid: [u32; 3],
    pub cells_across: u32,
    pub dx_mm: f64,
    pub dt_s: f64,
    pub mach_lattice: f64,
    pub tau: f64,
    pub blockage: f64,
    pub storage: Storage,
    pub memory_bytes: u64,
    /// the averaging window, in flow passes or revolutions
    pub averaged_over: f64,
    pub simulated: f64,
    /// what `simulated` and `averaged_over` count: "passes" or "revolutions"
    pub time_unit: String,
    /// dynamic pressure used for Cp
    pub q_ref_pa: f64,
}

#[derive(Clone, Debug)]
pub struct WindResult {
    pub summary: WindSummary,
    /// per input vertex, Pa relative to the far field (time-averaged); NaN where unsampled
    pub surface_pressure_pa: Vec<f32>,
    pub surface_cp: Vec<f32>,
    pub slices: Vec<FlowSlice>,
    /// (time in passes or revolutions, force N [x,y,z], moment N·m), model axes, one per batch
    pub history: Vec<(f64, Vec3, Vec3)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Estimate {
    pub fits: bool,
    pub cells: u64,
    pub grid: [u32; 3],
    pub cells_across: u32,
    pub memory_bytes: u64,
    pub budget_bytes: u64,
    pub storage: Storage,
    pub steps: u64,
    pub seconds: f64,
    pub note: String,
}

/// Everything derived from the config before touching the GPU.
#[derive(Clone, Debug)]
struct Plan {
    /// rows: lattice axes in model coordinates
    r: [Vec3; 3],
    /// model point at lattice-frame origin of the part (bbox centre / spin centre)
    pref: Vec3,
    dx: f64,
    dt: f64,
    dims: [u32; 3],
    /// lattice coordinates of `pref`
    center: [f64; 3],
    u_lat: f64,
    omega_lat: [f64; 3],
    nu_lat: f64,
    spin: bool,
    still: bool,
    cells_across: u32,
    d_front: f64,
    frontal_area: f64,
    steps_unit: f64,
    duration: f64,
    storage: Storage,
    memory: u64,
    blockage: f64,
    sponge: [u32; 6],
    faces: [FaceBc; 6],
    rotor_radius: f64,
    axis_lat: Vec3,
    axis_model: Vec3,
    box_lo: [i32; 3],
    box_n: [u32; 3],
    est_records: u64,
    /// refined grid (still parts): coarsest dims, nested boxes, finest-level
    /// origin in global finest coordinates, cells per level
    multi: Option<MultiPlan>,
    /// cell updates per finest time step (all levels)
    updates_per_step: f64,
}

#[derive(Clone, Debug)]
struct MultiPlan {
    dims0: [u32; 3],
    boxes: Vec<LevelBox>,
    org_f: [f64; 3],
    cells: Vec<u64>,
}

/// Nested refinement around the part, beyond its bounding box, in frontal
/// sizes: (upstream, downstream, sideways) for levels 1, 2, 3.
const REFINE_EXT: [(f64, f64, f64); 3] = [(1.2, 3.0, 1.0), (0.8, 1.6, 0.75), (0.65, 1.0, 0.65)];

/// Levels for a still part: the coarsest level keeps at least 24 cells
/// across the part's largest size; at most 4 levels.
fn still_levels(cells_across: u32) -> usize {
    let mut nl = 1;
    while nl < 4 && cells_across as f64 / (1u64 << nl) as f64 >= 24.0 {
        nl += 1;
    }
    nl
}

fn mat_vec(r: &[Vec3; 3], v: Vec3) -> Vec3 {
    [dot(r[0], v), dot(r[1], v), dot(r[2], v)]
}
fn mat_t_vec(r: &[Vec3; 3], v: Vec3) -> Vec3 {
    [r[0][0] * v[0] + r[1][0] * v[1] + r[2][0] * v[2], r[0][1] * v[0] + r[1][1] * v[1] + r[2][1] * v[2], r[0][2] * v[0] + r[1][2] * v[1] + r[2][2] * v[2]]
}

/// Rotation matrix (columns = rotated basis) about unit `a` by `t`.
pub fn rotation(a: Vec3, t: f64) -> [[f64; 3]; 3] {
    let (c, s) = (t.cos(), t.sin());
    let k = 1.0 - c;
    [
        [c + a[0] * a[0] * k, a[0] * a[1] * k - a[2] * s, a[0] * a[2] * k + a[1] * s],
        [a[1] * a[0] * k + a[2] * s, c + a[1] * a[1] * k, a[1] * a[2] * k - a[0] * s],
        [a[2] * a[0] * k - a[1] * s, a[2] * a[1] * k + a[0] * s, c + a[2] * a[2] * k],
    ]
}

fn frame(main: Vec3) -> [Vec3; 3] {
    let ex = normalize(main);
    let up = if ex[2].abs() > 0.95 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
    let ez = normalize(sub(up, scale(ex, dot(up, ex))));
    let ey = cross(ez, ex);
    [ex, ey, ez]
}

/// Projected area of the surface on the plane across `r[0]` (rasterized).
fn frontal_area(m: &TriMesh, r: &[Vec3; 3]) -> f64 {
    let pts: Vec<[f64; 2]> = m.positions.iter().map(|p| [dot(r[1], *p), dot(r[2], *p)]).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if size <= 0.0 {
        return 0.0;
    }
    let res = 1024usize;
    let h = size / res as f64;
    let (nu, nv) = (((hi[0] - lo[0]) / h).ceil() as usize + 1, ((hi[1] - lo[1]) / h).ceil() as usize + 1);
    let mut img = vec![false; nu * nv];
    for t in &m.triangles {
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        let i0 = ((a[0].min(b[0]).min(c[0]) - lo[0]) / h).floor().max(0.0) as usize;
        let i1 = (((a[0].max(b[0]).max(c[0]) - lo[0]) / h).ceil() as usize).min(nu - 1);
        let j0 = ((a[1].min(b[1]).min(c[1]) - lo[1]) / h).floor().max(0.0) as usize;
        let j1 = (((a[1].max(b[1]).max(c[1]) - lo[1]) / h).ceil() as usize).min(nv - 1);
        let e = |p: [f64; 2], q: [f64; 2], x: [f64; 2]| (q[0] - p[0]) * (x[1] - p[1]) - (q[1] - p[1]) * (x[0] - p[0]);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let x = [lo[0] + (i as f64 + 0.5) * h, lo[1] + (j as f64 + 0.5) * h];
                let (e0, e1, e2) = (e(a, b, x), e(b, c, x), e(c, a, x));
                if (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0) {
                    img[j * nu + i] = true;
                }
            }
        }
    }
    img.iter().filter(|&&v| v).count() as f64 * h * h
}

fn plan(cfg: &WindConfig, m: &TriMesh, unit_m: f64, opt: &SolverOptions, budget: Option<u64>) -> Result<Plan> {
    if m.triangles.is_empty() {
        bail!("the part has no surface");
    }
    let speed = cfg.speed_m_s;
    if !(speed >= 0.0) || !speed.is_finite() {
        bail!("wind speed must be zero or positive");
    }
    let spin = cfg.spin.filter(|s| s.rpm != 0.0 && norm(s.axis) > 0.0);
    if speed == 0.0 && spin.is_none() {
        bail!("still air needs a spinning part; set a wind speed or a spin");
    }
    if speed > 0.0 && norm(cfg.wind_dir) == 0.0 {
        bail!("the wind direction is zero");
    }
    let still = speed == 0.0;
    let r = if still { frame(spin.unwrap().axis) } else { frame(cfg.wind_dir) };
    let (blo, bhi) = m.bounds();
    let pref = match spin {
        Some(s) => scale(s.center_mm, unit_m),
        None => scale(simlab_core::add(blo, bhi), 0.5),
    };
    // lattice-frame extents relative to pref (metres)
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut rotor_radius: f64 = 0.0;
    let mut alo = f64::INFINITY;
    let mut ahi = f64::NEG_INFINITY;
    let axis_model = spin.map(|s| normalize(s.axis)).unwrap_or([1.0, 0.0, 0.0]);
    for p in &m.positions {
        let v = sub(*p, pref);
        let q = mat_vec(&r, v);
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
        let a = dot(v, axis_model);
        rotor_radius = rotor_radius.max((dot(v, v) - a * a).max(0.0).sqrt());
        alo = alo.min(a);
        ahi = ahi.max(a);
    }
    let axis_lat = mat_vec(&r, axis_model);
    if spin.is_some() {
        // swept cylinder's box
        for k in 0..3 {
            let e = rotor_radius * (1.0 - axis_lat[k] * axis_lat[k]).max(0.0).sqrt();
            let (p0, p1) = (alo * axis_lat[k], ahi * axis_lat[k]);
            lo[k] = p0.min(p1) - e;
            hi[k] = p0.max(p1) + e;
        }
    }
    let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let l_ref = ext[0].max(ext[1]).max(ext[2]);
    if l_ref <= 0.0 {
        bail!("the part has no size");
    }
    let n_across = opt.cells_across.unwrap_or_else(|| cells_across(&cfg.quality));
    let dx = l_ref / n_across as f64;
    let d_front = if still { 2.0 * rotor_radius } else { ext[1].max(ext[2]) }.max(l_ref * 1e-3);
    let mg = if still { &opt.still_margins } else { &opt.wind_margins };
    let dom_lo = [lo[0] - mg.upstream * d_front, lo[1] - mg.side * d_front, lo[2] - mg.side * d_front];
    let dom_hi = [hi[0] + mg.downstream * d_front, hi[1] + mg.side * d_front, hi[2] + mg.side * d_front];
    let mut dims = [0u32; 3];
    for k in 0..3 {
        dims[k] = ((dom_hi[k] - dom_lo[k]) / dx).ceil() as u32 + 1;
    }
    let dims_uniform = dims;
    let mut center = [-dom_lo[0] / dx, -dom_lo[1] / dx, -dom_lo[2] / dx];
    // still parts: nested refinement (the finest level holds the part)
    let mut multi: Option<MultiPlan> = None;
    if spin.is_none() && opt.refine {
        let nl = still_levels(n_across);
        if nl > 1 {
            let s0 = (1u64 << (nl - 1)) as f64;
            let dx0 = dx * s0;
            let mut dims0 = [0u32; 3];
            for k in 0..3 {
                dims0[k] = ((dom_hi[k] - dom_lo[k]) / dx0).ceil() as u32 + 1;
                dims[k] = ((dims0[k] - 1) as f64 * s0) as u32 + 1;
            }
            center = [-dom_lo[0] / dx, -dom_lo[1] / dx, -dom_lo[2] / dx];
            let mut lo_c = [(center[0] + lo[0] / dx) / s0, (center[1] + lo[1] / dx) / s0, (center[2] + lo[2] / dx) / s0];
            let mut hi_c = [(center[0] + hi[0] / dx) / s0, (center[1] + hi[1] / dx) / s0, (center[2] + hi[2] / dx) / s0];
            let mut pd = dims0;
            let mut boxes = Vec::new();
            let mut org = [0f64; 3];
            let mut cells = vec![dims0.iter().map(|&d| d as u64).product::<u64>()];
            for l in 1..nl {
                let sc = (1u64 << (nl - l)) as f64; // finest cells per level (l-1) cell
                let df = d_front / dx / sc;
                let (eu, ed, es) = REFINE_EXT[l - 1];
                let e_lo = [eu * df, es * df, es * df];
                let e_hi = [ed * df, es * df, es * df];
                let mut blo = [0i32; 3];
                let mut bn = [0u32; 3];
                for k in 0..3 {
                    let a = ((lo_c[k] - e_lo[k]).floor() as i32).max(3);
                    let b = ((hi_c[k] + e_hi[k]).ceil() as i32).min(pd[k] as i32 - 4);
                    blo[k] = a;
                    bn[k] = (b - a + 1).max(8) as u32;
                }
                for k in 0..3 {
                    org[k] += blo[k] as f64 * sc;
                    lo_c[k] = 2.0 * (lo_c[k] - blo[k] as f64);
                    hi_c[k] = 2.0 * (hi_c[k] - blo[k] as f64);
                    pd[k] = 2 * (bn[k] - 1) + 1;
                }
                cells.push(pd.iter().map(|&d| d as u64).product::<u64>());
                boxes.push(LevelBox { lo: blo, n: bn });
            }
            multi = Some(MultiPlan { dims0, boxes, org_f: org, cells });
        }
    }

    // time step from the Mach limit, then the viscosity limit
    let omega = spin.map(|s| s.rpm * 2.0 * PI / 60.0).unwrap_or(0.0);
    let tip = omega.abs() * rotor_radius;
    let umax_phys = speed + tip;
    let ulat_max = if spin.is_some() { opt.max_tip_speed } else { opt.max_lattice_speed };
    let mut dt = ulat_max * dx / umax_phys;
    let nu = cfg.air.kinematic_viscosity;
    let nu_max = (opt.max_tau - 0.5) / 3.0;
    if nu * dt / (dx * dx) > nu_max {
        dt = nu_max * dx * dx / nu;
    }
    let nu_lat = nu * dt / (dx * dx);
    let u_lat = speed * dt / dx;
    let omega_lat = scale(axis_lat, omega * dt);
    let steps_unit = if still { 2.0 * PI / (omega.abs() * dt) } else { l_ref / speed / dt };
    let duration = cfg.duration.unwrap_or_else(|| default_duration(&cfg.quality, still)).max(1.0);

    let n = dims.iter().map(|&d| d as u64).product::<u64>();
    // swept box for spinning parts (cells)
    let (box_lo, box_n) = if spin.is_some() {
        let mut bl = [0i32; 3];
        let mut bn = [0u32; 3];
        for k in 0..3 {
            let a = (center[k] + lo[k] / dx - 3.0).floor() as i32;
            let b = (center[k] + hi[k] / dx + 3.0).ceil() as i32;
            bl[k] = a.max(1);
            bn[k] = (b.min(dims[k] as i32 - 2) - bl[k] + 1).max(1) as u32;
        }
        (bl, bn)
    } else {
        ([0; 3], [0; 3])
    };
    let area_m2: f64 = (0..m.triangles.len()).map(|t| m.triangle_area(t)).sum();
    let est_records = if spin.is_some() { box_n.iter().map(|&v| v as u64).product() } else { (2.0 * area_m2 / (dx * dx)) as u64 + 1000 };
    let faces_cells = 2 * (dims[0] as u64 * dims[1] as u64 + dims[1] as u64 * dims[2] as u64 + dims[0] as u64 * dims[2] as u64);
    let mem_for = |s: Storage| n * s.bytes_per_cell() + est_records * crate::shader::RS as u64 * 4 + faces_cells * 44 + 64 * 1024 * 1024;
    let budget = budget.or(opt.memory_budget);
    let mem_for = |s: Storage| -> u64 {
        match &multi {
            Some(mp) => {
                let pops: u64 = mp.cells.iter().map(|&c| c * (2 * 27 * s.pop_bytes() + 4 + 8)).sum();
                pops + est_records * crate::mshader::RS as u64 * 4 * 2 + 64 * 1024 * 1024
            }
            None => mem_for(s),
        }
    };
    let n = match &multi {
        Some(mp) => mp.cells.iter().copied().max().unwrap_or(n),
        None => n,
    };
    let fits_layout = |s: Storage| match &multi {
        // A and B copies, at most 6 buffers per level for 27 directions
        Some(_) => n * 2 * s.pop_bytes() * 5 <= (1u64 << 32) - 256,
        None => n <= max_cells_layout(s),
    };
    let storage = match opt.storage {
        Some(s) => s,
        None => match budget {
            Some(b) if mem_for(Storage::Fp32) > b || !fits_layout(Storage::Fp32) => Storage::Fp16,
            _ if !fits_layout(Storage::Fp32) => Storage::Fp16,
            _ => Storage::Fp32,
        },
    };
    let layout_ok = fits_layout(storage);
    let memory = if layout_ok { mem_for(storage) } else { u64::MAX / 4 };
    let frontal = if still { PI * rotor_radius * rotor_radius } else { frontal_area(m, &r) };
    let blockage = frontal / ((dom_hi[1] - dom_lo[1]) * (dom_hi[2] - dom_lo[2]));
    // sponge widths in cells of the coarsest level
    let dxs = match &multi {
        Some(mp) => dx * ((dims[0] - 1) as f64 / (mp.dims0[0] - 1) as f64),
        None => dx,
    };
    let cl = |x: f64, lo: u32| -> u32 { (x / dxs).round().max(lo as f64) as u32 };
    let _ = dims_uniform;
    let (sponge, faces) = if still {
        let s = cl(opt.sponge_widths[2] * d_front, 8);
        ([s; 6], [FaceBc::Open; 6])
    } else {
        let side = if opt.side_bc == FaceBcChoice::Velocity { FaceBc::Velocity } else { FaceBc::Open };
        let s_side = cl(opt.sponge_widths[2] * d_front, 6);
        ([cl(opt.sponge_widths[0] * d_front, 6), cl(opt.sponge_widths[1] * d_front, 8), s_side, s_side, s_side, s_side], [FaceBc::Velocity, FaceBc::Open, side, side, side, side])
    };
    Ok(Plan {
        r,
        pref,
        dx,
        dt,
        dims,
        center,
        u_lat,
        omega_lat,
        nu_lat,
        spin: spin.is_some(),
        still,
        cells_across: n_across,
        d_front,
        frontal_area: frontal,
        steps_unit,
        duration,
        storage,
        memory,
        blockage,
        sponge,
        faces,
        rotor_radius,
        axis_lat,
        axis_model,
        box_lo,
        box_n,
        est_records,
        updates_per_step: match &multi {
            Some(mp) => {
                let nl = mp.cells.len();
                mp.cells.iter().enumerate().map(|(l, &c)| c as f64 * (1u64 << l) as f64).sum::<f64>() / (1u64 << (nl - 1)) as f64
            }
            None => dims.iter().map(|&d| d as f64).product(),
        },
        multi,
    })
}

/// Cells, memory and time for a study before running it. `surface` is in
/// metres; `model_unit_m` converts the spin centre (model mm) to metres.
pub fn estimate(cfg: &WindConfig, surface: &TriMesh, model_unit_m: f64, budget_bytes: u64) -> Result<Estimate> {
    estimate_with(cfg, surface, model_unit_m, budget_bytes, &SolverOptions::default())
}

pub fn estimate_with(cfg: &WindConfig, surface: &TriMesh, model_unit_m: f64, budget_bytes: u64, opt: &SolverOptions) -> Result<Estimate> {
    let p = plan(cfg, surface, model_unit_m, opt, Some(budget_bytes))?;
    let cells = match &p.multi {
        Some(mp) => mp.cells.iter().sum(),
        None => p.dims.iter().map(|&d| d as u64).product::<u64>(),
    };
    let steps = (p.duration * p.steps_unit).ceil() as u64;
    let mlups = match (&p.multi, p.storage) {
        (Some(_), _) => MLUPS_REFINED,
        (None, Storage::Fp16) => MLUPS_FP16,
        (None, Storage::Fp32) => MLUPS_FP32,
    };
    let seconds = steps as f64 * p.updates_per_step / (mlups * 1e6) * if p.spin { 1.15 } else { 1.0 } + 2.0 + p.est_records as f64 * 2e-6;
    let fits = p.memory <= budget_bytes;
    let note = format!(
        "{} cells across the part ({:.2} mm cells{}), {} x {} x {} grid, {} storage, {:.0} {}",
        p.cells_across,
        p.dx * 1e3,
        match &p.multi {
            Some(mp) => format!(" near it, {} grid levels", mp.cells.len()),
            None => String::new(),
        },
        p.dims[0],
        p.dims[1],
        p.dims[2],
        if p.storage == Storage::Fp16 { "FP16" } else { "FP32" },
        p.duration,
        if p.still { "revolutions" } else { "flow passes" }
    );
    let _ = mlups;
    Ok(Estimate { fits, cells, grid: p.dims, cells_across: p.cells_across, memory_bytes: p.memory, budget_bytes, storage: p.storage, steps, seconds, note })
}

/// The finest cells-across that fits the budget (FP16 storage allowed).
pub fn finest_fit(cfg: &WindConfig, surface: &TriMesh, model_unit_m: f64, budget_bytes: u64) -> Result<u32> {
    finest_fit_with(cfg, surface, model_unit_m, budget_bytes, &SolverOptions::default())
}

/// `finest_fit` with the caller's options (margins, storage, ...).
pub fn finest_fit_with(cfg: &WindConfig, surface: &TriMesh, model_unit_m: f64, budget_bytes: u64, opt: &SolverOptions) -> Result<u32> {
    let fits = |c: u32| -> Result<bool> {
        let o = SolverOptions { cells_across: Some(c), ..opt.clone() };
        Ok(plan(cfg, surface, model_unit_m, &o, Some(budget_bytes))?.memory <= budget_bytes)
    };
    let (mut lo, mut hi) = (8u32, 4097u32);
    if !fits(lo)? {
        bail!("even the coarsest grid does not fit the memory budget");
    }
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if fits(mid)? { lo = mid } else { hi = mid }
    }
    Ok(lo.min(4096))
}

/// Runs the wind tunnel. `surface` in metres; `model_unit_m` metres per
/// model unit (0.001 for mm), used for `Spin::center_mm` and output positions.
pub fn run(cfg: &WindConfig, surface: &TriMesh, model_unit_m: f64, progress: &dyn Progress) -> Result<WindResult> {
    run_with(cfg, surface, model_unit_m, &SolverOptions::default(), progress)
}

pub fn run_with(cfg: &WindConfig, surface: &TriMesh, model_unit_m: f64, opt: &SolverOptions, progress: &dyn Progress) -> Result<WindResult> {
    let t_start = Instant::now();
    let p = plan(cfg, surface, model_unit_m, opt, None)?;
    progress.report(0.0, "Building the wind tunnel grid");
    let n = p.dims.iter().map(|&d| d as u64).product::<u64>();
    let mut warnings: Vec<String> = Vec::new();

    // geometry in lattice coordinates (static) or body cells (spinning)
    let to_lat = |v: Vec3| -> Vec3 {
        let q = mat_vec(&p.r, sub(v, p.pref));
        [p.center[0] + q[0] / p.dx, p.center[1] + q[1] / p.dx, p.center[2] + q[2] / p.dx]
    };
    let to_body = |v: Vec3| -> Vec3 { scale(mat_vec(&p.r, sub(v, p.pref)), 1.0 / p.dx) };
    let geom = if p.spin {
        let s = Surface { verts: surface.positions.iter().map(|v| to_body(*v)).collect(), tris: surface.triangles.clone() };
        // occupancy 8x finer than the lattice (capped at 64 M samples), so
        // cells switch between solid and fluid where the surface really is
        let (blo, bhi) = s.bounds();
        let vol: f64 = (0..3).map(|k| bhi[k] - blo[k] + 2.0).product();
        let h = (vol / 64e6).cbrt().max(0.125);
        let vox = voxelize_spin(&s, p.axis_lat, h);
        let solid_cells = vox.occ_bits.iter().map(|b| b.count_ones() as f64).sum::<f64>() * vox.occ_h.powi(3);
        check_thin(&mut warnings, solid_cells, surface, p.dx);
        Geo::Old(Geometry::Spin { vox, box_lo: p.box_lo, box_n: p.box_n })
    } else if let Some(mp) = &p.multi {
        let fd = {
            let b = mp.boxes.last().unwrap();
            [2 * (b.n[0] - 1) + 1, 2 * (b.n[1] - 1) + 1, 2 * (b.n[2] - 1) + 1]
        };
        let o = mp.org_f;
        let s = Surface { verts: surface.positions.iter().map(|v| { let x = to_lat(*v); [x[0] - o[0], x[1] - o[1], x[2] - o[2]] }).collect(), tris: surface.triangles.clone() };
        let vox = crate::voxel::voxelize_static27(&s, [fd[0] as usize, fd[1] as usize, fd[2] as usize]);
        check_thin(&mut warnings, vox.solid.len() as f64, surface, p.dx);
        Geo::New(MGeometry::Static(vox))
    } else {
        let s = Surface { verts: surface.positions.iter().map(|v| to_lat(*v)).collect(), tris: surface.triangles.clone() };
        let vox = voxelize_static(&s, [p.dims[0] as usize, p.dims[1] as usize, p.dims[2] as usize]);
        check_thin(&mut warnings, vox.solid.len() as f64, surface, p.dx);
        if std::env::var_os("SIMLAB_CFD_DEBUG").is_some() {
            let mut cnt = [0u32; 19];
            for w in &vox.walls { for d in 1..19 { if w.mask & (1 << d) != 0 { cnt[d] += 1; } } }
            eprintln!("plan: dims {:?} center {:?} dx {} sponge {:?} solid {} walls {} counts {:?}", p.dims, p.center, p.dx, p.sponge, vox.solid.len(), vox.walls.len(), &cnt[1..]);
        }
        Geo::Old(Geometry::Static(vox))
    };
    if progress.cancelled() {
        return Err(Cancelled.into());
    }

    // probes
    let flip = if surface.volume() < 0.0 { -1.0 } else { 1.0 };
    let mut vn = vec![[0.0f64; 3]; surface.positions.len()];
    for t in &surface.triangles {
        let (a, b, c) = (surface.positions[t[0] as usize], surface.positions[t[1] as usize], surface.positions[t[2] as usize]);
        let nrm = cross(sub(b, a), sub(c, a));
        for &i in t {
            vn[i as usize] = simlab_core::add(vn[i as usize], nrm);
        }
    }
    let verts: Vec<([f64; 3], [f64; 3])> = surface
        .positions
        .iter()
        .zip(&vn)
        .map(|(v, nn)| (to_body(*v), mat_vec(&p.r, scale(normalize(*nn), flip))))
        .collect();
    let [nx, ny, nz] = p.dims;
    // slices through the part centre (global finest-cell coordinates); on a
    // refined grid each pixel takes the finest level covering it, with a
    // pitch that keeps a slice under ~4 M pixels
    let pitch: i32 = if p.multi.is_some() {
        let mut pt = 1i32;
        while ((nx as u64 / pt as u64) * (nz as u64 / pt as u64)).max((ny as u64 / pt as u64) * (nz as u64 / pt as u64)) > 4_000_000 {
            pt *= 2;
        }
        pt
    } else {
        1
    };
    let snap = |x: f64| -> i32 { ((x / pitch as f64).round() as i32 * pitch).max(pitch) };
    let (xc, yc) = (snap(p.center[0].clamp(1.0, nx as f64 - 2.0)), snap(p.center[1].clamp(1.0, ny as f64 - 2.0)));
    let span = |n: u32| -> u32 { ((n as i32 - 1 - pitch) / pitch).max(1) as u32 };
    // (origin, u, v, nu, nv)
    let slices: Vec<([i32; 3], [i32; 3], [i32; 3], u32, u32)> = vec![
        ([pitch, yc, pitch], [pitch, 0, 0], [0, 0, pitch], span(nx), span(nz)),
        ([xc, pitch, pitch], [0, pitch, 0], [0, 0, pitch], span(ny), span(nz)),
    ];
    let u_inf = [p.u_lat, 0.0, 0.0];
    progress.report(0.01, "Preparing the GPU");
    let mut lat = match geom {
        Geo::Old(g) => {
            let probes = Probes {
                verts,
                slices: slices.iter().map(|s| SliceDef { origin: s.0, u: s.1, v: s.2, nu: s.3, nv: s.4 }).collect(),
                vertex_offset: opt.vertex_offset,
            };
            let setup = LatticeSetup {
                dims: p.dims,
                faces: p.faces,
                nu: p.nu_lat,
                smagorinsky: opt.smagorinsky,
                u_inf,
                u_init: u_inf,
                body_force: [0.0; 3],
                sponge: p.sponge,
                sponge_tau: 1.0,
                sponge_sigma: opt.sponge_sigma,
                absorb_velocity: opt.absorb_velocity && !p.still,
                total_pressure_inlet: opt.total_pressure_inlet,
                storage: p.storage,
                center: p.center,
                debug_dump: false,
                wall_model: opt.wall_model,
            };
            Engine::One(Lattice::new(setup, g, probes)?)
        }
        Geo::New(g) => {
            let mp = p.multi.as_ref().unwrap();
            let nl = mp.cells.len();
            let probes = MProbes {
                verts,
                vertex_offset: opt.vertex_offset,
                slices: slices.iter().map(|s| (crate::mshader::SliceDef { origin: s.0, u: s.1, v: s.2, nu: s.3, nv: s.4 }, pitch as u32)).collect(),
            };
            let setup = MultiSetup {
                dims0: mp.dims0,
                boxes: mp.boxes.clone(),
                faces: p.faces,
                nu0: p.nu_lat / (1u64 << (nl - 1)) as f64,
                smagorinsky: 0.0,
                u_inf,
                u_init: u_inf,
                body_force0: [0.0; 3],
                sponge: p.sponge,
                sponge_tau: 1.0,
                sponge_sigma: opt.sponge_sigma,
                absorb_velocity: opt.absorb_velocity,
                total_pressure_inlet: opt.total_pressure_inlet,
                storage: p.storage,
                center: [p.center[0] - mp.org_f[0], p.center[1] - mp.org_f[1], p.center[2] - mp.org_f[2]],
                debug_dump: false,
            };
            Engine::Multi(MultiLattice::new(setup, g, probes)?)
        }
    };
    if lat.storage() != p.storage {
        warnings.push("this GPU has no half-precision storage; used FP32".into());
    }
    if lat.storage() == Storage::Fp16 {
        warnings.push("FP16 storage (to fit the memory budget): slightly noisier forces".into());
    }
    let t_setup = t_start.elapsed().as_secs_f64();

    // time loop
    let ss = lat.sub_steps();
    let total_steps = ((p.duration * p.steps_unit).ceil() as u64).div_ceil(ss) * ss;
    let omega_mag = norm(p.omega_lat);
    let ramp_steps = if p.spin { (opt.spin_ramp_revs * 2.0 * PI / omega_mag).ceil() as u64 } else { 0 };
    let unit_steps = p.steps_unit;
    // wind: skip 8 flow passes, then average at least 10 (a chaotic wake at
    // Re >= 1000 needs ~10+ passes per half-window for a ~2 % mean)
    let (tr_units, min_win_units): (f64, f64) = if p.still { (3.0, 2.0) } else { (8.0, 10.0) };
    let mut w0 = (tr_units.min(0.35 * p.duration) * unit_steps) as u64;
    w0 = w0.max(ramp_steps + (0.5 * unit_steps) as u64);
    let min_win = (min_win_units.min(0.3 * p.duration) * unit_steps) as u64;
    // short runs still average over their second half
    w0 = w0.min(total_steps / 2);
    let sample_every = ((unit_steps / 100.0).floor() as u64).max(1);
    let batch = (((0.25 * MLUPS_FP32 * 1e6 / p.updates_per_step) as u64).clamp(16, 4096)).div_ceil(ss) * ss;

    let mut hist: Vec<[f64; 6]> = Vec::with_capacity(total_steps as usize);
    let mut theta = 0.0f64;
    let mut rot_prev = rotation(p.axis_lat, 0.0);
    let mut step: u64 = 0;
    let mut converged = false;
    let mut sampling = false;
    let mut history_out: Vec<(f64, Vec3, Vec3)> = Vec::new();
    let fscale = cfg.air.density * p.dx.powi(4) / (p.dt * p.dt);
    let mscale = fscale * p.dx;
    let axis_l = normalize(p.axis_lat);
    let t_loop = Instant::now();
    while step < total_steps {
        let nb = batch.min(total_steps - step);
        let mut ins = Vec::with_capacity(nb as usize);
        for k in 0..nb {
            let s = step + k;
            let mut st = StepIn::still(false);
            if p.spin {
                let f = if s < ramp_steps { let x = (s as f64 + 0.5) / ramp_steps as f64; x * x * (3.0 - 2.0 * x) } else { 1.0 };
                let om = scale(p.omega_lat, f);
                // signed: negative rpm turns the part the other way about +axis
                theta += f * dot(p.omega_lat, axis_l);
                let rot = rotation(axis_l, theta);
                st = StepIn { rot, rot_prev, omega: om, sample: false };
                rot_prev = rot;
            } else {
                st.rot = IDENTITY;
            }
            if s + 1 > w0 && (s + 1 - w0) % sample_every == 0 {
                st.sample = true;
            }
            ins.push(st);
        }
        if !sampling && step + nb > w0 {
            lat.reset_samples();
            sampling = true;
        }
        let h = lat.run(&ins)?;
        hist.extend_from_slice(&h);
        step += nb;
        // live history: batch mean, model axes, SI
        let mut fm = [0.0; 6];
        for v in &h {
            for j in 0..6 {
                fm[j] += v[j] / h.len() as f64;
            }
        }
        let f_model = scale(mat_t_vec(&p.r, [fm[0], fm[1], fm[2]]), fscale);
        let m_model = scale(mat_t_vec(&p.r, [fm[3], fm[4], fm[5]]), mscale);
        let tu = step as f64 / unit_steps;
        history_out.push((tu, f_model, m_model));
        if p.spin {
            progress.metric("thrust_n", step, dot(f_model, p.axis_model));
            progress.metric("torque_nm", step, dot(m_model, p.axis_model));
        }
        if !p.still {
            progress.metric("drag_n", step, dot(f_model, p.r[0]));
            progress.metric("lift_n", step, dot(f_model, p.r[2]));
        }
        let el = t_loop.elapsed().as_secs_f64();
        let mlups = (step as f64 * p.updates_per_step) / el / 1e6;
        progress.report(
            0.02 + 0.96 * (step as f32 / total_steps as f32),
            &format!("Solving: {step} of {total_steps} steps ({:.1} {}, {:.0} MLUPs)", tu, if p.still { "revolutions" } else { "flow passes" }, mlups),
        );
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        if !hist.iter().all(|v| v.iter().all(|x| x.is_finite())) {
            bail!("the flow solution blew up (unstable); try a finer grid or a lower speed");
        }
        // convergence on the averaging window [w0, step)
        if step > w0 + min_win {
            let w = &hist[w0 as usize..];
            let half = w.len() / 2;
            // monitored: drag (wind) or thrust and torque (spinning)
            let signal = |v: &[f64; 6], which: usize| -> f64 {
                if p.spin {
                    if which == 0 { v[0] * axis_l[0] + v[1] * axis_l[1] + v[2] * axis_l[2] } else { v[3] * axis_l[0] + v[4] * axis_l[1] + v[5] * axis_l[2] }
                } else {
                    v[0]
                }
            };
            let nsig = if p.spin { 2 } else { 1 };
            let mut worst: f64 = 0.0;
            // statistical error of the window mean, from 8 block means: a
            // chaotic wake can make the two halves agree by chance
            let mut se_rel: f64 = 0.0;
            let mut trend = false;
            for which in 0..nsig {
                let m1: f64 = w[..half].iter().map(|v| signal(v, which)).sum::<f64>() / half as f64;
                let m2: f64 = w[half..].iter().map(|v| signal(v, which)).sum::<f64>() / (w.len() - half) as f64;
                let sc = 0.5 * (m1.abs() + m2.abs());
                let nbk = 8usize;
                let bl = w.len() / nbk;
                let bm: Vec<f64> = (0..nbk).map(|k| w[k * bl..(k + 1) * bl].iter().map(|v| signal(v, which)).sum::<f64>() / bl as f64).collect();
                let mm = bm.iter().sum::<f64>() / nbk as f64;
                let sd = (bm.iter().map(|b| (b - mm) * (b - mm)).sum::<f64>() / (nbk - 1) as f64).sqrt();
                let se = sd / (nbk as f64).sqrt();
                if sc > 0.0 {
                    worst = worst.max((m2 - m1).abs() / 2.0 / sc);
                    se_rel = se_rel.max(se / sc);
                    // a real drift: halves differ by far more than their noise
                    trend |= (m2 - m1).abs() > 6.0 * se;
                }
            }
            if worst < opt.tolerance && se_rel < opt.tolerance {
                converged = true;
                break;
            }
            // still drifting: drop the first half of the window (surface and
            // slice sums restart now, so they cover the newest part of it);
            // never in the last 30 % of the run, so the final average keeps
            // a long window
            if worst > 3.0 * opt.tolerance && trend && w.len() as u64 >= 2 * min_win && (step as f64) < 0.7 * total_steps as f64 {
                w0 += half as u64;
                lat.reset_samples();
            }
        }
    }
    let loop_s = t_loop.elapsed().as_secs_f64();
    let mlups = step as f64 * p.updates_per_step / loop_s / 1e6;

    // averages over the window
    let w = &hist[(w0 as usize).min(hist.len().saturating_sub(1))..];
    let mut avg = [0.0f64; 6];
    for v in w {
        for j in 0..6 {
            avg[j] += v[j] / w.len() as f64;
        }
    }
    let force = scale(mat_t_vec(&p.r, [avg[0], avg[1], avg[2]]), fscale);
    let moment = scale(mat_t_vec(&p.r, [avg[3], avg[4], avg[5]]), mscale);
    let rho = cfg.air.density;
    let vel = p.dx / p.dt;
    let (drag_dir, lift_dir, side_dir) = (p.r[0], p.r[2], p.r[1]);
    let drag = dot(force, drag_dir);
    let lift = dot(force, lift_dir);
    let side = dot(force, side_dir);
    let speed = cfg.speed_m_s;
    let spin = cfg.spin.filter(|_| p.spin);
    let q_ref = if speed > 0.0 { 0.5 * rho * speed * speed } else { 0.5 * rho * (norm(p.omega_lat) / p.dt * p.rotor_radius).powi(2) };
    let cd = if speed > 0.0 && !p.spin && p.frontal_area > 0.0 { Some(drag / (q_ref * p.frontal_area)) } else { None };
    let (thrust, torque, power) = match spin {
        Some(s) => {
            let a = p.axis_model;
            let om = s.rpm * 2.0 * PI / 60.0;
            let tq = dot(moment, a);
            (Some(dot(force, a)), Some(tq), Some(-tq * om))
        }
        None => (None, None, None),
    };
    let reynolds = if p.spin {
        // blade Reynolds number at 75 % radius, with the mean chord estimated
        // from the surface area (both faces of two blades: A = 4 R c)
        let area: f64 = (0..surface.triangles.len()).map(|t| surface.triangle_area(t)).sum();
        let chord = area / (4.0 * p.rotor_radius);
        let om = norm(p.omega_lat) / p.dt;
        (0.75 * om * p.rotor_radius + speed) * chord / cfg.air.kinematic_viscosity
    } else {
        speed * p.d_front / cfg.air.kinematic_viscosity
    };

    // surface pressure
    let vm = lat.vertex_means()?;
    let pconv = rho * vel * vel / 3.0;
    let surface_pressure_pa: Vec<f32> = vm.iter().map(|&r| ((r as f64 - 1.0) * pconv) as f32).collect();
    let surface_cp: Vec<f32> = surface_pressure_pa.iter().map(|&pp| (pp as f64 / q_ref) as f32).collect();

    // slices
    let sm = lat.slice_means()?;
    let mut out_slices = Vec::new();
    let mut off = 0usize;
    let to_mm = |v: Vec3| scale(v, 1.0 / model_unit_m);
    let lat_to_model = |x: [f64; 3]| -> Vec3 {
        let q = [(x[0] - p.center[0]) * p.dx, (x[1] - p.center[1]) * p.dx, (x[2] - p.center[2]) * p.dx];
        simlab_core::add(p.pref, mat_t_vec(&p.r, q))
    };
    for (si, &(s_origin, s_u, s_v, s_nu, s_nv)) in slices.iter().enumerate() {
        let np = (s_nu * s_nv) as usize;
        let px = &sm[off..off + np];
        off += np;
        // step vectors (one pixel, finest cells) and unit axes
        let su = [s_u[0] as f64, s_u[1] as f64, s_u[2] as f64];
        let sv = [s_v[0] as f64, s_v[1] as f64, s_v[2] as f64];
        let ua = normalize(su);
        let va = normalize(sv);
        let corner = [s_origin[0] as f64 - 0.5 * (su[0] + sv[0]), s_origin[1] as f64 - 0.5 * (su[1] + sv[1]), s_origin[2] as f64 - 0.5 * (su[2] + sv[2])];
        let origin = lat_to_model(corner);
        let u_mm = to_mm(scale(mat_t_vec(&p.r, su), s_nu as f64 * p.dx));
        let v_mm = to_mm(scale(mat_t_vec(&p.r, sv), s_nv as f64 * p.dx));
        let mut speedf = Vec::with_capacity(np);
        let mut pres = Vec::with_capacity(np);
        let mut uin = Vec::with_capacity(np);
        let mut vin = Vec::with_capacity(np);
        for c in px {
            let u = [c[1] as f64, c[2] as f64, c[3] as f64];
            speedf.push((norm(u) * vel) as f32);
            pres.push(((c[0] as f64 - 1.0) * pconv) as f32);
            uin.push((dot(u, ua) * vel) as f32);
            vin.push((dot(u, va) * vel) as f32);
        }
        let mk = |name: &str, label: &str, unit: &str, data: Vec<f32>| {
            let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
            for &x in &data {
                if x.is_finite() {
                    mn = mn.min(x);
                    mx = mx.max(x);
                }
            }
            SliceField { name: name.into(), label: label.into(), unit: unit.into(), min: mn, max: mx, data }
        };
        let (name, label, ul, vl) = if si == 0 {
            ("along", if p.still { "Along the spin axis" } else { "Along the wind" }, if p.still { "axial" } else { "streamwise" }, "up")
        } else {
            ("across", if p.still { "Across the axis, through the rotor" } else { "Across the wind, through the part" }, "sideways", "up")
        };
        out_slices.push(FlowSlice {
            name: name.into(),
            label: label.into(),
            origin_mm: to_mm(origin),
            u_mm,
            v_mm,
            nu: s_nu,
            nv: s_nv,
            fields: vec![
                mk("speed", "Air speed", "m/s", speedf),
                mk("pressure", "Pressure (relative to far field)", "Pa", pres),
                mk("u", &format!("In-plane velocity, {ul}"), "m/s", uin),
                mk("v", &format!("In-plane velocity, {vl}"), "m/s", vin),
            ],
        });
    }

    // warnings
    if !converged {
        warnings.push(format!("The forces had not settled to within {:.1}% by the end of the run, so the numbers are averages over its last part.", opt.tolerance * 100.0));
    }
    if p.blockage > 0.06 {
        warnings.push(format!("The part fills {:.1}% of the tunnel's cross-section, so drag may read a little high.", p.blockage * 100.0));
    }
    let tau0 = 0.5 + 3.0 * p.nu_lat;
    let wall_cells_re = if p.spin { reynolds } else { speed * p.dx / cfg.air.kinematic_viscosity };
    if tau0 < 0.502 && wall_cells_re > 300.0 {
        warnings.push(format!(
            "The thin layer of air hugging the surface is finer than one grid cell here (cell Reynolds number {:.0}), so surface friction and where the flow breaks away are approximate.",
            wall_cells_re
        ));
    }
    let seconds = t_start.elapsed().as_secs_f64();
    let simulated = step as f64 / unit_steps;
    let averaged = (step.saturating_sub(w0)) as f64 / unit_steps;
    let headline = if let (Some(t), Some(tq), Some(pw)) = (thrust, torque, power) {
        format!(
            "Thrust {} N ({} g), torque {} mN·m, {} W at {} rpm",
            sig(t.abs(), 3),
            sig(t.abs() / 9.80665e-3, 3),
            sig(tq.abs() * 1e3, 3),
            sig(pw.abs(), 3),
            sig(spin.unwrap().rpm.abs(), 4)
        )
    } else if let Some(c) = cd {
        format!("Drag {} N at {} m/s (Cd {})", sig(drag, 3), sig(speed, 3), sig(c, 3))
    } else {
        format!("Drag {} N at {} m/s", sig(drag, 3), sig(speed, 3))
    };
    let _ = t_setup;
    let summary = WindSummary {
        kind: "wind".into(),
        headline,
        force_n: force,
        moment_nm: moment,
        drag_n: drag,
        lift_n: lift,
        side_n: side,
        cd,
        ref_area_mm2: p.frontal_area * 1e6,
        thrust_n: thrust,
        torque_nm: torque,
        power_w: power,
        reynolds,
        cells: match &p.multi {
            Some(mp) => mp.cells.iter().sum(),
            None => n,
        },
        steps: step,
        mlups,
        seconds,
        converged,
        warnings,
        grid: p.dims,
        cells_across: p.cells_across,
        dx_mm: p.dx * 1e3,
        dt_s: p.dt,
        mach_lattice: (p.u_lat + norm(p.omega_lat) * p.rotor_radius / p.dx) * 3f64.sqrt(),
        tau: tau0,
        blockage: p.blockage,
        storage: lat.storage(),
        memory_bytes: lat.memory_bytes(),
        averaged_over: averaged,
        simulated,
        time_unit: if p.still { "revolutions".into() } else { "passes".into() },
        q_ref_pa: q_ref,
    };
    Ok(WindResult { summary, surface_pressure_pa, surface_cp, slices: out_slices, history: history_out })
}

/// `x` with `n` significant figures, plain decimal notation (no exponent).
pub fn sig(x: f64, n: i32) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let mag = x.abs().log10().floor() as i32;
    let decimals = (n - 1 - mag).max(0) as usize;
    let factor = 10f64.powi(n - 1 - mag);
    let rounded = (x * factor).round() / factor;
    format!("{rounded:.decimals$}")
}

fn check_thin(warnings: &mut Vec<String>, solid_cells: f64, m: &TriMesh, dx: f64) {
    let vol = m.volume().abs();
    if vol > 0.0 {
        let ratio = solid_cells * dx * dx * dx / vol;
        if ratio < 0.6 {
            warnings.push(format!(
                "parts of the model are thinner than a grid cell ({:.0} % of its volume became solid cells); thin walls still block the air, but use a finer grid for accurate forces",
                ratio * 100.0
            ));
        }
    } else {
        warnings.push("the surface is not closed; inside and outside were guessed by voting along three directions".into());
    }
}
