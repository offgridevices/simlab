//! SimLab wind tunnel: lattice Boltzmann on the GPU (wgpu -> Metal). A quick
//! preview; the reference answer for air tests is the OpenFOAM runner.
//!
//! Two solvers:
//! * Still parts with 48 or more cells across (Draft and up): a refined
//!   D3Q27 solver (`mlattice`, `mshader`): nested boxes around the part,
//!   each half the cell size of the one outside it (2-4 levels, the coarsest
//!   keeping ~24 cells across the part), a cumulant-type collision that is
//!   stable at very low viscosity, Bouzidi walls, momentum-exchange forces.
//!   On spheres at Re 20-1e4 it was within ~5 % of the standard drag curve,
//!   and much closer than the uniform grid at Re 1e4 for the same cost.
//! * Spinning parts, and still parts on coarse custom grids: one uniform
//!   D3Q19 grid (`lattice`, `shader`) with regularized collision and
//!   Smagorinsky LES, re-voxelizing a spinning part every step. Spinning
//!   results are a rough preview: on 3-inch props it read 30-60 % low on
//!   thrust and 20-55 % high on torque against OpenFOAM.
//!
//! Study-level API: [`run`], [`run_with`], [`estimate`], [`finest_fit`],
//! [`gpu_info`]. Lower level: [`lattice::Lattice`].
//!
//! Sign conventions in [`WindSummary`] (model axes, SI):
//! * `force_n` and `moment_nm` are the air's force and moment ON the part;
//!   the moment is about the part's bounding-box centre, or the spin centre
//!   when spinning.
//! * `thrust_n` = force · (+spin axis), signed.
//! * `torque_nm` = moment · (+spin axis), signed: the AIR's torque on the
//!   part. For a driven propeller it opposes the rotation, so with positive
//!   rpm it is negative. The torque the motor must supply is `-torque_nm`
//!   (that is what OpenFOAM's MRF reports); flip it once, in the caller.
//! * `power_w` = -torque_nm × ω (ω signed by rpm), already the shaft power
//!   the motor supplies: positive for a driven propeller. Do not flip it.

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

pub mod d3q27;
pub mod gpu;
pub mod lattice;
pub mod mlattice;
pub mod mshader;
pub mod shader;
pub mod shapes;
pub mod tunnel;
pub mod validation;
pub mod voxel;

pub use gpu::gpu_info;
pub use lattice::Storage;
pub use tunnel::{
    Estimate, FlowSlice, THIN_MAX_BOOST, cells_for_thin, thin_cells_target, thin_cost_cap, SliceField, SolverOptions, WindResult, WindSummary, cells_across, default_duration, estimate, estimate_with, finest_fit, finest_fit_with, max_cells_layout, planned_cells_across, run,
    run_with,
};
