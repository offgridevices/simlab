//! Linear solve K u = f: sparse Cholesky (faer, supernodal, multithreaded)
//! when the factor fits the memory budget, otherwise conjugate gradients
//! preconditioned with smoothed-aggregation algebraic multigrid
//! (see [`crate::amg`]).

use crate::assembly::{Dirichlet, DofMatrix, Layout, Pattern, assemble};
use crate::elem::Stiffness;
use crate::material::Elastic;
use crate::topo::{NodeGraph, color_elements, node_graph};
use anyhow::{Context, Result, anyhow, bail};
use faer::dyn_stack::{MemBuffer, MemStack, StackReq};
use faer::sparse::linalg::cholesky::{
    CholeskySymbolicParams, SymbolicCholesky, SymbolicCholeskyRaw, SymmetricOrdering, factorize_symbolic_cholesky,
};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMatRef};
use faer::{Conj, MatMut, Par, Side};
use serde::{Deserialize, Serialize};
use simlab_core::{Cancelled, Progress, TetMesh};
use std::time::Instant;

/// Which linear solver to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SolverChoice {
    /// direct when the factor fits the memory budget, else iterative
    #[default]
    Auto,
    Direct,
    Iterative,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolveOptions {
    /// bytes the solve may use (the engine's memory budget)
    pub memory_budget_bytes: u64,
    pub solver: SolverChoice,
    /// iterative solver: stop when ‖f − K u‖ ≤ rel_tol · ‖f‖. 1e-8 keeps
    /// stresses within ~1e-6 of the direct solution on the validation cases,
    /// far inside the 0.1 % target (thin parts cannot get much below ~5e-9
    /// in double precision; the direct solver lands there too).
    pub rel_tol: f64,
    pub max_iterations: usize,
    #[serde(default)]
    pub stiffening: Stiffening,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions { memory_budget_bytes: 16 << 30, solver: SolverChoice::Auto, rel_tol: 1e-8, max_iterations: 5000, stiffening: Stiffening::Auto }
    }
}

/// Stress stiffening: a part under high tension gets stiffer against
/// bending (a spinning propeller blade bends far less than a linear
/// analysis predicts; the earlier CalculiX study used full geometric
/// nonlinearity for this reason). When on, the solve is repeated with the
/// geometric stiffness of the current stress (and spin softening for spin
/// loads) until it settles. Free-flying parts are never stiffened (their
/// minimal support must carry no load).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Stiffening {
    /// on when the loads include a spin and the part is held
    #[default]
    Auto,
    On,
    Off,
}

/// What the solver did, for the run log and summary.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SolverReport {
    /// "direct" or "iterative"
    pub method: String,
    /// plain words, e.g. "sparse Cholesky (faer), 412 M factor entries"
    pub detail: String,
    pub dofs: usize,
    pub matrix_nnz: usize,
    pub factor_nnz: usize,
    pub iterations: usize,
    /// ‖f − K u‖ / ‖f‖ after the solve
    pub relative_residual: f64,
    pub graph_seconds: f64,
    pub assembly_seconds: f64,
    /// ordering + symbolic analysis (direct) or multigrid setup (iterative)
    pub setup_seconds: f64,
    pub factor_seconds: f64,
    pub solve_seconds: f64,
    /// estimated peak bytes of the chosen method
    pub memory_bytes: u64,
    /// estimated bytes the direct solver would have needed
    pub direct_memory_bytes: u64,
    /// stress-stiffening passes after the linear solve (0 = none)
    pub stiffening_passes: usize,
    /// relative displacement change of the last stiffening pass
    pub stiffening_change: f64,
    /// why Auto switched from the iterative to the direct solver, if it did
    pub fallback: Option<String>,
}

/// Memory estimate for a mesh that may not exist yet.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MemoryEstimate {
    pub dofs: usize,
    pub direct_bytes: u64,
    pub iterative_bytes: u64,
    /// expected solve time on an idle M4 Max with the multigrid solver (s);
    /// thin, poorly shaped meshes can take several times longer
    pub seconds: f64,
}

/// Rough peak memory of a stress solve for a tet10 mesh with `nodes` nodes
/// and `tets` elements, before meshing (from the expected sizes; the real
/// check happens after the symbolic factorization). Fitted on Gmsh meshes of
/// a drone arm and a propeller measured on an M4 Max.
pub fn estimate_memory(nodes: usize, tets: usize) -> MemoryEstimate {
    let dofs = 3 * nodes;
    let n = dofs as f64;
    // Cholesky factor entries: nnz(L) ≈ 9.2 · dofs^1.39 on a bulky part
    // (measured 138 M at 145k dofs, 677 M at 455k; thin parts such as
    // propellers need less: 285 M at 533k)
    let l = FACTOR_C * n.powf(FACTOR_EXP);
    let direct = l * 8.8 + 2.7e3 * n + mesh_bytes(nodes, tets) + 1e8;
    // iterative: measured peak 0.79 GB at 145k dofs, 2.5 GB at 455k, 5.2 GB at 1.2M
    let iterative = 4.3e3 * n + mesh_bytes(nodes, tets) + 1.5e8;
    MemoryEstimate { dofs, direct_bytes: direct as u64, iterative_bytes: iterative as u64, seconds: iterative_seconds(dofs) }
}

/// Fit of nnz(L) against dofs (see `estimate_memory`).
const FACTOR_C: f64 = 9.2;
const FACTOR_EXP: f64 = 1.39;

fn mesh_bytes(nodes: usize, tets: usize) -> f64 {
    // the mesh itself and the per-node results
    nodes as f64 * (24.0 + 8.0 * 12.0) + tets as f64 * 40.0
}

/// Node-level fill-reducing ordering (AMD on the node graph), expanded to
/// dofs. Ordering the 3× smaller node graph is ~10× faster than ordering the
/// dof matrix and keeps each node's 3 dofs together (better supernodes).
fn node_amd_dof_perm(g: &NodeGraph) -> Result<(Vec<u32>, Vec<u32>)> {
    let n = g.n();
    let ptr: Vec<u32> = g.ptr.iter().map(|&p| p as u32).collect();
    let a = SymbolicSparseColMatRef::<u32>::new_checked(n, n, &ptr, None, &g.adj);
    let mut fwd = vec![0u32; n];
    let mut inv = vec![0u32; n];
    let req = faer::sparse::linalg::amd::order_scratch::<u32>(n, g.adj.len());
    let mut mem = MemBuffer::try_new(req).map_err(|_| anyhow!("out of memory while ordering the matrix"))?;
    faer::sparse::linalg::amd::order(&mut fwd, &mut inv, a, Default::default(), MemStack::new(&mut mem)).map_err(|e| anyhow!("ordering failed: {e:?}"))?;
    let mut dfwd = vec![0u32; 3 * n];
    let mut dinv = vec![0u32; 3 * n];
    for k in 0..n {
        for c in 0..3 {
            dfwd[3 * k + c] = 3 * fwd[k] + c as u32;
        }
    }
    for (k, &d) in dfwd.iter().enumerate() {
        dinv[d as usize] = k as u32;
    }
    Ok((dfwd, dinv))
}

/// Floating-point work of a supernodal factorization (multiply-adds).
fn factor_flops(sym: &SymbolicCholesky<u32>) -> f64 {
    match sym.raw() {
        SymbolicCholeskyRaw::Supernodal(s) => {
            let mut f = 0.0;
            for k in 0..s.n_supernodes() {
                let nc = (s.supernode_end()[k] - s.supernode_begin()[k]) as f64;
                let np = s.supernode(k).pattern().len() as f64;
                f += nc * nc * nc / 3.0 + nc * nc * np + nc * np * np;
            }
            f
        }
        SymbolicCholeskyRaw::Simplicial(_) => sym.len_val() as f64 * 50.0,
    }
}

pub struct DirectPlan {
    pub pattern: Pattern,
    pub symbolic: SymbolicCholesky<u32>,
    pub flops: f64,
    pub memory_bytes: u64,
    pub factor_nnz: usize,
    pub seconds: f64,
}

/// Ordering + symbolic factorization (no numbers yet): tells how much memory
/// the direct solver needs.
pub fn plan_direct(g: &NodeGraph) -> Result<DirectPlan> {
    let t = Instant::now();
    let pattern = Pattern::new(g, Layout::Lower)?;
    let n = pattern.n_dof;
    let (fwd, inv) = node_amd_dof_perm(g)?;
    let a = SymbolicSparseColMatRef::<u32>::new_checked(n, n, &pattern.ptr, None, &pattern.idx);
    let perm = faer::perm::PermRef::<u32>::new_checked(&fwd, &inv, n);
    let symbolic = factorize_symbolic_cholesky(a, Side::Lower, SymmetricOrdering::Custom(perm), CholeskySymbolicParams::default())
        .map_err(|e| anyhow!("the matrix is too large for the direct solver ({e:?})"))?;
    let par = Par::rayon(0);
    let factor_scratch = symbolic.factorize_numeric_llt_scratch::<f64>(par, Default::default());
    let solve_scratch = symbolic.solve_in_place_scratch::<f64>(1, par);
    let len = symbolic.len_val();
    let mem = len as u64 * 8
        + pattern.idx.len() as u64 * 12 // values + indices of A
        + StackReq::any_of(&[factor_scratch, solve_scratch]).size_bytes() as u64
        + n as u64 * 8 * 8;
    let flops = factor_flops(&symbolic);
    Ok(DirectPlan { pattern, symbolic, flops, memory_bytes: mem, factor_nnz: len, seconds: t.elapsed().as_secs_f64() })
}

/// Solves K u = f with the prescribed values in `bc`. `f` is the external
/// nodal load (length 3·nodes). Progress fractions are mapped into `frac`.
pub fn solve(
    mesh: &TetMesh,
    el: &Elastic,
    bc: &Dirichlet,
    f: &[f64],
    opts: &SolveOptions,
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<(Vec<f64>, SolverReport)> {
    solve_with(mesh, &Stiffness::plain(el), bc, f, opts, progress, frac)
}

/// Like [`solve`], with stress stiffening / spin softening terms.
pub fn solve_with(
    mesh: &TetMesh,
    el: &Stiffness,
    bc: &Dirichlet,
    f: &[f64],
    opts: &SolveOptions,
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<(Vec<f64>, SolverReport)> {
    let span = |a: f32| frac.0 + (frac.1 - frac.0) * a;
    let n_dof = 3 * mesh.nodes.len();
    if f.len() != n_dof || bc.fixed.len() != n_dof {
        bail!("load or support vector has the wrong length");
    }
    let mut rep = SolverReport { dofs: n_dof, ..Default::default() };
    let t = Instant::now();
    progress.report(span(0.0), "Linking the elements together");
    let graph = node_graph(mesh.nodes.len(), &mesh.tets);
    let colors = color_elements(mesh.nodes.len(), &mesh.tets);
    rep.graph_seconds = t.elapsed().as_secs_f64();
    if progress.cancelled() {
        return Err(Cancelled.into());
    }

    let direct_fits = |p: &DirectPlan| p.memory_bytes <= opts.memory_budget_bytes;
    let mut plan = None;
    let worth_planning = match opts.solver {
        SolverChoice::Direct => true,
        SolverChoice::Iterative => false,
        // ordering a big mesh costs seconds; skip it when the direct solver
        // clearly would not fit or would be far slower anyway
        SolverChoice::Auto => n_dof <= AUTO_PLAN_MAX_DOFS && estimate_memory(mesh.nodes.len(), mesh.tets.len()).direct_bytes <= 2 * opts.memory_budget_bytes,
    };
    if worth_planning {
        progress.report(span(0.03), "Planning the solve (ordering the equations)");
        match plan_direct(&graph) {
            Ok(p) => {
                rep.direct_memory_bytes = p.memory_bytes;
                if opts.solver == SolverChoice::Direct && !direct_fits(&p) {
                    bail!(
                        "the direct solver needs about {:.1} GB, more than the {:.1} GB allowed; use the iterative solver or a coarser mesh",
                        p.memory_bytes as f64 / 1e9,
                        opts.memory_budget_bytes as f64 / 1e9
                    );
                }
                if direct_fits(&p) {
                    plan = Some(p);
                }
            }
            Err(e) if opts.solver == SolverChoice::Direct => return Err(e),
            Err(_) => {}
        }
    }
    if progress.cancelled() {
        return Err(Cancelled.into());
    }
    let direct_est = plan.as_ref().map(|p| direct_seconds(p.flops));
    let go_direct = match (opts.solver, direct_est) {
        (SolverChoice::Direct, _) => true,
        (SolverChoice::Auto, Some(d)) => d <= iterative_seconds(n_dof),
        _ => false,
    };
    if go_direct {
        return solve_direct(mesh, el, &graph, &colors, bc, f, plan.unwrap(), rep, progress, (span(0.06), frac.1));
    }
    // Auto: race the multigrid solver against the direct estimate. Thin,
    // badly shaped or nearly incompressible meshes can make multigrid slow;
    // then the direct solver (which does not care) takes over.
    let deadline = match (opts.solver, direct_est) {
        (SolverChoice::Auto, Some(d)) => Some((1.5 * d).max(3.0)),
        _ => None,
    };
    match solve_iterative(mesh, el, &graph, &colors, bc, f, opts, rep.clone(), deadline, progress, (span(0.06), frac.1)) {
        Ok(r) => Ok(r),
        Err(e) if e.downcast_ref::<Cancelled>().is_some() => Err(e),
        Err(e) if opts.solver == SolverChoice::Auto => {
            let slow = e.downcast_ref::<TooSlow>().is_some();
            progress.report(
                span(0.06),
                if slow { "The iterative solver was slow on this part; switching to the direct solver" } else { "The iterative solver struggled; switching to the direct solver" },
            );
            let p = match plan {
                Some(p) => p,
                None => {
                    let p = plan_direct(&graph)?;
                    rep.direct_memory_bytes = p.memory_bytes;
                    if !direct_fits(&p) {
                        return Err(e.context("and the direct solver does not fit in the memory budget"));
                    }
                    p
                }
            };
            rep.fallback = Some(if slow { "iterative too slow".into() } else { format!("iterative failed: {e}") });
            solve_direct(mesh, el, &graph, &colors, bc, f, p, rep, progress, (span(0.06), frac.1))
        }
        Err(e) => Err(e),
    }
}

/// The iterative solver gave up because the direct solver would be faster.
#[derive(Debug)]
pub struct TooSlow;
impl std::error::Error for TooSlow {}
impl std::fmt::Display for TooSlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the iterative solver is slower than the direct one would be")
    }
}

/// Auto mode plans the direct solve (ordering + symbolic factorization, to
/// know its memory and cost) up to this size; above it the multigrid solver
/// runs alone and the direct solver is only a fallback (measured at 1.2M
/// dofs: multigrid 7 s, the direct factor would be ~20 GB and minutes).
const AUTO_PLAN_MAX_DOFS: usize = 1_000_000;

/// Measured sustained rate of faer's supernodal Cholesky on the M4 Max.
const DIRECT_FLOP_RATE: f64 = 85e9;

fn direct_seconds(flops: f64) -> f64 {
    0.05 + flops / DIRECT_FLOP_RATE
}

/// Measured: 145k dofs 1.8 s, 455k 2.2 s, 1.2M 7 s (multigrid setup + CG).
fn iterative_seconds(n_dof: usize) -> f64 {
    0.5 + 6e-6 * n_dof as f64
}

#[allow(clippy::too_many_arguments)]
fn solve_direct(
    mesh: &TetMesh,
    el: &Stiffness,
    graph: &NodeGraph,
    colors: &[Vec<u32>],
    bc: &Dirichlet,
    f: &[f64],
    plan: DirectPlan,
    mut rep: SolverReport,
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<(Vec<f64>, SolverReport)> {
    let span = |a: f32| frac.0 + (frac.1 - frac.0) * a;
    let n = plan.pattern.n_dof;
    rep.method = "direct".into();
    rep.setup_seconds = plan.seconds;
    rep.factor_nnz = plan.factor_nnz;
    rep.memory_bytes = plan.memory_bytes;
    rep.matrix_nnz = plan.pattern.idx.len();
    let t = Instant::now();
    let mut rhs = f.to_vec();
    let val = assemble(mesh, graph, colors, el, &plan.pattern, bc, &mut rhs, progress, (span(0.0), span(0.2)))?;
    rep.assembly_seconds = t.elapsed().as_secs_f64();
    if progress.cancelled() {
        return Err(Cancelled.into());
    }
    let est = direct_seconds(plan.flops);
    progress.report(
        span(0.22),
        &format!(
            "Solving {} equations directly (about {} s; this step cannot be stopped part-way)",
            n,
            est.max(1.0).round()
        ),
    );
    let t = Instant::now();
    let a = SparseColMatRef::<u32, f64>::new(SymbolicSparseColMatRef::new_checked(n, n, &plan.pattern.ptr, None, &plan.pattern.idx), &val);
    let par = Par::rayon(0);
    let mut lval: Vec<f64> = Vec::new();
    lval.try_reserve_exact(plan.symbolic.len_val()).map_err(|_| anyhow!("not enough memory for the direct solver"))?;
    lval.resize(plan.symbolic.len_val(), 0.0);
    let req = StackReq::any_of(&[
        plan.symbolic.factorize_numeric_llt_scratch::<f64>(par, Default::default()),
        plan.symbolic.solve_in_place_scratch::<f64>(1, par),
    ]);
    let mut mem = MemBuffer::try_new(req).map_err(|_| anyhow!("not enough memory for the direct solver"))?;
    let llt = plan
        .symbolic
        .factorize_numeric_llt(&mut lval, a, Side::Lower, Default::default(), par, MemStack::new(&mut mem), Default::default())
        .map_err(|_| anyhow!("the part can still move: the supports do not hold it in every direction"))?;
    rep.factor_seconds = t.elapsed().as_secs_f64();
    progress.report(span(0.9), "Back-substitution");
    let t = Instant::now();
    let mut u = rhs.clone();
    llt.solve_in_place_with_conj(Conj::No, MatMut::from_column_major_slice_mut(&mut u, n, 1), par, MemStack::new(&mut mem));
    // one step of iterative refinement (cheap, recovers digits on stiff contrasts)
    let mat = DofMatrix { layout: Layout::Lower, n, ptr: plan.pattern.ptr, idx: plan.pattern.idx, val };
    let mut r = vec![0.0; n];
    mat.mul(&u, &mut r);
    for i in 0..n {
        r[i] = rhs[i] - r[i];
    }
    let fnorm = norm2(&rhs).max(1e-300);
    let mut res = norm2(&r) / fnorm;
    if res > 1e-14 {
        let mut d = r.clone();
        llt.solve_in_place_with_conj(Conj::No, MatMut::from_column_major_slice_mut(&mut d, n, 1), par, MemStack::new(&mut mem));
        for i in 0..n {
            u[i] += d[i];
        }
        mat.mul(&u, &mut r);
        for i in 0..n {
            r[i] = rhs[i] - r[i];
        }
        res = norm2(&r) / fnorm;
    }
    rep.relative_residual = res;
    rep.solve_seconds = t.elapsed().as_secs_f64();
    rep.detail = format!("sparse Cholesky (faer, supernodal), {:.0} M factor entries, {:.1} GFLOP", rep.factor_nnz as f64 / 1e6, plan.flops / 1e9);
    check_finite(&u)?;
    Ok((u, rep))
}

#[allow(clippy::too_many_arguments)]
fn solve_iterative(
    mesh: &TetMesh,
    el: &Stiffness,
    graph: &NodeGraph,
    colors: &[Vec<u32>],
    bc: &Dirichlet,
    f: &[f64],
    opts: &SolveOptions,
    mut rep: SolverReport,
    deadline_s: Option<f64>,
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<(Vec<f64>, SolverReport)> {
    let span = |a: f32| frac.0 + (frac.1 - frac.0) * a;
    let started = Instant::now();
    rep.method = "iterative".into();
    let t = Instant::now();
    let pattern = Pattern::new(graph, Layout::Full)?;
    let n = pattern.n_dof;
    let mut rhs = f.to_vec();
    let val = assemble(mesh, graph, colors, el, &pattern, bc, &mut rhs, progress, (span(0.0), span(0.15)))?;
    let mat = DofMatrix { layout: Layout::Full, n, ptr: pattern.ptr, idx: pattern.idx, val };
    rep.matrix_nnz = mat.nnz();
    rep.assembly_seconds = t.elapsed().as_secs_f64();
    progress.report(span(0.16), "Preparing the multigrid preconditioner");
    let t = Instant::now();
    let amg = crate::amg::Amg::new(&mat, &mesh.nodes, &bc.fixed).context("multigrid setup")?;
    rep.setup_seconds = t.elapsed().as_secs_f64();
    // matrix + hierarchy + work vectors; the measured process peak is higher
    // (setup temporaries, mesh, results): see `estimate_memory`
    rep.memory_bytes = (mat.nnz() as u64 * 12 + amg.bytes() + n as u64 * 8 * 8).max(estimate_memory(n / 3, mesh.tets.len()).iterative_bytes);
    if progress.cancelled() {
        return Err(Cancelled.into());
    }
    let t = Instant::now();
    let mut u = vec![0.0; n];
    let deadline = deadline_s.map(|d| started + std::time::Duration::from_secs_f64(d));
    let (iters, res) = pcg(&mat, &amg, &rhs, &mut u, opts.rel_tol, opts.max_iterations, deadline, progress, (span(0.2), span(1.0)))?;
    rep.iterations = iters;
    rep.relative_residual = res;
    rep.solve_seconds = t.elapsed().as_secs_f64();
    rep.detail = format!(
        "conjugate gradients with smoothed-aggregation multigrid ({} levels), {} iterations to residual {:.1e}",
        amg.levels(),
        iters,
        res
    );
    if !(res <= opts.rel_tol * 1.5) && res <= opts.rel_tol * 30.0 {
        // stalled at the round-off floor of this matrix, close to the target
        rep.detail.push_str(&format!(" (stalled at {res:.1e}, round-off floor)"));
    } else if !(res <= opts.rel_tol * 1.5) {
        bail!("the iterative solver did not converge (residual {res:.1e} after {iters} iterations)");
    }
    check_finite(&u)?;
    Ok((u, rep))
}

fn check_finite(u: &[f64]) -> Result<()> {
    if u.iter().any(|v| !v.is_finite()) {
        bail!("the solve produced invalid numbers: the part is probably not held in every direction");
    }
    Ok(())
}

pub fn norm2(x: &[f64]) -> f64 {
    use rayon::prelude::*;
    x.par_iter().map(|v| v * v).sum::<f64>().sqrt()
}

fn dotp(a: &[f64], b: &[f64]) -> f64 {
    use rayon::prelude::*;
    a.par_iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Preconditioned conjugate gradients. Returns (iterations, ‖r‖/‖f‖).
/// Converged means the *true* residual ‖f − K u‖/‖f‖ is below `tol`: the
/// recursively updated residual drifts on stiff problems, so CG restarts from
/// the true residual (at most 3 times, and only while that keeps helping).
#[allow(clippy::too_many_arguments)]
pub fn pcg(
    a: &DofMatrix,
    m: &crate::amg::Amg,
    f: &[f64],
    u: &mut [f64],
    tol: f64,
    max_it: usize,
    deadline: Option<Instant>,
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<(usize, f64)> {
    use rayon::prelude::*;
    let n = f.len();
    let fnorm = norm2(f);
    if fnorm == 0.0 {
        u.fill(0.0);
        return Ok((0, 0.0));
    }
    let mut r = vec![0.0; n];
    let mut z = vec![0.0; n];
    let mut p = vec![0.0; n];
    let mut q = vec![0.0; n];
    let mut it = 0;
    let start = 0.0f64;
    let mut true_res = f64::INFINITY;
    for _restart in 0..4 {
        a.mul(u, &mut r);
        r.par_iter_mut().zip(f).for_each(|(ri, fi)| *ri = fi - *ri);
        let prev = true_res;
        true_res = norm2(&r) / fnorm;
        // done, out of iterations, or stuck at the round-off floor
        if true_res <= tol || it >= max_it || true_res > 0.5 * prev {
            break;
        }
        m.apply(&r, &mut z);
        p.copy_from_slice(&z);
        let mut rz = dotp(&r, &z);
        let mut res = true_res;
        // aim a little lower than tol so the true residual also passes
        let target = 0.3 * tol;
        while it < max_it && res > target {
            if progress.cancelled() {
                return Err(Cancelled.into());
            }
            if deadline.is_some_and(|d| Instant::now() > d) {
                return Err(TooSlow.into());
            }
            a.mul(&p, &mut q);
            let pq = dotp(&p, &q);
            if !(pq > 0.0) {
                bail!("the part can still move: the supports do not hold it in every direction");
            }
            let alpha = rz / pq;
            u.par_iter_mut().zip(&p).for_each(|(ui, pi)| *ui += alpha * pi);
            r.par_iter_mut().zip(&q).for_each(|(ri, qi)| *ri -= alpha * qi);
            m.apply(&r, &mut z);
            let rz_new = dotp(&r, &z);
            let beta = rz_new / rz;
            rz = rz_new;
            p.par_iter_mut().zip(&z).for_each(|(pi, zi)| *pi = zi + beta * *pi);
            res = norm2(&r) / fnorm;
            it += 1;
            progress.metric("residual", it as u64, res);
            if it % 5 == 0 {
                let done = ((start - res.log10()) / (start - tol.log10())).clamp(0.0, 1.0) as f32;
                progress.report(frac.0 + (frac.1 - frac.0) * done, &format!("Solving iteratively: step {it}, error {res:.1e} (target {tol:.0e})"));
            }
        }
    }
    Ok((it, true_res))
}
