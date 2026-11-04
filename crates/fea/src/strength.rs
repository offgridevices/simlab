//! The strength study: turns a `StrengthConfig` (supports + loads picked on
//! surface patches) into nodal loads and supports, solves, and reports
//! stresses, Tsai-Hill failure index, safety factor and where it breaks.

use crate::assembly::{Dirichlet, internal_forces};
use crate::elem::{tet_rule_14, tet10_gradients, tet10_point, tet10_shape};
use crate::loads::{MassProps, add_body_force, add_traction, faces_area, faces_integral, mass_props};
use crate::material::{Elastic, principal, von_mises};
use crate::recovery::{Stresses, recover, weighted_percentile};
use crate::solver::{SolveOptions, SolverReport, solve};
use crate::topo::{BFace, boundary_faces, tet_coords};
use anyhow::{Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use simlab_core::{Load, Material, Progress, StrengthConfig, Support, TetMesh, Vec3, cross, dot, norm, normalize, scale, sub};
use std::collections::HashMap;
use std::time::Instant;

/// Standard gravity (m/s²), used for loads given in g.
pub const G0: f64 = 9.80665;
/// Safety factor at or above which the verdict is "safe" (printed parts
/// scatter a lot in strength, so a factor of 2 is the usual design rule).
pub const SAFE_FACTOR: f64 = 2.0;

/// Everything a strength run needs.
pub struct StrengthInput<'a> {
    pub mesh: &'a TetMesh,
    pub material: &'a Material,
    pub config: &'a StrengthConfig,
    /// metres per model unit: positions in the config (`center_mm`) are in
    /// model units; 0.001 for millimetre models
    pub model_unit_m: f64,
    /// extra traction on each boundary face (Pa, global axes, constant over
    /// the face), indexed like `mesh.faces`; the engine fills it from the
    /// propeller air-load model (`Load::PropAero`)
    pub face_tractions: Option<&'a [Vec3]>,
}

/// Inertia relief for a free-flying part.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InertiaRelief {
    /// rigid-body acceleration of the centre of mass (m/s²) that balances the loads
    pub accel_m_s2: Vec3,
    /// rigid-body angular acceleration (rad/s²)
    pub angular_accel_rad_s2: Vec3,
    /// centre of mass used (m), including motor masses
    pub center_m: Vec3,
    /// the 3-2-1 support: node, and which components (x, y, z) are held
    pub support: Vec<(u32, [bool; 3])>,
    /// largest support reaction (N): ~0 when the loads are in balance
    pub max_reaction_n: f64,
    /// that reaction relative to the total applied load
    pub relative_reaction: f64,
}

/// Prepared loads and supports (shared by the native solve and the CalculiX writer).
#[derive(Clone)]
pub struct Prepared {
    pub elastic: Elastic,
    pub faces: Vec<BFace>,
    pub bc: Dirichlet,
    /// total nodal load (N), including inertia relief
    pub f: Vec<f64>,
    /// pieces of `f`, so a CalculiX deck can use its own load cards
    pub parts: LoadParts,
    pub mass: MassProps,
    /// motor point masses: (kg, position m, face indices)
    pub motors: Vec<(f64, Vec3, Vec<usize>)>,
    pub inertia: Option<InertiaRelief>,
    /// nodes on fixed-support faces
    pub support_node: Vec<bool>,
    /// Some(rpm) when the loads are only a spin (stress ∝ rpm²)
    pub spin_only_rpm: Option<f64>,
    /// sum of applied forces (N), and the largest absolute applied load measure
    pub applied_force_n: Vec3,
    pub load_scale_n: f64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LoadParts {
    /// nodal forces from Force, Motor (thrust, torque, weight) and face tractions
    pub surface: Vec<f64>,
    /// (face index, pressure Pa) for each pressure-loaded face
    pub pressure: Vec<(usize, f64)>,
    /// nodal forces from the pressures
    pub pressure_nodal: Vec<f64>,
    /// uniform acceleration body load (m/s², force per unit mass)
    pub body_accel: Vec3,
    /// spins: (ω rad/s, centre m, unit axis)
    pub spins: Vec<(f64, Vec3, Vec3)>,
    /// nodal forces from gravity / g-loads and spin
    pub body_nodal: Vec<f64>,
    /// nodal inertia-relief forces (free-flying only)
    pub inertia_nodal: Vec<f64>,
}

fn faces_of(faces: &[BFace], patches: &[u32], what: &str) -> Result<Vec<usize>> {
    if patches.is_empty() {
        bail!("{what} has no surfaces picked");
    }
    let set: std::collections::HashSet<u32> = patches.iter().copied().collect();
    let v: Vec<usize> = (0..faces.len()).filter(|&i| set.contains(&faces[i].patch)).collect();
    if v.is_empty() {
        bail!("{what}: the picked surfaces (patches {patches:?}) are not on this mesh");
    }
    for p in &set {
        if !faces.iter().any(|f| f.patch == *p) {
            bail!("{what}: surface patch {p} is not on this mesh");
        }
    }
    Ok(v)
}

/// Rigid-body displacement matrix Φ(x) (3×6): u = t + θ × (x − c).
#[inline]
fn phi(x: Vec3, c: Vec3) -> [[f64; 6]; 3] {
    let r = sub(x, c);
    [[1.0, 0.0, 0.0, 0.0, r[2], -r[1]], [0.0, 1.0, 0.0, -r[2], 0.0, r[0]], [0.0, 0.0, 1.0, r[1], -r[0], 0.0]]
}

#[inline]
fn phi_a(x: Vec3, c: Vec3, a: &[f64; 6]) -> Vec3 {
    let p = phi(x, c);
    std::array::from_fn(|i| (0..6).map(|k| p[i][k] * a[k]).sum())
}

fn solve_small(m: &[[f64; 6]; 6], b: &[f64; 6]) -> Option<[f64; 6]> {
    let mut a = *m;
    let mut x = *b;
    for k in 0..6 {
        let p = (k..6).max_by(|&i, &j| a[i][k].abs().partial_cmp(&a[j][k].abs()).unwrap())?;
        if a[p][k].abs() < 1e-300 {
            return None;
        }
        a.swap(k, p);
        x.swap(k, p);
        for i in k + 1..6 {
            let f = a[i][k] / a[k][k];
            for j in k..6 {
                a[i][j] -= f * a[k][j];
            }
            x[i] -= f * x[k];
        }
    }
    for k in (0..6).rev() {
        let mut s = x[k];
        for j in k + 1..6 {
            s -= a[k][j] * x[j];
        }
        x[k] = s / a[k][k];
    }
    Some(x)
}

/// 6×6 rigid-body mass matrix ∫ρΦᵀΦ about `c` from mass properties.
fn rigid_mass(mp: &MassProps, c: Vec3, motors: &[(f64, Vec3, Vec<usize>)]) -> [[f64; 6]; 6] {
    // ∫ρ ΦᵀΦ with r = x − c: blocks [m I, −S(∫ρr)ᵀ...]; easiest from moments
    let m = mp.mass;
    let s1 = sub(mp.first, scale(c, m)); // ∫ρ r
    // ∫ρ r rᵀ
    let mut s2 = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            s2[i][j] = mp.second[i][j] - c[i] * mp.first[j] - mp.first[i] * c[j] + m * c[i] * c[j];
        }
    }
    let mut out = [[0.0; 6]; 6];
    let mut add_point = |w: f64, s1: Vec3, s2: [[f64; 3]; 3]| {
        // Φ = [I | −[r]×]; ΦᵀΦ = [[I, −[r]×], [[r]×, [r]×ᵀ[r]×]]... computed generically:
        // translational block
        for i in 0..3 {
            out[i][i] += w;
        }
        // coupling: Φ_t ᵀ Φ_θ = rows of Φ restricted: Φ_θ(r) = −[r]× → ∫ −[r]× = −[s1]×
        let sk = [[0.0, -s1[2], s1[1]], [s1[2], 0.0, -s1[0]], [-s1[1], s1[0], 0.0]];
        for i in 0..3 {
            for j in 0..3 {
                out[i][3 + j] += -sk[i][j];
                out[3 + j][i] += -sk[i][j];
            }
        }
        // rotational: ∫ [r]×ᵀ[r]× = ∫ (|r|² I − r rᵀ)
        let tr = s2[0][0] + s2[1][1] + s2[2][2];
        for i in 0..3 {
            for j in 0..3 {
                out[3 + i][3 + j] += if i == j { tr } else { 0.0 } - s2[i][j];
            }
        }
    };
    add_point(m, s1, s2);
    for (mk, xk, _) in motors {
        let r = sub(*xk, c);
        let mut rr = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                rr[i][j] = mk * r[i] * r[j];
            }
        }
        add_point(*mk, scale(r, *mk), rr);
    }
    out
}

/// Picks three far-apart corner nodes for the minimal (3-2-1) support of a
/// free body: A holds x, y, z; B holds the two components that stop
/// rotation about the axes across AB; C holds the one that stops rotation
/// about AB. The combination with the best-conditioned rotation constraint
/// is chosen.
pub fn three_two_one(mesh: &TetMesh) -> Result<Vec<(u32, [bool; 3])>> {
    let corners: Vec<u32> = {
        let mut is = vec![false; mesh.nodes.len()];
        for t in &mesh.tets {
            for &v in &t[..4] {
                is[v as usize] = true;
            }
        }
        (0..mesh.nodes.len() as u32).filter(|&i| is[i as usize]).collect()
    };
    if corners.len() < 3 {
        bail!("the mesh is too small");
    }
    let mut c = [0.0; 3];
    for &i in &corners {
        c = simlab_core::add(c, mesh.nodes[i as usize]);
    }
    c = scale(c, 1.0 / corners.len() as f64);
    let far = |from: Vec3| *corners.iter().max_by(|&&a, &&b| norm(sub(mesh.nodes[a as usize], from)).partial_cmp(&norm(sub(mesh.nodes[b as usize], from))).unwrap()).unwrap();
    let a = far(c);
    let pa = mesh.nodes[a as usize];
    let b = far(pa);
    let pb = mesh.nodes[b as usize];
    let ab = normalize(sub(pb, pa));
    let cc = *corners
        .iter()
        .max_by(|&&x, &&y| {
            let dx = norm(cross(ab, sub(mesh.nodes[x as usize], pa)));
            let dy = norm(cross(ab, sub(mesh.nodes[y as usize], pa)));
            dx.partial_cmp(&dy).unwrap()
        })
        .unwrap();
    let pc = mesh.nodes[cc as usize];
    if norm(cross(ab, sub(pc, pa))) < 1e-9 * norm(sub(pb, pa)) {
        bail!("the part is a line; it cannot be held by a 3-2-1 support");
    }
    // rotation θ moves a point p by θ × (p − A): row for component k is (e_k × ... )
    // displacement_k = (θ × d)_k = θ · (d × e_k)
    let row = |d: Vec3, k: usize| -> Vec3 {
        let mut e = [0.0; 3];
        e[k] = 1.0;
        cross(d, e)
    };
    let (dab, dac) = (sub(pb, pa), sub(pc, pa));
    let mut best = (0.0, 0, 0, 0);
    for k1 in 0..3 {
        for k2 in k1 + 1..3 {
            for k3 in 0..3 {
                let (r1, r2, r3) = (row(dab, k1), row(dab, k2), row(dac, k3));
                let det = dot(r1, cross(r2, r3)).abs() / (norm(dab) * norm(dab) * norm(dac));
                if det > best.0 {
                    best = (det, k1, k2, k3);
                }
            }
        }
    }
    let (_, k1, k2, k3) = best;
    let mut hb = [false; 3];
    hb[k1] = true;
    hb[k2] = true;
    let mut hc = [false; 3];
    hc[k3] = true;
    Ok(vec![(a, [true; 3]), (b, hb), (cc, hc)])
}

/// Builds nodal loads and supports from the config.
pub fn prepare(input: &StrengthInput) -> Result<Prepared> {
    prepare_inner(input, None)
}

/// Builds the loads from the config but uses `bc` (any prescribed
/// displacement components) instead of the config's support. For symmetry
/// planes and benchmark problems.
pub fn prepare_with_bc(input: &StrengthInput, bc: Dirichlet) -> Result<Prepared> {
    prepare_inner(input, Some(bc))
}

fn prepare_inner(input: &StrengthInput, custom_bc: Option<Dirichlet>) -> Result<Prepared> {
    let mesh = input.mesh;
    let cfg = input.config;
    let u = input.model_unit_m;
    if !(u > 0.0) {
        bail!("the model unit must be above zero");
    }
    if mesh.tets.is_empty() {
        bail!("the mesh has no elements");
    }
    let el = Elastic::from_material(input.material, cfg.print_up)?;
    let faces = boundary_faces(mesh)?;
    let n = mesh.nodes.len();
    let nd = 3 * n;
    let mut parts = LoadParts { surface: vec![0.0; nd], pressure_nodal: vec![0.0; nd], body_nodal: vec![0.0; nd], inertia_nodal: vec![0.0; nd], ..Default::default() };
    let mut warnings = Vec::new();
    let mut motors: Vec<(f64, Vec3, Vec<usize>)> = Vec::new();
    // total g-load first: motors' weight needs it
    let mut g_total = [0.0; 3];
    for l in &cfg.loads {
        if let Load::Acceleration { g, .. } = l {
            g_total = simlab_core::add(g_total, *g);
        }
    }
    let accel = scale(g_total, G0);
    let mut only_spin = true;
    let mut spin_rpm = None;
    let mut n_spin = 0;
    for l in &cfg.loads {
        match l {
            Load::Force { patches, force_n, label } => {
                only_spin = false;
                let fl = faces_of(&faces, patches, &format!("force {label}"))?;
                let (area, _) = faces_area(mesh, &faces, &fl);
                let t = scale(*force_n, 1.0 / area);
                add_traction(mesh, &faces, &fl, |_, _| t, &mut parts.surface);
            }
            Load::Pressure { patches, pressure_pa, label } => {
                only_spin = false;
                let fl = faces_of(&faces, patches, &format!("pressure {label}"))?;
                let p = *pressure_pa;
                add_traction(mesh, &faces, &fl, |_, nrm| scale(nrm, -p), &mut parts.pressure_nodal);
                parts.pressure.extend(fl.iter().map(|&i| (i, p)));
            }
            Load::Motor { patches, thrust_n, torque_nm, axis, motor_mass_g, label } => {
                only_spin = false;
                let fl = faces_of(&faces, patches, &format!("motor {label}"))?;
                if !(norm(*axis) > 0.0) {
                    bail!("motor {label}: the thrust axis is not set");
                }
                let ax = normalize(*axis);
                let (area, c) = faces_area(mesh, &faces, &fl);
                let tt = scale(ax, thrust_n / area);
                let perp = |x: Vec3| {
                    let r = sub(x, c);
                    sub(r, scale(ax, dot(r, ax)))
                };
                let j = faces_integral(mesh, &faces, &fl, |x| dot(perp(x), perp(x)));
                if torque_nm.abs() > 0.0 && !(j > 0.0) {
                    bail!("motor {label}: the mount surfaces cannot take a torque (they are a point)");
                }
                let tq = *torque_nm;
                let mass = motor_mass_g * 1e-3;
                let tw = scale(accel, mass / area);
                add_traction(
                    mesh,
                    &faces,
                    &fl,
                    |x, _| {
                        let mut t = simlab_core::add(tt, tw);
                        if tq != 0.0 {
                            t = simlab_core::add(t, scale(cross(ax, perp(x)), tq / j));
                        }
                        t
                    },
                    &mut parts.surface,
                );
                if mass > 0.0 {
                    motors.push((mass, c, fl));
                }
            }
            Load::Acceleration { .. } => {
                only_spin = false;
            }
            Load::Spin { rpm, axis, center_mm, label } => {
                if !(norm(*axis) > 0.0) {
                    bail!("spin {label}: the axis is not set");
                }
                let w = rpm * std::f64::consts::TAU / 60.0;
                parts.spins.push((w, scale(*center_mm, u), normalize(*axis)));
                spin_rpm = Some(*rpm);
                n_spin += 1;
            }
            Load::PropAero { .. } => {
                only_spin = false;
                if input.face_tractions.is_none() {
                    warnings.push("The propeller air load was not included (no air-load data was passed to the stress solver).".into());
                }
            }
        }
    }
    if let Some(ft) = input.face_tractions {
        if ft.len() != mesh.faces.len() {
            bail!("the air-load data does not match the mesh ({} values for {} faces)", ft.len(), mesh.faces.len());
        }
        let which: Vec<usize> = (0..faces.len()).filter(|&i| ft[i] != [0.0; 3]).collect();
        if !which.is_empty() {
            only_spin = false;
        }
        // face index i in `faces` == mesh.faces index i
        let ftv = ft.to_vec();
        let mut tmp = vec![0.0; nd];
        for &i in &which {
            let t = ftv[i];
            add_traction(mesh, &faces, &[i], |_, _| t, &mut tmp);
        }
        for k in 0..nd {
            parts.surface[k] += tmp[k];
        }
    }
    if cfg.loads.is_empty() && input.face_tractions.is_none() {
        bail!("the study has no loads");
    }
    parts.body_accel = accel;
    let rho = el.density;
    let spins = parts.spins.clone();
    if accel != [0.0; 3] || !spins.is_empty() {
        add_body_force(
            mesh,
            rho,
            |x| {
                let mut b = accel;
                for (w, c, ax) in &spins {
                    let r = sub(x, *c);
                    let rp = sub(r, scale(*ax, dot(r, *ax)));
                    b = simlab_core::add(b, scale(rp, w * w));
                }
                b
            },
            &mut parts.body_nodal,
        );
    }
    let mass = mass_props(mesh, rho);
    let mut f: Vec<f64> = (0..nd).map(|k| parts.surface[k] + parts.pressure_nodal[k] + parts.body_nodal[k]).collect();
    let mut applied = [0.0; 3];
    for i in 0..n {
        for k in 0..3 {
            applied[k] += f[3 * i + k];
        }
    }
    let load_scale = f.chunks(3).map(|c| norm([c[0], c[1], c[2]])).sum::<f64>().max(1e-300);

    let mut bc = Dirichlet::new(nd);
    let mut support_node = vec![false; n];
    let mut inertia = None;
    let has_custom = custom_bc.is_some();
    if let Some(c) = custom_bc {
        if c.fixed.len() != nd {
            bail!("the support list has the wrong length");
        }
        bc = c;
    }
    match &cfg.support {
        _ if has_custom => {
            for i in 0..n {
                support_node[i] = (0..3).all(|k| bc.fixed[3 * i + k]);
            }
        }
        Support::Fixed { patches } => {
            let fl = faces_of(&faces, patches, "the fixed support")?;
            for &i in &fl {
                for &v in &faces[i].nodes {
                    support_node[v as usize] = true;
                    for k in 0..3 {
                        bc.set(3 * v as usize + k, 0.0);
                    }
                }
            }
        }
        Support::FreeFlying => {
            let total_mass = mass.mass + motors.iter().map(|m| m.0).sum::<f64>();
            if !(total_mass > 0.0) {
                bail!("a free-flying part needs a material with a density above zero");
            }
            let mut cm = mass.first;
            for (mk, xk, _) in &motors {
                cm = simlab_core::add(cm, scale(*xk, *mk));
            }
            let cm = scale(cm, 1.0 / total_mass);
            let mrr = rigid_mass(&mass, cm, &motors);
            // generalized force Σ Φ(xᵢ)ᵀ fᵢ
            let mut fr = [0.0; 6];
            for i in 0..n {
                let p = phi(mesh.nodes[i], cm);
                for k in 0..6 {
                    for c in 0..3 {
                        fr[k] += p[c][k] * f[3 * i + c];
                    }
                }
            }
            let a = solve_small(&mrr, &fr).ok_or_else(|| anyhow::anyhow!("cannot balance the loads on this free part"))?;
            // inertial body force −ρΦ(x)a and motor inertia
            add_body_force(mesh, rho, |x| scale(phi_a(x, cm, &a), -1.0), &mut parts.inertia_nodal);
            for (mk, xk, fl) in &motors {
                let (area, _) = faces_area(mesh, &faces, fl);
                let t = scale(phi_a(*xk, cm, &a), -mk / area);
                add_traction(mesh, &faces, fl, |_, _| t, &mut parts.inertia_nodal);
            }
            for k in 0..nd {
                f[k] += parts.inertia_nodal[k];
            }
            let sup = three_two_one(mesh)?;
            for (v, hold) in &sup {
                for k in 0..3 {
                    if hold[k] {
                        bc.set(3 * *v as usize + k, 0.0);
                    }
                }
            }
            inertia = Some(InertiaRelief {
                accel_m_s2: [a[0], a[1], a[2]],
                angular_accel_rad_s2: [a[3], a[4], a[5]],
                center_m: cm,
                support: sup,
                max_reaction_n: 0.0,
                relative_reaction: 0.0,
            });
        }
    }
    let spin_only_rpm = if only_spin && n_spin == 1 { spin_rpm } else { None };
    Ok(Prepared {
        elastic: el,
        faces,
        bc,
        f,
        parts,
        mass,
        motors,
        inertia,
        support_node,
        spin_only_rpm,
        applied_force_n: applied,
        load_scale_n: load_scale,
        warnings,
    })
}

/// Peak values, with the node or element where they occur.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peaks {
    /// nodal (averaged) failure index, support-face nodes excluded: drives the headline
    pub fi_nodal: f64,
    pub fi_nodal_node: u32,
    /// nodal failure index including the fixed-support faces
    pub fi_nodal_all: f64,
    pub fi_nodal_all_node: u32,
    /// robust peak: max over elements of the failure index of the element's
    /// volume-averaged stress
    pub fi_element: f64,
    pub fi_element_index: u32,
    /// volume-weighted 99.9th percentile of the Gauss-point failure index
    pub fi_p999: f64,
    /// von Mises / max principal at nodes (support faces excluded), Pa
    pub von_mises: f64,
    pub max_principal: f64,
    pub von_mises_element: f64,
}

/// Where the part fails first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CriticalPoint {
    pub node: u32,
    /// position in metres
    pub position_m: Vec3,
    /// position in model units (what the UI shows)
    pub position_model: Vec3,
    /// surface patch the point is on, None if inside the part
    pub patch: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Timings {
    pub prepare_s: f64,
    pub solve_s: f64,
    pub recovery_s: f64,
    pub total_s: f64,
}

/// Full result of a strength run (SI units).
pub struct StrengthResult {
    /// nodal displacement (m); for free-flying parts the rigid-body motion
    /// is removed (mass-weighted), so this is the deformation
    pub displacement: Vec<Vec3>,
    /// nodal stress, global Voigt xx yy zz xy yz zx (Pa)
    pub stress: Vec<[f64; 6]>,
    pub von_mises: Vec<f64>,
    pub max_principal: Vec<f64>,
    /// Tsai-Hill failure index at nodes (1 = breaks)
    pub failure_index: Vec<f64>,
    /// failure index of each element's mean stress
    pub element_failure_index: Vec<f64>,
    pub peaks: Peaks,
    /// 1/√(headline failure index)
    pub safety_factor: f64,
    /// all loads multiplied by this reach first failure (= safety factor,
    /// since stress is linear in load and the failure index quadratic)
    pub load_factor: f64,
    /// spin-only studies: the rpm at which it first fails (stress ∝ rpm²)
    pub break_rpm: Option<f64>,
    pub critical: CriticalPoint,
    pub max_displacement_m: f64,
    pub max_displacement_node: u32,
    pub mass_kg: f64,
    /// sum of support reactions (N) and their moment about the origin (N·m)
    pub reaction_force_n: Vec3,
    pub reaction_moment_nm: Vec3,
    pub applied_force_n: Vec3,
    pub inertia_relief: Option<InertiaRelief>,
    pub solver: SolverReport,
    pub timings: Timings,
    pub warnings: Vec<String>,
    pub model_unit_m: f64,
    pub elements: usize,
    pub nodes: usize,
}

/// Runs a strength study.
pub fn solve_strength(input: &StrengthInput, opts: &SolveOptions, progress: &dyn Progress) -> Result<StrengthResult> {
    let t0 = Instant::now();
    progress.report(0.0, "Setting up loads and supports");
    let prep = prepare(input)?;
    let t_prep = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let (u, rep, pre) = solve_prepared(input, &prep, opts, progress, (0.05, 0.9))?;
    let t_solve = t1.elapsed().as_secs_f64();
    let t2 = Instant::now();
    progress.report(0.92, "Working out stresses");
    let mut res = postprocess_with(input, prep, u, rep, pre)?;
    res.timings = Timings { prepare_s: t_prep, solve_s: t_solve, recovery_s: t2.elapsed().as_secs_f64(), total_s: t0.elapsed().as_secs_f64() };
    progress.report(1.0, "Done");
    Ok(res)
}

/// Whether stress stiffening applies to this problem (see [`crate::solver::Stiffening`]).
pub fn stiffening_applies(prep: &Prepared, opts: &SolveOptions) -> bool {
    match opts.stiffening {
        crate::solver::Stiffening::Off => false,
        crate::solver::Stiffening::On => prep.inertia.is_none(),
        crate::solver::Stiffening::Auto => prep.inertia.is_none() && !prep.parts.spins.is_empty(),
    }
}

/// Solves a prepared problem: one linear solve, then (when stress
/// stiffening applies) fixed-point passes with the geometric stiffness of
/// the current stress and spin softening until the displacement changes by
/// less than 0.2 %. Returns the displacement, the solver report and the
/// displacement whose stress pre-loaded the last pass (for reactions).
pub fn solve_prepared(
    input: &StrengthInput,
    prep: &Prepared,
    opts: &SolveOptions,
    progress: &dyn Progress,
    frac: (f32, f32),
) -> Result<(Vec<f64>, SolverReport, Option<Vec<f64>>)> {
    let stiffen = stiffening_applies(prep, opts);
    let span = |a: f32| frac.0 + (frac.1 - frac.0) * a;
    let first_end = if stiffen { span(0.3) } else { frac.1 };
    let (mut u, mut rep) = solve(input.mesh, &prep.elastic, &prep.bc, &prep.f, opts, progress, (frac.0, first_end))?;
    if !stiffen {
        return Ok((u, rep, None));
    }
    let rho = prep.elastic.density;
    let spin: Vec<(f64, Vec3)> = prep.parts.spins.iter().map(|(w, _, ax)| (rho * w * w, *ax)).collect();
    // later passes reuse whichever solver won the first one
    let mut opts = opts.clone();
    if opts.solver == crate::solver::SolverChoice::Auto {
        opts.solver = if rep.method == "direct" { crate::solver::SolverChoice::Direct } else { crate::solver::SolverChoice::Iterative };
    }
    let opts = &opts;
    let mut pre = u.clone();
    let mut change = 1.0;
    let mut passes = 0;
    const MAX_PASSES: usize = 6;
    while passes < MAX_PASSES && change > 2e-3 {
        passes += 1;
        let st = crate::elem::Stiffness { el: &prep.elastic, prestress_u: Some(&pre), spin: &spin };
        let a = span(0.3 + 0.7 * (passes - 1) as f32 / MAX_PASSES as f32);
        let b = span(0.3 + 0.7 * passes as f32 / MAX_PASSES as f32);
        progress.report(a, &format!("Stiffening from the part's own stress, pass {passes}"));
        let (u2, rep2) = crate::solver::solve_with(input.mesh, &st, &prep.bc, &prep.f, opts, progress, (a, b)).map_err(|e| {
            if e.downcast_ref::<simlab_core::Cancelled>().is_some() {
                e
            } else {
                e.context("the spinning part became unstable (it may be spinning faster than its own natural frequency)")
            }
        })?;
        let num: f64 = u2.iter().zip(&u).map(|(a, b)| (a - b) * (a - b)).sum::<f64>().sqrt();
        let den: f64 = u2.iter().map(|a| a * a).sum::<f64>().sqrt().max(1e-300);
        change = num / den;
        let total = rep.solve_seconds + rep.factor_seconds + rep.assembly_seconds + rep.setup_seconds;
        rep = rep2;
        rep.solve_seconds += total;
        pre = std::mem::replace(&mut u, u2);
        // next pass uses the newest stress
        pre.copy_from_slice(&u);
    }
    rep.stiffening_passes = passes;
    rep.stiffening_change = change;
    // reactions must use the operator of the last pass, i.e. the stress of the
    // displacement before it; after convergence the two agree to `change`
    Ok((u.clone(), rep, Some(u)))
}

/// Turns a displacement solution into stresses, failure and summary values.
pub fn postprocess(input: &StrengthInput, prep: Prepared, u: Vec<f64>, solver: SolverReport) -> Result<StrengthResult> {
    postprocess_with(input, prep, u, solver, None)
}

/// [`postprocess`] for a stress-stiffened solve: `prestress_u` is the
/// displacement whose stress pre-loaded the operator (for the reactions).
pub fn postprocess_with(input: &StrengthInput, prep: Prepared, u: Vec<f64>, solver: SolverReport, prestress_u: Option<Vec<f64>>) -> Result<StrengthResult> {
    let mesh = input.mesh;
    let el = &prep.elastic;
    let n = mesh.nodes.len();
    let mut warnings = prep.warnings.clone();
    // reactions at prescribed dofs
    let touching: Vec<bool> = (0..n).map(|i| (0..3).any(|k| prep.bc.fixed[3 * i + k])).collect();
    let spin: Vec<(f64, Vec3)> = if prestress_u.is_some() { prep.parts.spins.iter().map(|(w, _, ax)| (el.density * w * w, *ax)).collect() } else { vec![] };
    let st = crate::elem::Stiffness { el, prestress_u: prestress_u.as_deref(), spin: &spin };
    let fint = internal_forces(mesh, &st, &u, &touching);
    let mut rf = [0.0; 3];
    let mut rm = [0.0; 3];
    let mut max_r: f64 = 0.0;
    for i in 0..n {
        let mut r = [0.0; 3];
        for k in 0..3 {
            if prep.bc.fixed[3 * i + k] {
                r[k] = fint[3 * i + k] - prep.f[3 * i + k];
            }
        }
        rf = simlab_core::add(rf, r);
        rm = simlab_core::add(rm, cross(mesh.nodes[i], r));
        max_r = max_r.max(norm(r));
    }
    let mut inertia = prep.inertia.clone();
    let mut disp: Vec<Vec3> = u.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
    if let Some(ir) = inertia.as_mut() {
        ir.max_reaction_n = max_r;
        ir.relative_reaction = max_r / prep.load_scale_n;
        if ir.relative_reaction > 1e-6 {
            warnings.push(format!("The free-flying balance left a support force of {max_r:.3e} N; results may be slightly off."));
        }
        // remove the rigid-body part of the motion (mass-weighted)
        remove_rigid_motion(mesh, el.density, &prep.mass, ir.center_m, &mut disp);
    }
    let st: Stresses = recover(mesh, el, &u);
    let vm: Vec<f64> = st.nodal.par_iter().map(von_mises).collect();
    let mp: Vec<f64> = st.nodal.par_iter().map(|s| principal(s)[0]).collect();
    let fi: Vec<f64> = st.nodal.par_iter().map(|s| el.failure_index(s)).collect();
    let efi: Vec<f64> = st.element_mean.par_iter().map(|s| el.failure_index(s)).collect();
    // which nodes are real material nodes (used by an element)
    let mut used = vec![false; n];
    for t in &mesh.tets {
        for &v in t {
            used[v as usize] = true;
        }
    }
    // the zone next to a fully held surface: the held faces' nodes plus two
    // rings of elements around them. A rigid hold makes stress there grow
    // without limit as the mesh is refined (a singularity), so the headline
    // uses the highest stress outside it; the zone's own peak is reported.
    let zone = support_zone(mesh, &prep.support_node, 2);
    let argmax = |vals: &[f64], skip_support: bool| -> (f64, u32) {
        let mut best = (f64::NEG_INFINITY, 0u32);
        for i in 0..n {
            if !used[i] || (skip_support && zone[i]) {
                continue;
            }
            if vals[i] > best.0 {
                best = (vals[i], i as u32);
            }
        }
        if best.0 == f64::NEG_INFINITY { (0.0, 0) } else { best }
    };
    let any_free = (0..n).any(|i| used[i] && !zone[i]);
    let (fi_h, fi_h_node) = argmax(&fi, any_free);
    let (fi_all, fi_all_node) = argmax(&fi, false);
    let (vm_max, _) = argmax(&vm, any_free);
    let (mp_max, _) = argmax(&mp, any_free);
    // robust measures skip elements touching the fixed-support faces too
    let free_el: Vec<bool> = mesh.tets.iter().map(|t| !any_free || !t.iter().any(|&v| zone[v as usize])).collect();
    let (fe_max, fe_idx) = efi.iter().enumerate().fold((0.0f64, 0u32), |b, (i, &v)| if free_el[i] && v > b.0 { (v, i as u32) } else { b });
    let vm_el = st.element_mean.iter().zip(&free_el).filter(|(_, f)| **f).map(|(s, _)| von_mises(s)).fold(0.0, f64::max);
    let mut pts: Vec<(f64, f64)> = st
        .gauss
        .par_iter()
        .zip(&st.gauss_w)
        .zip(&free_el)
        .filter(|(_, f)| **f)
        .flat_map_iter(|((g, w), _)| (0..4).map(move |k| (el.failure_index(&g[k]), w[k])))
        .collect();
    let p999 = weighted_percentile(&mut pts, 0.999);
    drop(pts);
    let peaks = Peaks {
        fi_nodal: fi_h,
        fi_nodal_node: fi_h_node,
        fi_nodal_all: fi_all,
        fi_nodal_all_node: fi_all_node,
        fi_element: fe_max,
        fi_element_index: fe_idx,
        fi_p999: p999,
        von_mises: vm_max,
        max_principal: mp_max,
        von_mises_element: vm_el,
    };
    let sf = if fi_h > 0.0 { 1.0 / fi_h.sqrt() } else { f64::INFINITY };
    if fi_all > 1.5 * fi_h && fi_all > 0.0 {
        warnings.push(format!(
            "Stress is highest right at the held surfaces ({:.1}× the rest of the part, as stress). A fully rigid hold exaggerates it there, so the safety factor uses the highest stress away from the held surfaces.",
            (fi_all / fi_h).sqrt()
        ));
    }
    if fi_h > 4.0 * fe_max && fe_max > 0.0 {
        warnings.push("The highest stress is in a sharp corner; its value depends on the mesh size. A rounded corner (fillet) in the real part would lower it.".into());
    }
    let (dmax, dnode) = disp.iter().enumerate().fold((0.0f64, 0u32), |b, (i, d)| if used[i] && norm(*d) > b.0 { (norm(*d), i as u32) } else { b });
    // size of the part
    let (lo, hi) = bounds(mesh);
    let size = norm(sub(hi, lo));
    if dmax > 0.05 * size {
        warnings.push(format!("The part bends a lot ({:.0} % of its size); a straight-line (linear) analysis is only rough at this load.", 100.0 * dmax / size));
    }
    if !input.material.estimated_fields.is_empty() {
        warnings.push(format!("Some material values are estimates: {}.", input.material.estimated_fields.join(", ")));
    }
    // patch of the critical node
    let mut node_patch: HashMap<u32, u32> = HashMap::new();
    for f in &prep.faces {
        for &v in &f.nodes {
            node_patch.entry(v).or_insert(f.patch);
        }
    }
    let crit_x = mesh.nodes[fi_h_node as usize];
    let critical = CriticalPoint {
        node: fi_h_node,
        position_m: crit_x,
        position_model: scale(crit_x, 1.0 / input.model_unit_m),
        patch: node_patch.get(&fi_h_node).copied(),
    };
    let break_rpm = prep.spin_only_rpm.map(|rpm| rpm * sf.sqrt());
    Ok(StrengthResult {
        displacement: disp,
        stress: st.nodal,
        von_mises: vm,
        max_principal: mp,
        failure_index: fi,
        element_failure_index: efi,
        peaks,
        safety_factor: sf,
        load_factor: sf,
        break_rpm,
        critical,
        max_displacement_m: dmax,
        max_displacement_node: dnode,
        mass_kg: prep.mass.mass,
        reaction_force_n: rf,
        reaction_moment_nm: rm,
        applied_force_n: prep.applied_force_n,
        inertia_relief: inertia,
        solver,
        timings: Timings { prepare_s: 0.0, solve_s: 0.0, recovery_s: 0.0, total_s: 0.0 },
        warnings,
        model_unit_m: input.model_unit_m,
        elements: mesh.tets.len(),
        nodes: n,
    })
}

/// Removes the mass-weighted rigid-body part (translation + rotation about
/// `center`) from a nodal displacement field. Free-flying results are shown
/// this way; apply it to a reference solution before comparing.
pub fn remove_rigid_motion(mesh: &TetMesh, density: f64, mass: &MassProps, center: Vec3, disp: &mut [Vec3]) {
    let mrr = rigid_mass(mass, center, &[]);
    let rule = tet_rule_14();
    let d: &[Vec3] = disp;
    let proj = (0..mesh.tets.len())
        .into_par_iter()
        .map(|e| {
            let x = tet_coords(mesh, e);
            let t = &mesh.tets[e];
            let mut acc = [0.0; 6];
            for q in &rule {
                let (_, det) = tet10_gradients(&x, &q.l);
                let p = tet10_point(&x, &q.l);
                let nn = tet10_shape(&q.l);
                let mut uu = [0.0; 3];
                for a in 0..10 {
                    for k in 0..3 {
                        uu[k] += nn[a] * d[t[a] as usize][k];
                    }
                }
                let ph = phi(p, center);
                let w = q.w * det * density;
                for k in 0..6 {
                    acc[k] += w * (ph[0][k] * uu[0] + ph[1][k] * uu[1] + ph[2][k] * uu[2]);
                }
            }
            acc
        })
        .reduce(|| [0.0; 6], |a, b| std::array::from_fn(|k| a[k] + b[k]));
    if let Some(q) = solve_small(&mrr, &proj) {
        for (i, dd) in disp.iter_mut().enumerate() {
            *dd = sub(*dd, phi_a(mesh.nodes[i], center, &q));
        }
    }
}

/// Nodes within `rings` element rings of the marked nodes (marked included).
pub fn support_zone(mesh: &TetMesh, marked: &[bool], rings: usize) -> Vec<bool> {
    let mut zone = marked.to_vec();
    if !marked.iter().any(|&m| m) {
        return zone;
    }
    for _ in 0..rings {
        let prev = zone.clone();
        for t in &mesh.tets {
            if t.iter().any(|&v| prev[v as usize]) {
                for &v in t {
                    zone[v as usize] = true;
                }
            }
        }
    }
    zone
}

pub fn bounds(mesh: &TetMesh) -> (Vec3, Vec3) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &mesh.nodes {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}
