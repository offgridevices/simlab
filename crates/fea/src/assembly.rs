//! Parallel, allocation-light assembly of the global stiffness matrix.
//!
//! The sparsity pattern comes straight from the node graph (3×3 blocks), so
//! the position of every element entry is found with one binary search per
//! node pair; elements are coloured so that no two elements processed at the
//! same time share a node, which makes the scatter conflict-free without
//! locks or atomics. The element matrices are never stored.

use crate::elem::Stiffness;
use crate::topo::NodeGraph;
use anyhow::{Result, bail};
use rayon::prelude::*;
use simlab_core::{Cancelled, Progress, TetMesh};

/// Which entries of the symmetric matrix are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// lower triangle, compressed columns (what the Cholesky factorization reads)
    Lower,
    /// both triangles, compressed rows (iterative solver: fast parallel products)
    Full,
}

/// Sparse symmetric matrix over dofs (dof = 3·node + component).
pub struct DofMatrix {
    pub layout: Layout,
    pub n: usize,
    /// column pointers (Lower) or row pointers (Full)
    pub ptr: Vec<u32>,
    pub idx: Vec<u32>,
    pub val: Vec<f64>,
}

impl DofMatrix {
    pub fn nnz(&self) -> usize {
        self.val.len()
    }

    /// y = A x
    pub fn mul(&self, x: &[f64], y: &mut [f64]) {
        match self.layout {
            Layout::Full => {
                y.par_chunks_mut(256).enumerate().for_each(|(ci, yc)| {
                    for (k, yi) in yc.iter_mut().enumerate() {
                        let i = ci * 256 + k;
                        let (a, b) = (self.ptr[i] as usize, self.ptr[i + 1] as usize);
                        let mut s = 0.0;
                        for p in a..b {
                            s += self.val[p] * x[self.idx[p] as usize];
                        }
                        *yi = s;
                    }
                });
            }
            Layout::Lower => {
                y.fill(0.0);
                for j in 0..self.n {
                    let (a, b) = (self.ptr[j] as usize, self.ptr[j + 1] as usize);
                    let mut s = 0.0;
                    for p in a..b {
                        let i = self.idx[p] as usize;
                        let v = self.val[p];
                        if i == j {
                            s += v * x[j];
                        } else {
                            s += v * x[i];
                            y[i] += v * x[j];
                        }
                    }
                    y[j] += s;
                }
            }
        }
    }

    pub fn diagonal(&self) -> Vec<f64> {
        let mut d = vec![0.0; self.n];
        for j in 0..self.n {
            for p in self.ptr[j] as usize..self.ptr[j + 1] as usize {
                if self.idx[p] as usize == j {
                    d[j] = self.val[p];
                }
            }
        }
        d
    }
}

/// Prescribed displacement components (dof = 3·node + component).
#[derive(Clone, Debug, Default)]
pub struct Dirichlet {
    pub fixed: Vec<bool>,
    pub value: Vec<f64>,
}

impl Dirichlet {
    pub fn new(n_dof: usize) -> Self {
        Dirichlet { fixed: vec![false; n_dof], value: vec![0.0; n_dof] }
    }
    pub fn set(&mut self, dof: usize, value: f64) {
        self.fixed[dof] = true;
        self.value[dof] = value;
    }
    pub fn count(&self) -> usize {
        self.fixed.iter().filter(|&&f| f).count()
    }
}

/// The matrix pattern for a mesh, built once from the node graph.
pub struct Pattern {
    pub layout: Layout,
    pub n_dof: usize,
    pub ptr: Vec<u32>,
    pub idx: Vec<u32>,
    /// position of node i inside its own adjacency row
    self_pos: Vec<u32>,
}

impl Pattern {
    pub fn new(g: &NodeGraph, layout: Layout) -> Result<Pattern> {
        let n = g.n();
        let n_dof = 3 * n;
        let self_pos: Vec<u32> = (0..n).map(|i| g.row(i).binary_search(&(i as u32)).expect("node graph row lacks its node") as u32).collect();
        // entries per dof row/column
        let mut ptr = Vec::with_capacity(n_dof + 1);
        ptr.push(0u64);
        let mut total = 0u64;
        for i in 0..n {
            let deg = g.row(i).len() as u64;
            let m = deg - 1 - self_pos[i] as u64; // neighbours above i
            for c in 0..3u64 {
                total += match layout {
                    Layout::Full => 3 * deg,
                    Layout::Lower => (3 - c) + 3 * m,
                };
                ptr.push(total);
            }
        }
        if total >= u32::MAX as u64 {
            bail!("the mesh is too large for one matrix ({} entries)", total);
        }
        let ptr: Vec<u32> = ptr.into_iter().map(|p| p as u32).collect();
        let mut idx = vec![0u32; total as usize];
        // fill indices, node by node in parallel
        let chunks: Vec<(usize, &mut [u32])> = {
            let mut v = Vec::with_capacity(n);
            let mut rest: &mut [u32] = &mut idx;
            for i in 0..n {
                let len = (ptr[3 * i + 3] - ptr[3 * i]) as usize;
                let (a, b) = rest.split_at_mut(len);
                v.push((i, a));
                rest = b;
            }
            v
        };
        chunks.into_par_iter().for_each(|(i, out)| {
            let row = g.row(i);
            let sp = self_pos[i] as usize;
            let mut w = 0;
            for c in 0..3usize {
                match layout {
                    Layout::Full => {
                        for &j in row {
                            for cc in 0..3 {
                                out[w] = 3 * j + cc;
                                w += 1;
                            }
                        }
                    }
                    Layout::Lower => {
                        for r in c..3 {
                            out[w] = (3 * i + r) as u32;
                            w += 1;
                        }
                        for &j in &row[sp + 1..] {
                            for r in 0..3 {
                                out[w] = 3 * j + r;
                                w += 1;
                            }
                        }
                    }
                }
            }
        });
        Ok(Pattern { layout, n_dof, ptr, idx, self_pos })
    }

    /// Position of entry (row node `i`, comp `r`; col node `j`, comp `c`).
    /// Lower: requires (i, r) ≥ (j, c). `ki` = index of `i` in node j's row (Lower)
    /// or of `j` in node i's row (Full).
    #[inline]
    fn pos(&self, i: usize, r: usize, j: usize, c: usize, k: usize) -> usize {
        match self.layout {
            Layout::Full => self.ptr[3 * i + r] as usize + 3 * k + c,
            Layout::Lower => {
                let base = self.ptr[3 * j + c] as usize;
                if i == j { base + (r - c) } else { base + (3 - c) + 3 * (k - self.self_pos[j] as usize - 1) + r }
            }
        }
    }
}

struct SyncPtr(*mut f64);
unsafe impl Sync for SyncPtr {}
unsafe impl Send for SyncPtr {}
impl SyncPtr {
    #[inline]
    unsafe fn add(&self, i: usize, v: f64) {
        unsafe { *self.0.add(i) += v };
    }
}

/// Assembles K (with prescribed dofs decoupled: their rows and columns are
/// zeroed, the diagonal is set to the mean free diagonal and `f` is changed
/// so that the solution takes the prescribed value) into `pattern`'s layout.
/// `f` holds the external load on entry.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    mesh: &TetMesh,
    graph: &NodeGraph,
    colors: &[Vec<u32>],
    st: &Stiffness,
    pattern: &Pattern,
    bc: &Dirichlet,
    f: &mut [f64],
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<Vec<f64>> {
    let mut val = vec![0.0f64; pattern.idx.len()];
    let vp = SyncPtr(val.as_mut_ptr());
    let fp = SyncPtr(f.as_mut_ptr());
    let total = mesh.tets.len().max(1);
    let mut done = 0usize;
    let bad = std::sync::atomic::AtomicUsize::new(usize::MAX);
    for (ci, color) in colors.iter().enumerate() {
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        color.par_iter().for_each_init(
            || Box::new([0.0f64; 900]),
            |ke, &e| {
                let e = e as usize;
                let vol = st.element(mesh, e, ke);
                if !(vol > 0.0) {
                    bad.fetch_min(e, std::sync::atomic::Ordering::Relaxed);
                }
                let t = &mesh.tets[e];
                for p in 0..10 {
                    let gi = t[p] as usize;
                    for q in 0..10 {
                        let gj = t[q] as usize;
                        let (row_n, col_n) = (gi, gj);
                        let k = match pattern.layout {
                            Layout::Lower => {
                                if gi < gj {
                                    continue;
                                }
                                graph.row(gj).binary_search(&(gi as u32)).unwrap()
                            }
                            Layout::Full => graph.row(gi).binary_search(&(gj as u32)).unwrap(),
                        };
                        for r in 0..3 {
                            let rd = 3 * row_n + r;
                            for c in 0..3 {
                                let cd = 3 * col_n + c;
                                let kv = ke[(3 * p + r) * 30 + 3 * q + c];
                                let (rf, cf) = (bc.fixed[rd], bc.fixed[cd]);
                                if !rf && cf && bc.value[cd] != 0.0 {
                                    // move the prescribed displacement to the right-hand side
                                    // (every (row, col) pair of this node pair is visited here)
                                    unsafe { fp.add(rd, -kv * bc.value[cd]) };
                                }
                                if pattern.layout == Layout::Lower {
                                    if rf && !cf && bc.value[rd] != 0.0 && gi != gj {
                                        // symmetric counterpart lives above the diagonal
                                        unsafe { fp.add(cd, -kv * bc.value[rd]) };
                                    }
                                    if gi == gj && r < c {
                                        continue;
                                    }
                                }
                                if rf || cf {
                                    continue;
                                }
                                unsafe { vp.add(pattern.pos(row_n, r, col_n, c, k), kv) };
                            }
                        }
                    }
                }
            },
        );
        done += color.len();
        progress.report(frac.0 + (frac.1 - frac.0) * done as f32 / total as f32, &format!("Building the stiffness matrix ({} of {} colour groups)", ci + 1, colors.len()));
    }
    let b = bad.load(std::sync::atomic::Ordering::Relaxed);
    if b != usize::MAX {
        bail!("element {b} is inverted or flat; the mesh is broken");
    }
    // decoupled prescribed rows
    let mut dsum = 0.0;
    let mut dn = 0usize;
    let diag_pos = |d: usize| -> usize {
        let i = d / 3;
        let r = d % 3;
        let k = pattern.self_pos[i] as usize;
        pattern.pos(i, r, i, r, k)
    };
    for d in 0..pattern.n_dof {
        if !bc.fixed[d] {
            dsum += val[diag_pos(d)];
            dn += 1;
        }
    }
    let dscale = if dn > 0 { dsum / dn as f64 } else { 1.0 };
    for d in 0..pattern.n_dof {
        if bc.fixed[d] {
            val[diag_pos(d)] = dscale;
            f[d] = dscale * bc.value[d];
        }
    }
    Ok(val)
}

/// Internal nodal forces of the elements touching `nodes_of_interest` (Σ Kₑuₑ
/// for a linear solve; see [`Stiffness::element_force`] for a nonlinear
/// one). Used for support reactions and Newton residuals. Returns a
/// full-length vector.
pub fn internal_forces(mesh: &TetMesh, st: &Stiffness, u: &[f64], touching: &[bool]) -> Vec<f64> {
    let elems: Vec<usize> = (0..mesh.tets.len()).filter(|&e| mesh.tets[e].iter().any(|&v| touching[v as usize])).collect();
    let parts: Vec<(usize, [f64; 30])> = elems.par_iter().map(|&e| (e, st.element_force(mesh, e, u))).collect();
    let mut f = vec![0.0; u.len()];
    for (e, fe) in parts {
        let t = &mesh.tets[e];
        for i in 0..30 {
            f[3 * t[i / 3] as usize + i % 3] += fe[i];
        }
    }
    f
}
