//! Case writing without Docker: every file is there and looks like a valid
//! OpenFOAM dictionary.

use simlab_core::{Air, GridQuality, Spin, WindConfig};
use simlab_reference::{geom::icosphere, plan::Kind, prepare_case};
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("simlab-ref-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// braces and parentheses balance outside comments and strings
fn balanced(text: &str) -> bool {
    let (mut b, mut p) = (0i64, 0i64);
    let mut in_str = false;
    for line in text.lines() {
        let line = line.split("//").next().unwrap();
        for c in line.chars() {
            match c {
                '"' => in_str = !in_str,
                '{' if !in_str => b += 1,
                '}' if !in_str => b -= 1,
                '(' if !in_str => p += 1,
                ')' if !in_str => p -= 1,
                _ => {}
            }
            if b < 0 || p < 0 {
                return false;
            }
        }
    }
    b == 0 && p == 0 && !in_str
}

fn check_dicts(dir: &Path, files: &[&str]) {
    for f in files {
        let p = dir.join(f);
        let t = std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("{f} missing"));
        if !f.ends_with(".points") {
            assert!(t.contains("FoamFile"), "{f} has no FoamFile header");
        }
        assert!(balanced(&t), "{f} is not balanced");
    }
}

fn wind(speed: f64, q: GridQuality) -> WindConfig {
    WindConfig { wind_dir: [1.0, 0.0, 0.0], speed_m_s: speed, air: Air::default(), spin: None, quality: q, duration: None }
}

#[test]
fn wind_case_is_complete() {
    let dir = tmp("wind");
    let s = icosphere([0.01, 0.0, 0.02], 0.01, 3);
    let cfg = wind(10.0, GridQuality::Normal);
    let prep = prepare_case(&s, &cfg, 0.001, &dir, 8).unwrap();
    assert_eq!(prep.plan.kind, Kind::Wind);
    check_dicts(
        &dir,
        &[
            "system/controlDict", "system/fvSchemes", "system/fvSolution", "system/blockMeshDict",
            "system/snappyHexMeshDict", "system/surfaceFeatureExtractDict", "system/decomposeParDict",
            "system/sampleDict", "constant/transportProperties", "constant/turbulenceProperties",
            "0.orig/U", "0.orig/p", "0.orig/k", "0.orig/omega", "0.orig/nut",
            "system/vertices.points", "system/side.points", "system/top.points",
        ],
    );
    assert!(dir.join("constant/triSurface/part.stl").metadata().unwrap().len() > 84);
    assert!(!dir.join("constant/MRFProperties").exists());
    // every outer patch has an entry in every field
    for f in ["U", "p", "k", "omega", "nut"] {
        let t = std::fs::read_to_string(dir.join("0.orig").join(f)).unwrap();
        for patch in ["part", "inlet", "outlet", "sideA", "sideB", "sideC", "sideD"] {
            assert!(t.contains(&format!("    {patch} {{")), "{f} lacks {patch}");
        }
    }
    let u = std::fs::read_to_string(dir.join("0.orig/U")).unwrap();
    assert!(u.contains("inlet { type fixedValue; value uniform (1.000000000e1 0 0); }"), "{u}");
    let sn = std::fs::read_to_string(dir.join("system/snappyHexMeshDict")).unwrap();
    assert!(sn.contains("addLayers true;") && sn.contains("nSurfaceLayers 3;"));
    assert!(sn.contains("level (5 6)"));
    let bm = std::fs::read_to_string(dir.join("system/blockMeshDict")).unwrap();
    assert!(bm.contains("hex (0 1 2 3 4 5 6 7)"));
    let cd = std::fs::read_to_string(dir.join("system/controlDict")).unwrap();
    assert!(cd.contains("forceCoeffs") && cd.contains("stopAt endTime;"));
    let ar = std::fs::read_to_string(dir.join("Allrun")).unwrap();
    assert!(ar.contains("-np 8") && ar.contains("snappyHexMesh -parallel"));
    // the box: 4 L upstream, 10 L downstream, 4 L to the sides; 20 mm sphere
    let p = &prep.plan;
    assert!((p.h0 - 0.004).abs() < 1e-9);
    assert!(p.lo[0] <= -0.01 - 0.08 && p.hi[0] >= 0.01 + 0.2);
    assert!((p.ref_area - std::f64::consts::PI * 1e-4).abs() < 3e-6, "{}", p.ref_area);
    assert!(prep.estimated_cells > 100_000 && prep.estimated_cells < 5_000_000, "{}", prep.estimated_cells);
    // case.json is valid JSON
    let _: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("case.json")).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn low_reynolds_is_laminar_and_serial() {
    let dir = tmp("lam");
    let s = icosphere([0.0; 3], 0.01, 3);
    let mut cfg = wind(0.0758, GridQuality::Draft); // Re 100
    cfg.wind_dir = [0.0, 1.0, 1.0];
    let prep = prepare_case(&s, &cfg, 0.001, &dir, 1).unwrap();
    assert_eq!(prep.plan.turbulence_name(), "laminar");
    assert!(!dir.join("0.orig/k").exists());
    let t = std::fs::read_to_string(dir.join("constant/turbulenceProperties")).unwrap();
    assert!(t.contains("simulationType laminar;"));
    let ar = std::fs::read_to_string(dir.join("Allrun")).unwrap();
    assert!(!ar.contains("mpirun") && ar.contains("step solve simpleFoam"));
    check_dicts(&dir, &["system/controlDict", "system/snappyHexMeshDict", "0.orig/U", "0.orig/p"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hovering_rotor_gets_mrf_and_open_boundaries() {
    let dir = tmp("hover");
    // a stand-in "rotor": a flat disc-ish sphere, 38 mm radius about z
    let mut s = icosphere([0.0; 3], 0.038, 3);
    for p in &mut s.positions {
        p[2] *= 0.05;
    }
    let cfg = WindConfig {
        wind_dir: [0.0, 0.0, -1.0],
        speed_m_s: 0.0,
        air: Air::default(),
        spin: Some(Spin { rpm: 17000.0, axis: [0.0, 0.0, 1.0], center_mm: [0.0, 0.0, 0.0] }),
        quality: GridQuality::Normal,
        duration: None,
    };
    let prep = prepare_case(&s, &cfg, 0.001, &dir, 4).unwrap();
    let p = &prep.plan;
    assert_eq!(p.kind, Kind::SpinHover);
    let r = p.rotor.as_ref().unwrap();
    assert!((r.omega - 17000.0 * std::f64::consts::TAU / 60.0).abs() < 1e-9);
    assert!((r.zone_radius - 1.085 * 0.038).abs() < 1e-6);
    // the proven propeller-study background cell for R = 38.245 mm was 10 mm
    assert!((p.h0 / 0.038 - 0.2615).abs() < 1e-9);
    check_dicts(&dir, &["constant/MRFProperties", "system/snappyHexMeshDict", "0.orig/U", "0.orig/p", "0.orig/k", "system/axial.points", "system/disc.points"]);
    let mrf = std::fs::read_to_string(dir.join("constant/MRFProperties")).unwrap();
    assert!(mrf.contains("cellZone rotorZone;") && mrf.contains("axis (0 0 1.000000000e0);"), "{mrf}");
    let pf = std::fs::read_to_string(dir.join("0.orig/p")).unwrap();
    assert_eq!(pf.matches("totalPressure").count(), 6);
    let uf = std::fs::read_to_string(dir.join("0.orig/U")).unwrap();
    assert_eq!(uf.matches("pressureInletOutletVelocity").count(), 6);
    let sn = std::fs::read_to_string(dir.join("system/snappyHexMeshDict")).unwrap();
    assert!(sn.contains("cellZone rotorZone") && sn.contains("addLayers false;") && sn.contains("level (6 6)"));
    let cd = std::fs::read_to_string(dir.join("system/controlDict")).unwrap();
    assert!(!cd.contains("forceCoeffs"), "no coefficients in still air");
    // negative rpm flips the rotation vector
    let mut cfg2 = cfg.clone();
    cfg2.spin.as_mut().unwrap().rpm = -17000.0;
    let p2 = simlab_reference::plan::plan(&s, &cfg2, 0.001).unwrap();
    assert_eq!(p2.rotor.unwrap().axis, [0.0, 0.0, -1.0]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nothing_to_simulate_is_an_error() {
    let s = icosphere([0.0; 3], 0.01, 2);
    assert!(simlab_reference::plan::plan(&s, &wind(0.0, GridQuality::Draft), 0.001).is_err());
}

/// The model follows the Reynolds number: laminar below 300, k-ω SST to
/// 5000, SST with Langtry–Menter transition above; all steady unless a run
/// length is asked for. The transition model gets its two extra fields.
#[test]
fn still_parts_pick_the_model_by_reynolds_number() {
    let s = icosphere([0.0; 3], 0.01, 3);
    let nu = Air::default().kinematic_viscosity;
    let at = |re: f64| simlab_reference::plan::plan(&s, &wind(re * nu / 0.02, GridQuality::Draft), 0.001).unwrap();
    assert_eq!(at(100.0).turbulence_name(), "laminar");
    assert_eq!(at(100.0).layers, 0, "the validated Re 100 Draft mesh is kept");
    let p = at(1000.0);
    assert_eq!(p.turbulence_name(), "k-omega SST");
    assert!(p.transient.is_none());
    assert_eq!(p.layers, 3, "the wall is resolved above Re 300 at every quality");
    let p = at(2.0e4);
    assert_eq!(p.turbulence_name(), "k-omega SST with Langtry-Menter transition");
    assert!(p.transient.is_none());
    // free-stream transition onset for 0.5 % turbulence (Langtry–Menter)
    assert!((simlab_reference::plan::re_theta_t(0.005) - 879.67).abs() < 0.1);

    let dir = tmp("lm");
    let prep = prepare_case(&s, &wind(2.0e4 * nu / 0.02, GridQuality::Draft), 0.001, &dir, 8).unwrap();
    assert!(prep.plan.transient.is_none());
    check_dicts(&dir, &["0.orig/ReThetat", "0.orig/gammaInt", "constant/turbulenceProperties", "system/fvSchemes", "system/fvSolution"]);
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    assert!(read("constant/turbulenceProperties").contains("RASModel kOmegaSSTLM;"));
    for f in ["ReThetat", "gammaInt"] {
        let t = read(&format!("0.orig/{f}"));
        for patch in ["part", "inlet", "outlet", "sideA", "sideB", "sideC", "sideD"] {
            assert!(t.contains(&format!("    {patch} {{")), "{f} lacks {patch}");
        }
        assert!(t.contains("part { type zeroGradient; }"));
    }
    assert!(read("0.orig/ReThetat").contains("inlet { type fixedValue; value uniform 8.79674"));
    assert!(read("system/fvSchemes").contains("div(phi,ReThetat) bounded Gauss upwind;"));
    assert!(read("system/fvSolution").contains("(U|k|omega|ReThetat|gammaInt)"));
    let ar = read("Allrun");
    assert!(ar.contains("step solve mpirun") && ar.contains("simpleFoam -parallel") && !ar.contains("pimpleFoam"));
    assert!(!dir.join("system/controlDict.init").exists());
    std::fs::remove_dir_all(&dir).ok();
}

/// Asking for a run length (flow passes) gives a time-accurate run: a
/// steady start with the same model (the `.init` files), then pimpleFoam,
/// forces averaged after the start-up, mean fields sampled.
#[test]
fn a_run_length_gives_a_time_accurate_run() {
    let dir = tmp("transient");
    let s = icosphere([0.0; 3], 0.01, 3);
    let mut cfg = wind(15.0, GridQuality::Draft); // 20 mm sphere, Re ≈ 19,800
    cfg.duration = Some(24.0);
    let prep = prepare_case(&s, &cfg, 0.001, &dir, 8).unwrap();
    let p = &prep.plan;
    let t = p.transient.as_ref().expect("time-accurate");
    assert_eq!(p.turbulence_name(), "k-omega SST with Langtry-Menter transition");
    assert!((t.flow_time - 0.02 / 15.0).abs() < 1e-12);
    let (skip, min_avg, _) = simlab_reference::plan::transient_passes(&GridQuality::Draft);
    assert!((t.end_time / t.flow_time - 24.0).abs() < 1e-9);
    assert!((t.average_from / t.flow_time - skip).abs() < 1e-9);
    assert!((t.min_average / t.flow_time - min_avg).abs() < 1e-9);
    assert!(t.average_from + t.min_average < t.end_time);
    assert_eq!((t.max_co, t.outer_correctors), (4.0, 2));
    assert!(t.dt_estimate > 1e-5 && t.dt_estimate < 1e-4, "{}", t.dt_estimate);
    assert!(t.steps() > 300 && t.steps() < 5000, "{}", t.steps());
    check_dicts(
        &dir,
        &[
            "system/controlDict", "system/controlDict.init", "system/fvSchemes", "system/fvSchemes.init",
            "system/fvSolution", "system/fvSolution.init", "constant/turbulenceProperties",
            "constant/turbulenceProperties.init", "system/sampleDict", "0.orig/k", "0.orig/omega", "0.orig/nut",
            "0.orig/ReThetat", "0.orig/gammaInt",
        ],
    );
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    let cd = read("system/controlDict");
    assert!(cd.contains("application pimpleFoam;") && cd.contains("adjustTimeStep yes;") && cd.contains("maxCo 4;"));
    assert!(cd.contains("type fieldAverage;") && cd.contains(&format!("timeStart {:.9e};", t.average_from)));
    let init = read("system/controlDict.init");
    assert!(init.contains("application simpleFoam;") && init.contains("endTime 300;"));
    assert!(!init.contains("fieldAverage"));
    assert!(read("constant/turbulenceProperties.init").contains("RASModel kOmegaSSTLM;"));
    assert!(read("system/fvSchemes").contains("ddtSchemes { default backward; }"));
    assert!(read("system/fvSchemes.init").contains("steadyState"));
    assert!(read("system/fvSolution").contains("nOuterCorrectors 2;") && read("system/fvSolution.init").contains("SIMPLE"));
    assert!(read("system/sampleDict").contains("fields (pMean UMean);"));
    let ar = read("Allrun");
    let (i_init, i_solve, i_sample) = (ar.find("step init").unwrap(), ar.find("step solve").unwrap(), ar.find("step sample").unwrap());
    assert!(i_init < i_solve && i_solve < i_sample);
    assert!(ar.contains("simpleFoam -parallel") && ar.contains("pimpleFoam -parallel") && ar.contains("'(pMean UMean)'"));
    // the init swap restores the time-accurate files before pimpleFoam
    assert!(ar.find("mv $f.run $f").unwrap() < i_solve);
    std::fs::remove_dir_all(&dir).ok();

    // a short run averages over its second half
    cfg.duration = Some(10.0);
    let t = simlab_reference::plan::plan(&s, &cfg, 0.001).unwrap().transient.unwrap();
    assert!((t.end_time / t.flow_time - 10.0).abs() < 1e-9);
    assert!((t.average_from / t.flow_time - 5.0).abs() < 1e-9);
}

/// Spinning parts keep the validated steady MRF method.
#[test]
fn spinning_parts_stay_steady() {
    let s = icosphere([0.0; 3], 0.038, 2);
    let mut cfg = wind(10.0, GridQuality::Normal);
    cfg.spin = Some(Spin { rpm: 20000.0, axis: [1.0, 0.0, 0.0], center_mm: [0.0, 0.0, 0.0] });
    let p = simlab_reference::plan::plan(&s, &cfg, 0.001).unwrap();
    assert!(p.transient.is_none());
    assert_eq!(p.turbulence_name(), "k-omega SST");
    cfg.speed_m_s = 0.0;
    let p = simlab_reference::plan::plan(&s, &cfg, 0.001).unwrap();
    assert!(p.transient.is_none() && p.kind == Kind::SpinHover);
}

#[test]
fn slender_parts_are_meshed_across_their_thickness() {
    use simlab_reference::plan::{THIN_ACCEPT, plan, thin_cells_target, thin_level};
    // a 150 x 12 x 6 mm arm broadside to the wind
    let mut arm = simlab_core::TriMesh::default();
    let (lo, hi) = ([-0.003, -0.075, -0.006], [0.003, 0.075, 0.006]);
    let c = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
    arm.positions = (0..8).map(c).collect();
    arm.triangles = vec![[0, 2, 3], [0, 3, 1], [4, 5, 7], [4, 7, 6], [0, 1, 5], [0, 5, 4], [2, 6, 7], [2, 7, 3], [0, 4, 6], [0, 6, 2], [1, 3, 7], [1, 7, 5]];
    for q in [GridQuality::Draft, GridQuality::Normal] {
        let p = plan(&arm, &wind(15.0, q.clone()), 0.001).unwrap();
        eprintln!("arm {q:?}: surface level {:?}, {:.1} cells across the 12 mm", p.surface_level, p.cells_thin);
        assert!((p.thin - 0.012).abs() < 1e-9);
        assert!(p.cells_thin >= THIN_ACCEPT * thin_cells_target(&q) as f64);
        assert!(p.warnings.iter().all(|w| !w.contains("thin across the wind")));
    }
    // a sphere keeps the quality's levels
    let s = icosphere([0.0; 3], 0.01, 4);
    let p = plan(&s, &wind(15.0, GridQuality::Draft), 0.001).unwrap();
    assert_eq!(p.surface_level, (4, 5));
    // background cells 30 mm, 12 mm thick: 12 cells (0.75 x 16) need 1 mm cells, level 5 (0.94 mm)
    assert_eq!(thin_level(0.030, 0.012, 16), 5);
    assert_eq!(thin_level(0.030, 0.012, 24), 6);
    // a 20 mm sphere (background 4 mm) needs no more than its Draft level 4
    assert!(thin_level(0.004, 0.020, 16) <= 4);
}
