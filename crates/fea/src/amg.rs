//! Smoothed-aggregation algebraic multigrid (Vaněk, Mandel & Brezina 1996)
//! for 3D elasticity, used as the preconditioner of conjugate gradients
//! when the direct solver does not fit in memory.
//!
//! Why this preconditioner: for elasticity the slowest error components are
//! the six rigid-body motions of large pieces of the part. Smoothed
//! aggregation builds its coarse spaces from exactly those modes (computed
//! from the node coordinates), which gives mesh-independent iteration counts
//! on solid meshes — the method behind ML/MueLu (Trilinos) and PETSc GAMG.
//! Jacobi or incomplete-Cholesky preconditioners need iteration counts that
//! grow with mesh size (thousands at 1 M dofs).
//!
//! Details: node-block aggregation on the strength graph (θ below), the
//! tentative prolongator from a QR of the rigid-body modes per aggregate,
//! one damped-Jacobi prolongator smoothing step (ω = 4/(3λ_max)), Galerkin
//! coarse operators, Chebyshev(3) smoothing with a Jacobi inner
//! preconditioner (symmetric, so the V-cycle is a valid CG preconditioner),
//! and a dense Cholesky on the coarsest level.
//!
//! Tuning knobs for experiments (environment): `SIMLAB_AMG_THETA`
//! (strength threshold, default 0.02), `SIMLAB_AMG_DEG` (Chebyshev degree,
//! 3), `SIMLAB_AMG_RATIO` (smoothing interval λmax/ratio..1.1λmax, 20).
//!
//! Known weakness: on thin, badly shaped meshes (an STL-derived propeller,
//! 0.5 mm blades) it needs 400-1100 iterations instead of 40-70; the Auto
//! solver then switches to the direct solver (see `solver::solve`).

use crate::assembly::{DofMatrix, Layout};
use anyhow::{Result, bail};
use faer::Mat;
use rayon::prelude::*;
use simlab_core::Vec3;

fn env_or(k: &str, d: f64) -> f64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn theta() -> f64 {
    env_or("SIMLAB_AMG_THETA", 0.02)
}
const COARSE_MAX: usize = 3000;
fn cheb_degree() -> usize {
    env_or("SIMLAB_AMG_DEG", 3.0) as usize
}
fn cheb_ratio() -> f64 {
    env_or("SIMLAB_AMG_RATIO", 20.0)
}

/// Compressed-row matrix owned by a coarse level (or a prolongator).
struct Csr {
    n: usize,
    ncols: usize,
    ptr: Vec<usize>,
    idx: Vec<u32>,
    val: Vec<f64>,
}

trait Rows: Sync {
    fn n(&self) -> usize;
    fn row(&self, i: usize) -> (&[u32], &[f64]);
}

impl Rows for Csr {
    fn n(&self) -> usize {
        self.n
    }
    #[inline]
    fn row(&self, i: usize) -> (&[u32], &[f64]) {
        let (a, b) = (self.ptr[i], self.ptr[i + 1]);
        (&self.idx[a..b], &self.val[a..b])
    }
}

impl Rows for DofMatrix {
    fn n(&self) -> usize {
        self.n
    }
    #[inline]
    fn row(&self, i: usize) -> (&[u32], &[f64]) {
        let (a, b) = (self.ptr[i] as usize, self.ptr[i + 1] as usize);
        (&self.idx[a..b], &self.val[a..b])
    }
}

fn spmv<M: Rows + ?Sized>(a: &M, x: &[f64], y: &mut [f64]) {
    y.par_chunks_mut(512).enumerate().for_each(|(c, yc)| {
        for (k, yi) in yc.iter_mut().enumerate() {
            let (idx, val) = a.row(c * 512 + k);
            let mut s = 0.0;
            for (j, v) in idx.iter().zip(val) {
                s += v * x[*j as usize];
            }
            *yi = s;
        }
    });
}

enum LevelMat<'a> {
    Fine(&'a DofMatrix),
    Own(Csr),
}

impl LevelMat<'_> {
    fn rows(&self) -> &dyn RowsDyn {
        match self {
            LevelMat::Fine(m) => *m,
            LevelMat::Own(m) => m,
        }
    }
}

/// object-safe wrapper so levels can hold either matrix type
trait RowsDyn: Sync {
    fn n(&self) -> usize;
    fn row(&self, i: usize) -> (&[u32], &[f64]);
    fn mul(&self, x: &[f64], y: &mut [f64]);
}
impl<T: Rows> RowsDyn for T {
    fn n(&self) -> usize {
        Rows::n(self)
    }
    fn row(&self, i: usize) -> (&[u32], &[f64]) {
        Rows::row(self, i)
    }
    fn mul(&self, x: &[f64], y: &mut [f64]) {
        spmv(self, x, y)
    }
}
impl Rows for &dyn RowsDyn {
    fn n(&self) -> usize {
        RowsDyn::n(*self)
    }
    fn row(&self, i: usize) -> (&[u32], &[f64]) {
        RowsDyn::row(*self, i)
    }
}

struct Level<'a> {
    a: LevelMat<'a>,
    dinv: Vec<f64>,
    lmax: f64,
    /// prolongator n × nc and its transpose
    p: Csr,
    r: Csr,
}

pub struct Amg<'a> {
    levels: Vec<Level<'a>>,
    coarse_n: usize,
    coarse: Option<faer::linalg::solvers::Llt<f64>>,
    /// coarse dofs with no connection (zero rows), solved as zero
    coarse_dead: Vec<bool>,
}

impl<'a> Amg<'a> {
    /// `coords` are node positions (fine dof = 3·node + comp), `fixed` marks
    /// decoupled (prescribed) dofs.
    pub fn new(a: &'a DofMatrix, coords: &[Vec3], fixed: &[bool]) -> Result<Amg<'a>> {
        if a.layout != Layout::Full {
            bail!("multigrid needs the full matrix");
        }
        let n = a.n;
        // rigid-body modes about the centroid, rotations scaled by the size
        let nn = coords.len();
        let mut c = [0.0; 3];
        for p in coords {
            for k in 0..3 {
                c[k] += p[k] / nn as f64;
            }
        }
        let mut l = 0.0f64;
        for p in coords {
            for k in 0..3 {
                l = l.max((p[k] - c[k]).abs());
            }
        }
        let l = if l > 0.0 { l } else { 1.0 };
        let mut b = vec![[0.0f64; 6]; n];
        for (i, p) in coords.iter().enumerate() {
            let r = [(p[0] - c[0]) / l, (p[1] - c[1]) / l, (p[2] - c[2]) / l];
            // u = t + θ × r
            b[3 * i] = [1.0, 0.0, 0.0, 0.0, r[2], -r[1]];
            b[3 * i + 1] = [0.0, 1.0, 0.0, -r[2], 0.0, r[0]];
            b[3 * i + 2] = [0.0, 0.0, 1.0, r[1], -r[0], 0.0];
            for k in 0..3 {
                if fixed[3 * i + k] {
                    b[3 * i + k] = [0.0; 6];
                }
            }
        }
        let mut levels: Vec<Level<'a>> = Vec::new();
        let mut cur = LevelMat::Fine(a);
        let mut bs = 3usize;
        loop {
            let m = cur.rows();
            let nl = m.n();
            let diag: Vec<f64> = (0..nl)
                .into_par_iter()
                .map(|i| {
                    let (idx, val) = m.row(i);
                    idx.iter().zip(val).find(|(j, _)| **j as usize == i).map(|(_, v)| *v).unwrap_or(0.0)
                })
                .collect();
            let dinv: Vec<f64> = diag.iter().map(|&d| if d > 0.0 { 1.0 / d } else { 0.0 }).collect();
            if nl <= COARSE_MAX || levels.len() >= 12 {
                break;
            }
            let lmax = power_lmax(m, &dinv);
            let (p, bc) = build_prolongator(m, &dinv, lmax, &b, bs);
            if p.ncols == 0 || p.ncols * 10 > nl * 9 {
                // coarsening stalled: stop here and solve this level directly if small enough
                if nl > 20 * COARSE_MAX {
                    bail!("multigrid coarsening stalled at {nl} unknowns");
                }
                break;
            }
            let r = transpose(&p);
            let ac = galerkin(m, &p, &r);
            levels.push(Level { a: cur, dinv, lmax, p, r });
            cur = LevelMat::Own(ac);
            b = bc;
            bs = 6;
        }
        // coarsest: dense Cholesky
        let m = cur.rows();
        let nc = m.n();
        let mut dense = Mat::<f64>::zeros(nc, nc);
        let mut dead = vec![false; nc];
        for i in 0..nc {
            let (idx, val) = m.row(i);
            let mut has = false;
            for (j, v) in idx.iter().zip(val) {
                dense[(i, *j as usize)] += *v;
                if *j as usize == i && *v != 0.0 {
                    has = true;
                }
            }
            if !has {
                dead[i] = true;
                dense[(i, i)] = 1.0;
            }
        }
        let coarse = if nc > 0 {
            Some(dense.llt(faer::Side::Lower).map_err(|_| anyhow::anyhow!("the part can still move: the supports do not hold it in every direction"))?)
        } else {
            None
        };
        // keep the coarsest matrix alive for its size only (not needed further)
        drop(cur);
        Ok(Amg { levels, coarse_n: nc, coarse, coarse_dead: dead })
    }

    pub fn levels(&self) -> usize {
        self.levels.len() + 1
    }

    pub fn bytes(&self) -> u64 {
        let mut b = 0u64;
        for l in &self.levels {
            if let LevelMat::Own(m) = &l.a {
                b += m.val.len() as u64 * 12;
            }
            b += (l.p.val.len() + l.r.val.len()) as u64 * 12 + l.dinv.len() as u64 * 8 * 4;
        }
        b + (self.coarse_n * self.coarse_n) as u64 * 8
    }

    /// z = M⁻¹ r (one V-cycle)
    pub fn apply(&self, r: &[f64], z: &mut [f64]) {
        self.vcycle(0, r, z);
    }

    fn vcycle(&self, lev: usize, b: &[f64], x: &mut [f64]) {
        if lev == self.levels.len() {
            x.copy_from_slice(b);
            if let Some(llt) = &self.coarse {
                let mut col = faer::MatMut::from_column_major_slice_mut(x, self.coarse_n, 1);
                use faer::linalg::solvers::SolveCore;
                llt.solve_in_place_with_conj(faer::Conj::No, col.as_mut());
            }
            for (i, d) in self.coarse_dead.iter().enumerate() {
                if *d {
                    x[i] = 0.0;
                }
            }
            return;
        }
        let l = &self.levels[lev];
        let a = l.a.rows();
        let n = a.n();
        x.fill(0.0);
        chebyshev(a, &l.dinv, l.lmax, b, x, true);
        let mut res = vec![0.0; n];
        a.mul(x, &mut res);
        res.par_iter_mut().zip(b).for_each(|(r, bi)| *r = bi - *r);
        let nc = l.p.ncols;
        let mut bc = vec![0.0; nc];
        spmv(&l.r, &res, &mut bc);
        let mut xc = vec![0.0; nc];
        self.vcycle(lev + 1, &bc, &mut xc);
        spmv(&l.p, &xc, &mut res);
        x.par_iter_mut().zip(&res).for_each(|(xi, ci)| *xi += ci);
        chebyshev(a, &l.dinv, l.lmax, b, x, false);
    }
}

/// Chebyshev smoothing of A x = b with Jacobi inner preconditioning on the
/// interval [λ/ratio, 1.1 λ] of D⁻¹A.
fn chebyshev(a: &dyn RowsDyn, dinv: &[f64], lmax: f64, b: &[f64], x: &mut [f64], x_zero: bool) {
    let n = b.len();
    let hi = 1.1 * lmax;
    let lo = lmax / cheb_ratio();
    let deg = cheb_degree();
    let theta = 0.5 * (hi + lo);
    let delta = 0.5 * (hi - lo);
    let sigma = theta / delta;
    let mut rho = 1.0 / sigma;
    let mut r = vec![0.0; n];
    if x_zero {
        r.par_iter_mut().zip(b).zip(dinv).for_each(|((ri, bi), di)| *ri = bi * di);
    } else {
        a.mul(x, &mut r);
        r.par_iter_mut().zip(b).zip(dinv).for_each(|((ri, bi), di)| *ri = (bi - *ri) * di);
    }
    let mut d: Vec<f64> = r.par_iter().map(|v| v / theta).collect();
    let mut ad = vec![0.0; n];
    for k in 0..deg {
        x.par_iter_mut().zip(&d).for_each(|(xi, di)| *xi += di);
        if k + 1 == deg {
            break;
        }
        a.mul(&d, &mut ad);
        let rho_new = 1.0 / (2.0 * sigma - rho);
        let (c1, c2) = (rho_new * rho, 2.0 * rho_new / delta);
        r.par_iter_mut().zip(&ad).zip(dinv).zip(d.par_iter_mut()).for_each(|(((ri, adi), di), dd)| {
            *ri -= adi * di;
            *dd = c1 * *dd + c2 * *ri;
        });
        rho = rho_new;
    }
}

/// Largest eigenvalue of D⁻¹A by power iteration (slightly over-estimated).
fn power_lmax(a: &dyn RowsDyn, dinv: &[f64]) -> f64 {
    let n = a.n();
    let mut x: Vec<f64> = (0..n).map(|i| ((i as f64 * 0.618_033_988_75).fract() - 0.5) * dinv[i].signum().abs()).collect();
    let mut y = vec![0.0; n];
    let mut lam = 1.0;
    for _ in 0..25 {
        let nx = crate::solver::norm2(&x);
        if nx == 0.0 {
            return 1.0;
        }
        x.par_iter_mut().for_each(|v| *v /= nx);
        a.mul(&x, &mut y);
        y.par_iter_mut().zip(dinv).for_each(|(v, d)| *v *= d);
        lam = crate::solver::norm2(&y);
        std::mem::swap(&mut x, &mut y);
    }
    lam * 1.05
}

/// Aggregation + tentative prolongator (QR of the near-null space per
/// aggregate) + one Jacobi smoothing step. Returns P and the coarse
/// near-null space.
fn build_prolongator(a: &dyn RowsDyn, dinv: &[f64], lmax: f64, b: &[[f64; 6]], bs: usize) -> (Csr, Vec<[f64; 6]>) {
    let n = a.n();
    let nn = n / bs;
    // node-block strength graph
    let block_norm2 = |i: usize| -> Vec<(u32, f64)> {
        let mut out: Vec<(u32, f64)> = Vec::new();
        for r in 0..bs {
            let (idx, val) = a.row(bs * i + r);
            for (j, v) in idx.iter().zip(val) {
                let jn = *j / bs as u32;
                match out.last_mut() {
                    Some(last) if last.0 == jn => last.1 += v * v,
                    _ => {
                        if let Some(e) = out.iter_mut().find(|e| e.0 == jn) {
                            e.1 += v * v;
                        } else {
                            out.push((jn, v * v));
                        }
                    }
                }
            }
        }
        out
    };
    let theta = theta();
    let rows: Vec<Vec<(u32, f64)>> = (0..nn).into_par_iter().map(block_norm2).collect();
    let self_norm: Vec<f64> = rows.iter().enumerate().map(|(i, r)| r.iter().find(|e| e.0 as usize == i).map(|e| e.1.sqrt()).unwrap_or(0.0)).collect();
    // isolated = no off-diagonal coupling (fully prescribed nodes)
    let strong: Vec<Vec<u32>> = rows
        .par_iter()
        .enumerate()
        .map(|(i, r)| {
            r.iter()
                .filter(|e| e.0 as usize != i && e.1 > 0.0 && e.1.sqrt() > theta * (self_norm[i] * self_norm[e.0 as usize]).sqrt())
                .map(|e| e.0)
                .collect()
        })
        .collect();
    drop(rows);
    const NONE: u32 = u32::MAX;
    let mut agg = vec![NONE; nn];
    let mut nagg = 0u32;
    // phase 1: roots whose whole neighbourhood is free
    for i in 0..nn {
        if agg[i] != NONE || strong[i].is_empty() {
            continue;
        }
        if strong[i].iter().all(|&j| agg[j as usize] == NONE) {
            agg[i] = nagg;
            for &j in &strong[i] {
                agg[j as usize] = nagg;
            }
            nagg += 1;
        }
    }
    // phase 2: join a neighbouring aggregate
    let snapshot = agg.clone();
    for i in 0..nn {
        if agg[i] != NONE || strong[i].is_empty() {
            continue;
        }
        if let Some(&j) = strong[i].iter().find(|&&j| snapshot[j as usize] != NONE) {
            agg[i] = snapshot[j as usize];
        }
    }
    // phase 3: leftovers form their own aggregates
    for i in 0..nn {
        if agg[i] != NONE || strong[i].is_empty() {
            continue;
        }
        agg[i] = nagg;
        for &j in &strong[i] {
            if agg[j as usize] == NONE {
                agg[j as usize] = nagg;
            }
        }
        nagg += 1;
    }
    let nagg = nagg as usize;
    // members of each aggregate
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); nagg];
    for (i, &g) in agg.iter().enumerate() {
        if g != NONE {
            members[g as usize].push(i as u32);
        }
    }
    // QR per aggregate (modified Gram-Schmidt)
    let qr: Vec<(Vec<[f64; 6]>, [[f64; 6]; 6])> = members
        .par_iter()
        .map(|mem| {
            let rows: Vec<usize> = mem.iter().flat_map(|&i| (0..bs).map(move |r| bs * i as usize + r)).collect();
            let m = rows.len();
            let mut q: Vec<[f64; 6]> = rows.iter().map(|&d| b[d]).collect();
            let mut rr = [[0.0; 6]; 6];
            let mut col_scale = [0.0f64; 6];
            for k in 0..6 {
                col_scale[k] = (0..m).map(|i| q[i][k] * q[i][k]).sum::<f64>().sqrt();
            }
            for k in 0..6 {
                for j in 0..k {
                    let d: f64 = (0..m).map(|i| q[i][j] * q[i][k]).sum();
                    rr[j][k] = d;
                    for i in 0..m {
                        q[i][k] -= d * q[i][j];
                    }
                }
                let nrm = (0..m).map(|i| q[i][k] * q[i][k]).sum::<f64>().sqrt();
                if nrm > 1e-10 * col_scale[k].max(1e-300) && nrm > 0.0 {
                    rr[k][k] = nrm;
                    for i in 0..m {
                        q[i][k] /= nrm;
                    }
                } else {
                    rr[k][k] = 0.0;
                    for i in 0..m {
                        q[i][k] = 0.0;
                    }
                }
            }
            (q, rr)
        })
        .collect();
    // tentative prolongator rows: dof -> (aggregate, 6 values)
    let mut tent: Vec<Option<(u32, [f64; 6])>> = vec![None; n];
    for (g, mem) in members.iter().enumerate() {
        let (q, _) = &qr[g];
        let mut k = 0;
        for &i in mem {
            for r in 0..bs {
                tent[bs * i as usize + r] = Some((g as u32, q[k]));
                k += 1;
            }
        }
    }
    let mut bc = vec![[0.0f64; 6]; 6 * nagg];
    for (g, (_, rr)) in qr.iter().enumerate() {
        for k in 0..6 {
            bc[6 * g + k] = rr[k];
        }
    }
    // P = (I − ω D⁻¹A) P_tent
    let omega = 4.0 / (3.0 * lmax);
    let prow: Vec<Vec<(u32, [f64; 6])>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut acc: Vec<(u32, [f64; 6])> = Vec::with_capacity(16);
            let add = |acc: &mut Vec<(u32, [f64; 6])>, g: u32, v: &[f64; 6], s: f64| {
                if let Some(e) = acc.iter_mut().find(|e| e.0 == g) {
                    for k in 0..6 {
                        e.1[k] += s * v[k];
                    }
                } else {
                    let mut w = [0.0; 6];
                    for k in 0..6 {
                        w[k] = s * v[k];
                    }
                    acc.push((g, w));
                }
            };
            if let Some((g, v)) = &tent[i] {
                add(&mut acc, *g, v, 1.0);
            }
            if dinv[i] != 0.0 {
                let (idx, val) = a.row(i);
                let s = -omega * dinv[i];
                for (j, aij) in idx.iter().zip(val) {
                    if let Some((g, v)) = &tent[*j as usize] {
                        add(&mut acc, *g, v, s * aij);
                    }
                }
            }
            acc.sort_unstable_by_key(|e| e.0);
            acc
        })
        .collect();
    let mut ptr = Vec::with_capacity(n + 1);
    ptr.push(0usize);
    let mut tot = 0;
    for r in &prow {
        tot += 6 * r.len();
        ptr.push(tot);
    }
    let mut idx = Vec::with_capacity(tot);
    let mut val = Vec::with_capacity(tot);
    for r in &prow {
        for (g, v) in r {
            for k in 0..6 {
                idx.push(6 * g + k as u32);
                val.push(v[k]);
            }
        }
    }
    (Csr { n, ncols: 6 * nagg, ptr, idx, val }, bc)
}

fn transpose(p: &Csr) -> Csr {
    let mut cnt = vec![0usize; p.ncols + 1];
    for &j in &p.idx {
        cnt[j as usize + 1] += 1;
    }
    for j in 0..p.ncols {
        cnt[j + 1] += cnt[j];
    }
    let mut fill = cnt.clone();
    let mut idx = vec![0u32; p.idx.len()];
    let mut val = vec![0.0; p.idx.len()];
    for i in 0..p.n {
        for k in p.ptr[i]..p.ptr[i + 1] {
            let j = p.idx[k] as usize;
            idx[fill[j]] = i as u32;
            val[fill[j]] = p.val[k];
            fill[j] += 1;
        }
    }
    Csr { n: p.ncols, ncols: p.n, ptr: cnt, idx, val }
}

/// Sparse product C = A · B (rows in parallel, dense accumulator per thread).
fn spgemm(a: &dyn RowsDyn, b: &Csr) -> Csr {
    let nc = b.ncols;
    let rows: Vec<(Vec<u32>, Vec<f64>)> = (0..a.n())
        .into_par_iter()
        .map_init(
            || (vec![0.0f64; nc], vec![u32::MAX; nc], Vec::<u32>::new()),
            |(acc, mark, list), i| {
                list.clear();
                let (ai, av) = a.row(i);
                for (k, v) in ai.iter().zip(av) {
                    let k = *k as usize;
                    for p in b.ptr[k]..b.ptr[k + 1] {
                        let j = b.idx[p] as usize;
                        if mark[j] != i as u32 {
                            mark[j] = i as u32;
                            acc[j] = 0.0;
                            list.push(j as u32);
                        }
                        acc[j] += v * b.val[p];
                    }
                }
                list.sort_unstable();
                let vals: Vec<f64> = list.iter().map(|&j| acc[j as usize]).collect();
                (list.clone(), vals)
            },
        )
        .collect();
    let mut ptr = Vec::with_capacity(a.n() + 1);
    ptr.push(0);
    let mut t = 0;
    for r in &rows {
        t += r.0.len();
        ptr.push(t);
    }
    let mut idx = Vec::with_capacity(t);
    let mut val = Vec::with_capacity(t);
    for (i, v) in rows {
        idx.extend(i);
        val.extend(v);
    }
    Csr { n: a.n(), ncols: nc, ptr, idx, val }
}

/// A_c = Pᵀ A P
fn galerkin(a: &dyn RowsDyn, p: &Csr, r: &Csr) -> Csr {
    let ap = spgemm(a, p);
    let rr: &dyn RowsDyn = r;
    let mut c = spgemm(rr, &ap);
    // symmetrize round-off
    let ct = transpose(&c);
    for i in 0..c.n {
        for k in c.ptr[i]..c.ptr[i + 1] {
            let j = c.idx[k] as usize;
            // find (j, i) in ct row i == column i of c... ct row i holds c[*, i]
            let row = &ct.idx[ct.ptr[i]..ct.ptr[i + 1]];
            if let Ok(pos) = row.binary_search(&(j as u32)) {
                c.val[k] = 0.5 * (c.val[k] + ct.val[ct.ptr[i] + pos]);
            }
        }
    }
    c
}
