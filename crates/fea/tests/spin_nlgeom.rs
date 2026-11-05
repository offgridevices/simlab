//! A spinning, held, printed (transversely isotropic) blade, solved
//! geometrically nonlinear (SimLab's stiffening solve), against CalculiX
//! 2.21 `NLGEOM` on the same deck.
//!
//! The blade runs from 5 to 45 mm out, tilted 4° out of the rotation plane
//! (like a coned propeller blade, so the spin both stretches and bends it),
//! printed with the layers tilted too, at 50,000 rpm: about 1.5 % strain at
//! the root, where true (Cauchy) and small-strain stress differ by ~3 %,
//! and the linearised stiffening SimLab used before differed from CalculiX
//! by 2-3 %, like on the E-v5 propeller.
//!
//! `matches_stored_calculix` needs no Docker: it compares with CalculiX's
//! answer stored in `tests/data/spin_blade_nlgeom.frd`. The ignored
//! `matches_fresh_calculix` runs CalculiX again with the engine's settings
//! (`SIMLAB_WRITE_FIXTURE=1` also rewrites the stored file).
mod common;
use common::*;
use simlab_core::{Load, Material, Silent, StrengthConfig, Support, TetMesh, Vec3, norm, sub};
use simlab_fea::calculix::{Frd, InpOptions, compare, equilibrium_error, read_frd, write_inp_file};
use simlab_fea::mesh_gen::{Midside, box_mesh};
use simlab_fea::{Prepared, SolveOptions, StrengthInput, StrengthResult, postprocess_reference, prepare, solve_strength};

/// how far the blade leans out of the rotation plane (degrees)
const TILT_DEG: f64 = 4.0;
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/spin_blade_nlgeom.frd");

fn blade() -> TetMesh {
    let tilt = TILT_DEG.to_radians();
    box_mesh(
        16,
        2,
        2,
        |p| {
            let (x, y, z) = (5.0 + 40.0 * p[0], 8.0 * (p[1] - 0.5), 3.0 * (p[2] - 0.5));
            [(x * tilt.cos() - z * tilt.sin()) * 1e-3, y * 1e-3, (x * tilt.sin() + z * tilt.cos()) * 1e-3]
        },
        Midside::Mapped,
    )
}

fn petg() -> Material {
    let mut m = Material::isotropic("petg", "PETG-like", 1250.0, 2117.0, 0.38, 50.8);
    m.e_z_mpa = 1899.0;
    m.uts_z_mpa = 22.86;
    m
}

fn config() -> StrengthConfig {
    // patch 1 = the root face (x = 0 of the unit box)
    cfg(Support::Fixed { patches: vec![1] }, vec![Load::Spin { rpm: 50_000.0, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3], label: String::new() }], [0.3, 0.1, 1.0])
}

struct Case {
    mesh: TetMesh,
    mat: Material,
    c: StrengthConfig,
}

impl Case {
    fn new() -> Case {
        Case { mesh: blade(), mat: petg(), c: config() }
    }
    fn input(&self) -> StrengthInput<'_> {
        StrengthInput { mesh: &self.mesh, material: &self.mat, config: &self.c, model_unit_m: 0.001, face_tractions: None }
    }
    fn prep(&self) -> Prepared {
        prepare(&self.input()).unwrap()
    }
    fn simlab(&self, stiffening: simlab_fea::Stiffening) -> StrengthResult {
        solve_strength(&self.input(), &SolveOptions { stiffening, ..Default::default() }, &Silent).unwrap()
    }
    /// CalculiX's answer through SimLab's postprocessing (same peak rules).
    fn reference(&self, frd: &Frd) -> StrengthResult {
        let n = self.mesh.nodes.len();
        let u: Vec<f64> = frd.displacements(n).unwrap().iter().flat_map(|d| *d).collect();
        postprocess_reference(&self.input(), self.prep(), u, Default::default(), true, frd.stresses(n).unwrap()).unwrap()
    }
}

/// Reaction on the leading half of the root (y > 0): a real load path, not
/// the near-zero total of a balanced part. Our reaction from our solution;
/// CalculiX's from its own nodal forces (`RF`) minus the load on those nodes.
fn half_root_reaction(case: &Case, ours: Option<&StrengthResult>, frd: Option<&Frd>) -> Vec3 {
    let prep = case.prep();
    let n = case.mesh.nodes.len();
    let mut r = [0.0; 3];
    // CalculiX's RF is the internal force; the load on the deformed part is f + the spin load's follower share
    let follow = frd.map(|frd| {
        let u: Vec<f64> = frd.displacements(n).unwrap().iter().flat_map(|d| *d).collect();
        simlab_fea::strength::spin_follow_load(&case.mesh, &prep, &u)
    });
    let fint = ours.map(|res| {
        let u: Vec<f64> = res.displacement.iter().flat_map(|d| *d).collect();
        let spin: Vec<(f64, Vec3)> = prep.parts.spins.iter().map(|(w, _, ax)| (prep.elastic.density * w * w, *ax)).collect();
        let st = simlab_fea::elem::Stiffness { el: &prep.elastic, large_u: Some(&u), prestress_u: None, spin: &spin };
        simlab_fea::assembly::internal_forces(&case.mesh, &st, &u, &vec![true; n])
    });
    for i in 0..n {
        if !prep.support_node[i] || case.mesh.nodes[i][1] <= 0.0 {
            continue;
        }
        for k in 0..3 {
            r[k] += match (&fint, frd) {
                (Some(f), _) => f[3 * i + k] - prep.f[3 * i + k],
                (None, Some(frd)) => frd.last("FORC").unwrap().values[&(i as u32 + 1)][k] - prep.f[3 * i + k] - follow.as_ref().unwrap()[3 * i + k],
                _ => unreachable!(),
            };
        }
    }
    r
}

fn check(case: &Case, frd: &Frd) {
    let n = case.mesh.nodes.len();
    let ours = case.simlab(simlab_fea::Stiffening::Auto);
    assert!(ours.solver.stiffening_passes > 0, "the nonlinear solve did not run");
    let refr = case.reference(frd);
    let c = compare(&ours.displacement, &ours.stress, &frd.displacements(n).unwrap(), &frd.stresses(n).unwrap());
    let eq = {
        let u: Vec<f64> = frd.displacements(n).unwrap().iter().flat_map(|d| *d).collect();
        equilibrium_error(frd, &case.mesh, &case.prep(), &u, true).unwrap()
    };
    let (ra, rb) = (half_root_reaction(case, Some(&ours), None), half_root_reaction(case, None, Some(frd)));
    let tip = (0..n).max_by(|&a, &b| norm(ours.displacement[a]).partial_cmp(&norm(ours.displacement[b])).unwrap()).unwrap();
    let ru = frd.displacements(n).unwrap();
    // a point on the blade away from the hold: the node nearest mid-span, top face, centre line
    let tilt = TILT_DEG.to_radians();
    let probe_x = [25.0 * tilt.cos() - 1.5 * tilt.sin(), 0.0, 25.0 * tilt.sin() + 1.5 * tilt.cos()].map(|v| v * 1e-3);
    let probe = (0..n).min_by(|&a, &b| norm(sub(case.mesh.nodes[a], probe_x)).partial_cmp(&norm(sub(case.mesh.nodes[b], probe_x))).unwrap()).unwrap();
    let vm = simlab_fea::material::von_mises;
    println!(
        "spin blade vs CalculiX NLGEOM: |Δu| {:.2e}, |Δσ_vM| {:.2e} (RMS {:.2e}) of peak; tip |u| {:.6e} vs {:.6e} m; headline peak {:.4} vs {:.4} MPa, SF {:.5} vs {:.5}; \
         half-root reaction [{:.4} {:.4} {:.4}] vs [{:.4} {:.4} {:.4}] N; mid-span vM {:.4} vs {:.4} MPa; CalculiX equilibrium {:.1e}",
        c.displacement,
        c.von_mises,
        c.von_mises_rms,
        norm(ours.displacement[tip]),
        norm(ru[tip]),
        ours.peaks.von_mises / 1e6,
        refr.peaks.von_mises / 1e6,
        ours.safety_factor,
        refr.safety_factor,
        ra[0],
        ra[1],
        ra[2],
        rb[0],
        rb[1],
        rb[2],
        vm(&ours.stress[probe]) / 1e6,
        vm(&frd.stresses(n).unwrap()[probe]) / 1e6,
        eq
    );
    assert!(c.displacement < 1e-3 && c.von_mises < 1e-3, "{c:?}");
    assert!(rel(norm(ours.displacement[tip]), norm(ru[tip])) < 1e-3);
    assert!(rel(ours.peaks.von_mises, refr.peaks.von_mises) < 1e-3);
    assert!(rel(ours.safety_factor, refr.safety_factor) < 1e-3);
    assert!(norm(sub(ra, rb)) < 1e-3 * norm(rb), "half-root reaction {ra:?} vs {rb:?}");
    assert!(rel(vm(&ours.stress[probe]), vm(&frd.stresses(n).unwrap()[probe])) < 1e-3);
    assert!(eq < 1e-2, "CalculiX's answer does not balance its loads: {eq:e}");
    // the case is nonlinear enough to tell the formulations apart: the
    // linear answer is far off, and so is small-strain stress
    let lin = case.simlab(simlab_fea::Stiffening::Off);
    assert!(rel(norm(lin.displacement[tip]), norm(ru[tip])) > 0.05);
    assert!(rel(lin.peaks.von_mises, refr.peaks.von_mises) > 0.01);
}

#[test]
fn matches_stored_calculix() {
    let case = Case::new();
    let frd = read_frd(&std::fs::read_to_string(FIXTURE).expect("fixture")).unwrap();
    check(&case, &frd);
}

/// CalculiX's equation solver must run single-threaded (see `ccx_env`).
#[test]
fn calculix_solver_runs_on_one_thread() {
    let env = simlab_fea::calculix::ccx_env(12);
    let get = |k: &str| env.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    assert_eq!(get("CCX_NPROC_EQUATION_SOLVER"), Some("1"));
    assert_eq!(get("CCX_NPROC_STIFFNESS"), Some("12"));
}

/// Same, with a fresh CalculiX run (Docker or ccx on PATH), run the way the
/// engine runs it; also the linear step, node by node.
#[test]
#[ignore]
fn matches_fresh_calculix() {
    let case = Case::new();
    let prep = case.prep();
    let dir = scratch().join("ccx").join("spin-blade-nlgeom");
    std::fs::create_dir_all(&dir).unwrap();
    write_inp_file(&dir.join("job.inp"), &case.mesh, &prep, &InpOptions { nlgeom: true, ..Default::default() }).unwrap();
    assert!(run_ccx(&dir, "job"), "CalculiX is not available (no ccx on PATH and no simlab-reference:1 image)");
    let text = std::fs::read_to_string(dir.join("job.frd")).unwrap();
    if std::env::var("SIMLAB_WRITE_FIXTURE").is_ok() {
        std::fs::write(FIXTURE, last_increment(&text)).unwrap();
    }
    check(&case, &read_frd(&text).unwrap());
    // linear: the plain small-strain problem
    let lin = case.simlab(simlab_fea::Stiffening::Off);
    let cmp = ccx_compare("spin-blade-linear", &case.mesh, &prep, &lin, true).unwrap();
    assert!(cmp.displacement < 1e-3 && cmp.von_mises < 1e-3, "{cmp:?}");
}

/// The `.frd` with only the mesh and the last increment's results (a
/// nonlinear step writes every increment).
fn last_increment(text: &str) -> String {
    let mut segs: Vec<String> = vec![String::new()];
    for l in text.lines() {
        if l.starts_with("    1PSTEP") {
            segs.push(String::new());
        }
        let s = segs.last_mut().unwrap();
        s.push_str(l);
        s.push('\n');
    }
    let name = |s: &str| s.lines().find(|l| l.starts_with(" -4")).and_then(|l| l.split_whitespace().nth(1)).unwrap_or("").to_string();
    let mut keep: Vec<&String> = vec![];
    let mut seen = std::collections::HashSet::new();
    for s in segs[1..].iter().rev() {
        if !seen.insert(name(s)) {
            break;
        }
        keep.push(s);
    }
    let mut out = segs[0].clone();
    for s in keep.iter().rev() {
        out.push_str(s.trim_end_matches(" 9999\n"));
    }
    out.push_str(" 9999\n");
    out
}
