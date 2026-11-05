//! CalculiX reference runs: write the same problem as a `.inp` deck, read
//! the `.frd` results back, and compare node by node.
//!
//! The deck uses SI units, C3D10 elements with the same node numbering
//! (1-based), `*ORIENTATION` + `*ELASTIC, TYPE=ENGINEERING CONSTANTS` for a
//! printed (transversely isotropic) material, the same fixed nodes (or the
//! same 3-2-1 nodes for a free-flying part), and loads either as CalculiX's
//! own cards (`*DLOAD` P for pressure, GRAV, CENTRIF; `*CLOAD` for force,
//! motor and air tractions and the inertia-relief forces) or entirely as the
//! nodal forces SimLab computed (`native_loads = false`, which isolates the
//! solver from load integration).
//!
//! A geometrically nonlinear deck (`nlgeom`) is the problem SimLab's
//! stiffening solve answers: total Lagrangian, CENTRIF on the deformed
//! shape, every other load fixed in size and direction (so pressure goes in
//! as nodal forces: CalculiX's `P` would follow the faces as they turn).
//!
//! Run CalculiX with [`ccx_env`]: its multi-threaded SPOOLES solver can
//! return a wrong answer on real meshes (on the E-v5 propeller its own nodal
//! forces missed the loads by 40× the largest load, next to the held hub);
//! the single-threaded one is exact to round-off. [`equilibrium_error`]
//! checks any CalculiX result the same way.

use crate::strength::Prepared;
use anyhow::{Context, Result, bail};
use simlab_core::{TetMesh, Vec3, norm};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct InpOptions {
    pub heading: String,
    /// use CalculiX's own pressure / gravity / centrifugal cards
    pub native_loads: bool,
    /// request reactions (RF) in the results too
    pub write_reactions: bool,
    /// geometrically nonlinear step (`*STEP, NLGEOM`): stress stiffening and
    /// spin softening come out of the large-deformation solve, the
    /// counterpart of SimLab's stress-stiffening passes. Off: one linear step.
    pub nlgeom: bool,
    /// `*STATIC, SOLVER=...` (e.g. "ITERATIVE CHOLESKY"); None = CalculiX's
    /// default direct solver (SPOOLES in the reference image)
    pub solver: Option<String>,
}

impl Default for InpOptions {
    fn default() -> Self {
        InpOptions { heading: "SimLab reference run".into(), native_loads: true, write_reactions: true, nlgeom: false, solver: None }
    }
}

/// Environment for running `ccx`: element work on `threads` cores, the
/// equation solver (SPOOLES) on one. CalculiX's multi-threaded SPOOLES gave
/// a solution whose own nodal forces were 2 N off where the loads are
/// 0.05 N (E-v5 propeller, 553k equations, 12 threads): stresses next to
/// the hold 40 % off and the reaction on one side of the hub 8 % off,
/// while the tip looked right. The serial solver matched SimLab to 3e-6 of
/// the peak at every node on the same deck, for ~15 s more.
pub fn ccx_env(threads: usize) -> Vec<(String, String)> {
    let t = threads.max(1).to_string();
    vec![("OMP_NUM_THREADS".into(), t.clone()), ("CCX_NPROC_STIFFNESS".into(), t.clone()), ("CCX_NPROC_RESULTS".into(), t), ("CCX_NPROC_EQUATION_SOLVER".into(), "1".into())]
}

/// Writes a CalculiX deck for a prepared strength problem. Numbers are
/// written with 13 significant digits (CalculiX reads at most 20 characters
/// per number).
pub fn write_inp<W: Write>(w: &mut W, mesh: &TetMesh, prep: &Prepared, opts: &InpOptions) -> Result<()> {
    let el = &prep.elastic;
    writeln!(w, "*HEADING\n{}", opts.heading.replace('\n', " "))?;
    writeln!(w, "** units: m, N, Pa, kg/m^3. Written by simlab-fea.")?;
    writeln!(w, "*NODE, NSET=NALL")?;
    for (i, p) in mesh.nodes.iter().enumerate() {
        writeln!(w, "{}, {:.12e}, {:.12e}, {:.12e}", i + 1, p[0], p[1], p[2])?;
    }
    writeln!(w, "*ELEMENT, TYPE=C3D10, ELSET=EALL")?;
    for (e, t) in mesh.tets.iter().enumerate() {
        write!(w, "{}", e + 1)?;
        for &v in t {
            write!(w, ", {}", v + 1)?;
        }
        writeln!(w)?;
    }
    writeln!(w, "*MATERIAL, NAME=PRINTED")?;
    if el.is_isotropic() {
        writeln!(w, "*ELASTIC\n{:.10e}, {:.10}", el.e_xy, el.nu_xy)?;
    } else {
        // E1 E2 E3 ν12 ν13 ν23 G12 G13 / G23 ; local 3 = print direction
        writeln!(w, "*ELASTIC, TYPE=ENGINEERING CONSTANTS")?;
        writeln!(
            w,
            "{:.10e}, {:.10e}, {:.10e}, {:.10}, {:.10}, {:.10}, {:.10e}, {:.10e},",
            el.e_xy, el.e_xy, el.e_z, el.nu_xy, el.nu_xz, el.nu_xz, el.g_xy, el.g_xz
        )?;
        writeln!(w, "{:.10e}, 0.", el.g_xz)?;
    }
    writeln!(w, "*DENSITY\n{:.10e}", el.density)?;
    if el.is_isotropic() {
        writeln!(w, "*SOLID SECTION, ELSET=EALL, MATERIAL=PRINTED")?;
    } else {
        let [e1, e2, _] = el.frame;
        writeln!(w, "*ORIENTATION, NAME=PRINT, SYSTEM=RECTANGULAR")?;
        writeln!(w, "{:.15}, {:.15}, {:.15}, {:.15}, {:.15}, {:.15}", e1[0], e1[1], e1[2], e2[0], e2[1], e2[2])?;
        writeln!(w, "*SOLID SECTION, ELSET=EALL, MATERIAL=PRINTED, ORIENTATION=PRINT")?;
    }
    writeln!(w, "*BOUNDARY")?;
    let n = mesh.nodes.len();
    for i in 0..n {
        for k in 0..3 {
            let d = 3 * i + k;
            if prep.bc.fixed[d] {
                writeln!(w, "{}, {}, {}, {:.12e}", i + 1, k + 1, k + 1, prep.bc.value[d])?;
            }
        }
    }
    writeln!(w, "{}", if opts.nlgeom { "*STEP, NLGEOM" } else { "*STEP" })?;
    match &opts.solver {
        Some(sv) => writeln!(w, "*STATIC, SOLVER={sv}")?,
        None => writeln!(w, "*STATIC")?,
    }
    if opts.nlgeom {
        // the load in four steps to start with (CalculiX grows or shrinks
        // them as it goes): the whole spin in one Newton solve sometimes
        // diverged, and the halving that follows costs more than this
        writeln!(w, "0.25, 1.0")?;
    }
    if opts.nlgeom {
        // Newton converges on the displacement: CalculiX's default force
        // test compares the largest nodal residual with the AVERAGE nodal
        // force, which on a spinning part (big loads at the tips, tiny ones
        // near the hub) stalls at a few % while the corrections are already
        // 1e-5 of the step and the stresses settled. So: corrections under
        // 1e-4 of the displacement, residual under 10 % of the average force,
        // and room for more iterations before CalculiX calls it diverging
        // (I_0) or too slow (I_R) and halves the step.
        writeln!(w, "*CONTROLS, PARAMETERS=FIELD\n0.1, 1.e-4")?;
        writeln!(w, "*CONTROLS, PARAMETERS=TIME INCREMENTATION\n8, 16, 9, 40, 10, 4, 0, 40")?;
    }
    // pressure as CalculiX's P card follows the faces in a nonlinear step;
    // SimLab's does not, so a nonlinear deck gets it as nodal forces
    let dead_pressure = opts.nlgeom && !prep.parts.pressure.is_empty();
    let nodal: Vec<f64> = if opts.native_loads {
        (0..3 * n).map(|d| prep.parts.surface[d] + prep.parts.inertia_nodal[d] + if dead_pressure { prep.parts.pressure_nodal[d] } else { 0.0 }).collect()
    } else {
        prep.f.clone()
    };
    let fmax = nodal.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let mut wrote_cload = false;
    for i in 0..n {
        for k in 0..3 {
            let v = nodal[3 * i + k];
            if v != 0.0 && v.abs() > 1e-14 * fmax {
                if !wrote_cload {
                    writeln!(w, "*CLOAD")?;
                    wrote_cload = true;
                }
                writeln!(w, "{}, {}, {:.12e}", i + 1, k + 1, v)?;
            }
        }
    }
    if opts.native_loads {
        if !prep.parts.pressure.is_empty() && !dead_pressure {
            writeln!(w, "*DLOAD")?;
            for &(fi, p) in &prep.parts.pressure {
                let f = &prep.faces[fi];
                writeln!(w, "{}, P{}, {:.12e}", f.tet + 1, f.ccx_face, p)?;
            }
        }
        let a = prep.parts.body_accel;
        if norm(a) > 0.0 || !prep.parts.spins.is_empty() {
            writeln!(w, "*DLOAD")?;
            if norm(a) > 0.0 {
                let m = norm(a);
                writeln!(w, "EALL, GRAV, {:.12e}, {:.15}, {:.15}, {:.15}", m, a[0] / m, a[1] / m, a[2] / m)?;
            }
            for (om, c, ax) in &prep.parts.spins {
                writeln!(w, "EALL, CENTRIF, {:.12e}, {:.12e}, {:.12e}, {:.12e}, {:.15}, {:.15}, {:.15}", om * om, c[0], c[1], c[2], ax[0], ax[1], ax[2])?;
            }
        }
    }
    writeln!(w, "*NODE FILE\n{}", if opts.write_reactions { "U, RF" } else { "U" })?;
    writeln!(w, "*EL FILE\nS")?;
    writeln!(w, "*END STEP")?;
    Ok(())
}

pub fn write_inp_file(path: &Path, mesh: &TetMesh, prep: &Prepared, opts: &InpOptions) -> Result<()> {
    let f = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut w = std::io::BufWriter::new(f);
    write_inp(&mut w, mesh, prep, opts)?;
    w.flush()?;
    Ok(())
}

/// One result block of a `.frd` file (e.g. DISP or STRESS of one step).
#[derive(Clone, Debug, Default)]
pub struct FrdBlock {
    pub name: String,
    pub step: u32,
    pub components: Vec<String>,
    /// node number (1-based, as in the file) -> values
    pub values: HashMap<u32, Vec<f64>>,
}

#[derive(Clone, Debug, Default)]
pub struct Frd {
    pub nodes: HashMap<u32, Vec3>,
    pub blocks: Vec<FrdBlock>,
}

impl Frd {
    /// Last block called `name` (DISP, STRESS, FORC, ...).
    pub fn last(&self, name: &str) -> Option<&FrdBlock> {
        self.blocks.iter().rev().find(|b| b.name == name)
    }

    /// Displacements per node index (0-based), from the last DISP block.
    pub fn displacements(&self, n_nodes: usize) -> Result<Vec<Vec3>> {
        let b = self.last("DISP").context("no displacements in the results file")?;
        let mut out = vec![[f64::NAN; 3]; n_nodes];
        for (&id, v) in &b.values {
            if id as usize >= 1 && (id as usize) <= n_nodes && v.len() >= 3 {
                out[id as usize - 1] = [v[0], v[1], v[2]];
            }
        }
        Ok(out)
    }

    /// Nodal stresses (xx yy zz xy yz zx) per node index, from the last STRESS block.
    pub fn stresses(&self, n_nodes: usize) -> Result<Vec<[f64; 6]>> {
        let b = self.last("STRESS").context("no stresses in the results file")?;
        let mut out = vec![[f64::NAN; 6]; n_nodes];
        for (&id, v) in &b.values {
            if id as usize >= 1 && (id as usize) <= n_nodes && v.len() >= 6 {
                out[id as usize - 1] = [v[0], v[1], v[2], v[3], v[4], v[5]];
            }
        }
        Ok(out)
    }
}

fn parse_fixed(s: &str, width: usize) -> Result<Vec<f64>> {
    let s = s.trim_end();
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let j = (i + width).min(b.len());
        let t = std::str::from_utf8(&b[i..j])?.trim();
        if !t.is_empty() {
            out.push(t.parse::<f64>().with_context(|| format!("bad number {t:?}"))?);
        }
        i = j;
    }
    Ok(out)
}

/// Reads an ASCII CalculiX `.frd` results file (short or long format).
pub fn read_frd(text: &str) -> Result<Frd> {
    let mut frd = Frd::default();
    let mut lines = text.lines().peekable();
    let mut step = 0u32;
    while let Some(line) = lines.next() {
        let key = line.get(0..6).unwrap_or("").trim().to_string();
        if key == "2C" || key == "3C" || line.starts_with("  100C") {
            let fmt: u32 = line.split_whitespace().last().and_then(|t| t.parse().ok()).unwrap_or(1);
            let idw = if fmt == 0 { 5 } else { 10 };
            if fmt >= 2 {
                bail!("binary .frd files are not supported; set the output to ASCII");
            }
            if key == "2C" {
                while let Some(l) = lines.next() {
                    if l.starts_with(" -3") {
                        break;
                    }
                    if l.starts_with(" -1") {
                        let id: u32 = l.get(3..3 + idw).context("short node line")?.trim().parse()?;
                        let v = parse_fixed(l.get(3 + idw..).unwrap_or(""), 12)?;
                        if v.len() >= 3 {
                            frd.nodes.insert(id, [v[0], v[1], v[2]]);
                        }
                    }
                }
            } else if key == "3C" {
                for l in lines.by_ref() {
                    if l.starts_with(" -3") {
                        break;
                    }
                }
            } else {
                // result block: -4 name, -5 components, -1/-2 values, -3 end
                step += 1;
                let mut blk = FrdBlock { step, ..Default::default() };
                let mut ncomp = 0usize;
                let mut last: Option<u32> = None;
                while let Some(l) = lines.next() {
                    if l.starts_with(" -3") {
                        break;
                    }
                    if l.starts_with(" -4") {
                        let t: Vec<&str> = l.split_whitespace().collect();
                        blk.name = t.get(1).unwrap_or(&"").to_string();
                    } else if l.starts_with(" -5") {
                        let t: Vec<&str> = l.split_whitespace().collect();
                        let name = t.get(1).unwrap_or(&"").to_string();
                        // "ALL" is a vector-magnitude placeholder with no data column
                        if name != "ALL" {
                            blk.components.push(name);
                            ncomp += 1;
                        }
                    } else if l.starts_with(" -1") {
                        let id: u32 = l.get(3..3 + idw).context("short result line")?.trim().parse()?;
                        let v = parse_fixed(l.get(3 + idw..).unwrap_or(""), 12)?;
                        blk.values.insert(id, v);
                        last = Some(id);
                    } else if l.starts_with(" -2") {
                        if let Some(id) = last {
                            let v = parse_fixed(l.get(3 + idw..).unwrap_or(""), 12)?;
                            blk.values.get_mut(&id).unwrap().extend(v);
                        }
                    }
                }
                for v in blk.values.values_mut() {
                    v.truncate(ncomp.max(1));
                }
                frd.blocks.push(blk);
            }
        }
        let _ = lines.peek();
    }
    Ok(frd)
}

pub fn read_frd_file(path: &Path) -> Result<Frd> {
    read_frd(&std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?)
}

/// How far a CalculiX result is from balancing its loads: the largest
/// |internal force − applied load| over the free nodes, relative to the
/// largest applied nodal load. Uses CalculiX's own nodal forces (the `RF`
/// output, which `.frd` gives at every node), so the rounding of the
/// written displacements does not enter. `nonlinear`: the centrifugal load
/// acts on the deformed shape (`u` = CalculiX's displacement). A correct
/// solve gives ~1e-3 (CalculiX integrates CENTRIF with fewer points than
/// SimLab); a broken one ≫ 1.
pub fn equilibrium_error(frd: &Frd, mesh: &TetMesh, prep: &Prepared, u: &[f64], nonlinear: bool) -> Result<f64> {
    let b = frd.last("FORC").context("no nodal forces (RF) in the results file")?;
    let n = mesh.nodes.len();
    let mut load = prep.f.clone();
    if nonlinear {
        for (l, d) in load.iter_mut().zip(crate::strength::spin_follow_load(mesh, prep, u)) {
            *l += d;
        }
    }
    let fmax = load.chunks(3).map(|c| norm([c[0], c[1], c[2]])).fold(0.0f64, f64::max).max(1e-300);
    let mut worst: f64 = 0.0;
    for i in 0..n {
        if (0..3).any(|k| prep.bc.fixed[3 * i + k]) {
            continue;
        }
        let Some(v) = b.values.get(&(i as u32 + 1)) else { continue };
        if v.len() < 3 {
            continue;
        }
        worst = worst.max(norm([v[0] - load[3 * i], v[1] - load[3 * i + 1], v[2] - load[3 * i + 2]]));
    }
    Ok(worst / fmax)
}

/// Node-by-node agreement between SimLab and a reference solution.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// max |Δu| / max |u|
    pub displacement: f64,
    /// max |Δσ_vM| / max σ_vM (all nodes)
    pub von_mises: f64,
    /// RMS of Δσ_vM / max σ_vM
    pub von_mises_rms: f64,
    pub peak_von_mises: (f64, f64),
    pub peak_displacement: (f64, f64),
}

/// Compares nodal displacement and von Mises stress. Nodes no element uses
/// (none, for mesher output) and NaN reference values are skipped.
pub fn compare(ours_u: &[Vec3], ours_s: &[[f64; 6]], ref_u: &[Vec3], ref_s: &[[f64; 6]]) -> Comparison {
    use crate::material::von_mises;
    let mut du: f64 = 0.0;
    let mut umax: f64 = 0.0;
    let mut rumax: f64 = 0.0;
    for (a, b) in ours_u.iter().zip(ref_u) {
        if b[0].is_nan() {
            continue;
        }
        du = du.max(norm(simlab_core::sub(*a, *b)));
        umax = umax.max(norm(*a));
        rumax = rumax.max(norm(*b));
    }
    let mut dv: f64 = 0.0;
    let mut sq = 0.0;
    let mut cnt = 0usize;
    let mut vmax: f64 = 0.0;
    let mut rvmax: f64 = 0.0;
    for (a, b) in ours_s.iter().zip(ref_s) {
        if b[0].is_nan() {
            continue;
        }
        let (va, vb) = (von_mises(a), von_mises(b));
        dv = dv.max((va - vb).abs());
        sq += (va - vb) * (va - vb);
        cnt += 1;
        vmax = vmax.max(va);
        rvmax = rvmax.max(vb);
    }
    Comparison {
        displacement: du / rumax.max(1e-300),
        von_mises: dv / rvmax.max(1e-300),
        von_mises_rms: (sq / cnt.max(1) as f64).sqrt() / rvmax.max(1e-300),
        peak_von_mises: (vmax, rvmax),
        peak_displacement: (umax, rumax),
    }
}

/// A mesh read from a CalculiX/Abaqus deck: C3D10 elements only.
pub struct InpMesh {
    pub mesh: TetMesh,
    /// node sets, as 0-based node indices
    pub nsets: HashMap<String, Vec<u32>>,
    /// file node number for each mesh node
    pub node_ids: Vec<u32>,
}

/// Reads `*NODE`, `*ELEMENT, TYPE=C3D10` and `*NSET` cards (no `*INCLUDE`,
/// no `GENERATE`). Coordinates are multiplied by `scale` (0.001 for a deck
/// in millimetres). Boundary faces are found from the elements (patch 0).
pub fn read_inp_mesh(text: &str, scale: f64) -> Result<InpMesh> {
    #[derive(PartialEq)]
    enum Mode {
        None,
        Node,
        Elem,
        Nset(String),
    }
    let mut mode = Mode::None;
    let mut id_of: HashMap<u32, u32> = HashMap::new();
    let mut nodes = Vec::new();
    let mut node_ids = Vec::new();
    let mut elems: Vec<Vec<u32>> = Vec::new();
    let mut pending: Vec<u32> = Vec::new();
    let mut raw_sets: HashMap<String, Vec<u32>> = HashMap::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("**") {
            continue;
        }
        if l.starts_with('*') {
            let up = l.to_ascii_uppercase();
            let key = up.split(',').next().unwrap_or("").trim().to_string();
            mode = match key.as_str() {
                "*NODE" => Mode::Node,
                "*ELEMENT" => {
                    if !up.replace(' ', "").contains("TYPE=C3D10") {
                        bail!("only C3D10 elements are supported ({l})");
                    }
                    Mode::Elem
                }
                "*NSET" => {
                    let name = up.split(',').find_map(|t| t.trim().strip_prefix("NSET=").map(|s| s.trim().to_string())).unwrap_or_default();
                    if up.contains("GENERATE") {
                        bail!("NSET GENERATE is not supported");
                    }
                    Mode::Nset(name)
                }
                _ => Mode::None,
            };
            continue;
        }
        let nums: Vec<&str> = l.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
        match &mode {
            Mode::Node => {
                if nums.len() < 4 {
                    bail!("short node line: {l}");
                }
                let id: u32 = nums[0].parse()?;
                let p: Vec<f64> = nums[1..4].iter().map(|t| t.parse::<f64>()).collect::<std::result::Result<_, _>>()?;
                id_of.insert(id, nodes.len() as u32);
                node_ids.push(id);
                nodes.push([p[0] * scale, p[1] * scale, p[2] * scale]);
            }
            Mode::Elem => {
                for t in nums {
                    pending.push(t.parse()?);
                }
                if pending.len() >= 11 {
                    elems.push(pending[1..11].to_vec());
                    pending.clear();
                }
            }
            Mode::Nset(name) => {
                let v = raw_sets.entry(name.clone()).or_default();
                for t in nums {
                    v.push(t.parse()?);
                }
            }
            Mode::None => {}
        }
    }
    let map = |id: u32| id_of.get(&id).copied().with_context(|| format!("element uses unknown node {id}"));
    let mut tets = Vec::with_capacity(elems.len());
    for e in &elems {
        let mut t = [0u32; 10];
        for k in 0..10 {
            t[k] = map(e[k])?;
        }
        tets.push(t);
    }
    let mut nsets = HashMap::new();
    for (k, v) in raw_sets {
        nsets.insert(k, v.into_iter().map(map).collect::<Result<Vec<u32>>>()?);
    }
    let faces = crate::topo::exterior_faces(&tets);
    let face_patch = vec![0; faces.len()];
    let mesh = TetMesh { nodes, tets, faces, face_patch };
    mesh.check()?;
    Ok(InpMesh { mesh, nsets, node_ids })
}
