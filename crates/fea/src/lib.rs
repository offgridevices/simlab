//! SimLab's stress test: linear elastic finite elements on quadratic
//! tetrahedra (tet10), for 3D-printed parts.
//!
//! * [`solve_strength`] runs a whole study from a [`simlab_core::StrengthConfig`]:
//!   supports (fixed patches, or free-flying with inertia relief), forces,
//!   pressures, motor thrust/torque/weight, g-loads, spin, and per-face air
//!   loads; then stresses, the Tsai-Hill failure index in the print frame,
//!   the safety factor, where it breaks first and at what load or rpm.
//!   [`StrengthResult::summary`] gives the API's `StrengthSummary`;
//!   [`StrengthResult::surface_fields`] the per-node fields for display.
//! * [`estimate_memory`] sizes a run before meshing.
//! * [`calculix`] writes the same problem as a CalculiX deck and reads its
//!   `.frd` results back for node-by-node comparison.
//! * Lower level: [`solver::solve`] for any set of prescribed displacement
//!   components and nodal loads, [`recovery::recover`] for stresses.
//!
//! Units: SI inside (m, N, Pa, kg). Config positions are model units
//! (`model_unit_m` metres each); reported positions are given in both.
//!
//! Material model and failure criterion: see [`material`].

// numeric kernels index several arrays in step; `!(x > 0.0)` deliberately also rejects NaN
#![allow(clippy::needless_range_loop, clippy::neg_cmp_op_on_partial_ord)]

pub mod amg;
pub mod assembly;
pub mod calculix;
pub mod elem;
pub mod loads;
pub mod material;
pub mod mesh_gen;
pub mod recovery;
pub mod solver;
pub mod strength;
pub mod summary;
pub mod topo;

pub use material::Elastic;
pub use solver::{MemoryEstimate, SolveOptions, SolverChoice, SolverReport, Stiffening, estimate_memory};
pub use strength::{CriticalPoint, InertiaRelief, Peaks, Prepared, StrengthInput, StrengthResult, postprocess, postprocess_with, prepare, prepare_with_bc, solve_prepared, solve_strength};
pub use summary::{BreaksAt, NodalField, StrengthSummary, Verdict};

/// Each boundary face of `mesh` (in `mesh.faces` order) as (area-weighted
/// centroid in m, outward unit normal, area in m²), integrated over the
/// curved tri6 face. Outward is decided from the owning element, since Gmsh
/// does not orient boundary faces. For mapping air pressure onto the
/// propeller's faces (pass the tractions back in `StrengthInput::face_tractions`).
pub fn boundary_faces(mesh: &simlab_core::TetMesh) -> Vec<(simlab_core::Vec3, simlab_core::Vec3, f64)> {
    let oriented: Vec<[u32; 6]> = match topo::boundary_faces(mesh) {
        Ok(f) => f.iter().map(|b| b.nodes).collect(),
        Err(_) => mesh.faces.clone(),
    };
    oriented
        .iter()
        .map(|f| {
            let x: [simlab_core::Vec3; 6] = std::array::from_fn(|i| mesh.nodes[f[i] as usize]);
            let (mut a, mut c, mut n) = (0.0, [0.0; 3], [0.0; 3]);
            for qp in elem::tri_rule_7() {
                let (p, na) = elem::tri6_point(&x, &qp.l);
                let da = simlab_core::norm(na) * qp.w;
                a += da;
                for k in 0..3 {
                    c[k] += p[k] * da;
                    n[k] += na[k] * qp.w;
                }
            }
            (if a > 0.0 { simlab_core::scale(c, 1.0 / a) } else { x[0] }, simlab_core::normalize(n), a)
        })
        .collect()
}
