//! SimLab wind tunnel: a lattice Boltzmann solver on the GPU (wgpu → Metal).
//!
//! * D3Q19 lattice, in-place AA-pattern streaming (one copy of the
//!   populations), regularized BGK collision with a Smagorinsky eddy
//!   viscosity computed locally from the non-equilibrium stress.
//! * Populations in FP32, or FP16 (scaled IEEE half with stochastic
//!   rounding, FP32 arithmetic) to fit twice the cells.
//! * Static parts: robust CPU voxelization (ray-parity / winding voting
//!   along x, y and z), Bouzidi interpolated bounce-back with per-link wall
//!   distances from exact ray–triangle tests, thin sheets block the flow.
//! * Spinning parts: re-voxelized on the GPU every step (triangles scattered
//!   onto the lattice links, occupancy from a body-frame grid 8× finer than
//!   the lattice), moving-wall bounce-back with u = ω × r, refill of cells the
//!   part uncovers, momentum of covered/uncovered cells added to the force.
//! * Forces: Galilean-invariant momentum exchange, reduced on the GPU every
//!   step, averaged over the converged part of the run.
//!
//! Study-level API: [`run`], [`run_with`], [`estimate`], [`finest_fit`],
//! [`gpu_info`]. Lower level: [`lattice::Lattice`].

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
    Estimate, FlowSlice, SliceField, SolverOptions, WindResult, WindSummary, cells_across, default_duration, estimate, estimate_with, finest_fit, finest_fit_with, max_cells_layout, run,
    run_with,
};
