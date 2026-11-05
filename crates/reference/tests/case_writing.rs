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
