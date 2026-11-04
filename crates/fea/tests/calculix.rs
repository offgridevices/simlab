//! CalculiX deck writer and .frd reader.
mod common;
use common::*;
use simlab_core::{Load, Material, Support};
use simlab_fea::calculix::{InpOptions, compare, read_frd, write_inp};
use simlab_fea::mesh_gen::{Midside, box_mesh};
use simlab_fea::{StrengthInput, prepare};

fn small() -> simlab_core::TetMesh {
    box_mesh(4, 1, 1, |p| [p[0] * 0.04, p[1] * 0.01, p[2] * 0.01], Midside::Mapped)
}

#[test]
fn deck_has_the_right_cards() {
    let mesh = small();
    let mut mat = Material::isotropic("m", "m", 1270.0, 2000.0, 0.38, 50.0);
    mat.e_z_mpa = 1400.0;
    let c = cfg(
        Support::Fixed { patches: vec![1] },
        vec![
            Load::Pressure { patches: vec![6], pressure_pa: 1e5, label: String::new() },
            Load::Acceleration { g: [0.0, 0.0, -1.0], label: String::new() },
            Load::Spin { rpm: 1000.0, axis: [1.0, 0.0, 0.0], center_mm: [0.0; 3], label: String::new() },
            Load::Force { patches: vec![2], force_n: [0.0, 0.0, -10.0], label: String::new() },
        ],
        [1.0, 0.0, 0.0],
    );
    let input = StrengthInput { mesh: &mesh, material: &mat, config: &c, model_unit_m: 0.001, face_tractions: None };
    let prep = prepare(&input).unwrap();
    let mut buf = Vec::new();
    write_inp(&mut buf, &mesh, &prep, &InpOptions::default()).unwrap();
    let s = String::from_utf8(buf).unwrap();
    for card in ["*NODE, NSET=NALL", "*ELEMENT, TYPE=C3D10", "*ELASTIC, TYPE=ENGINEERING CONSTANTS", "*ORIENTATION", "*BOUNDARY", "*CLOAD", "GRAV", "CENTRIF", "*DLOAD", "*STATIC", "*EL FILE"] {
        assert!(s.contains(card), "missing {card}");
    }
    let n_nodes = s.lines().skip_while(|l| !l.starts_with("*NODE")).skip(1).take_while(|l| !l.starts_with('*')).count();
    assert_eq!(n_nodes, mesh.nodes.len());
    let n_el = s.lines().skip_while(|l| !l.starts_with("*ELEMENT")).skip(1).take_while(|l| !l.starts_with('*')).count();
    assert_eq!(n_el, mesh.tets.len());
    // pressure faces: as many P cards as pressure faces
    let p_cards = s.lines().filter(|l| l.split(", ").nth(1).is_some_and(|t| t.len() == 2 && t.starts_with('P'))).count();
    assert_eq!(p_cards, prep.parts.pressure.len());
    // E_xy, E_xy, E_z first on the engineering-constants line
    let ec = s.lines().skip_while(|l| !l.starts_with("*ELASTIC")).nth(1).unwrap();
    let v: Vec<f64> = ec.split(',').filter(|t| !t.trim().is_empty()).map(|t| t.trim().parse().unwrap()).collect();
    assert!((v[0] - 2e9).abs() < 1.0 && (v[2] - 1.4e9).abs() < 1.0 && (v[3] - 0.38).abs() < 1e-12);
    // all-nodal variant: CLOAD sums to the total applied force
    let mut buf = Vec::new();
    write_inp(&mut buf, &mesh, &prep, &InpOptions { native_loads: false, ..Default::default() }).unwrap();
    let s = String::from_utf8(buf).unwrap();
    let mut tot = [0.0; 3];
    for l in s.lines().skip_while(|l| !l.starts_with("*CLOAD")).skip(1).take_while(|l| !l.starts_with('*')) {
        let t: Vec<&str> = l.split(',').map(|t| t.trim()).collect();
        let k: usize = t[1].parse().unwrap();
        tot[k - 1] += t[2].parse::<f64>().unwrap();
    }
    for k in 0..3 {
        assert!((tot[k] - prep.applied_force_n[k]).abs() < 1e-9 * (1.0 + prep.applied_force_n[k].abs()));
    }
    assert!(!s.contains("GRAV") && !s.contains("CENTRIF"));
}

const FRD: &str = "    1C
    1UUSER
    2C                             3                                     1
 -1         1 0.00000E+00 0.00000E+00 0.00000E+00
 -1         2 1.00000E-03 0.00000E+00 0.00000E+00
 -1         3 0.00000E+00 1.00000E-03-2.50000E-04
 -3
    3C                             1                                     1
 -1         1    6    0    1
 -2         1         2         3
 -3
    1PSTEP                         1           1           1
  100CL  101 1.000000000           3                     0    1           1
 -4  DISP        4    1
 -5  D1          1    2    1    0
 -5  D2          1    2    2    0
 -5  D3          1    2    3    0
 -5  ALL         1    2    0    0    1ALL
 -1         1 0.00000E+00 0.00000E+00 0.00000E+00
 -1         2 1.50000E-06-2.00000E-07 3.00000E-08
 -1         3-1.00000E-06 4.00000E-06 0.00000E+00
 -3
  100CL  101 1.000000000           3                     0    1           1
 -4  STRESS      6    1
 -5  SXX         1    4    1    1
 -5  SYY         1    4    2    2
 -5  SZZ         1    4    3    3
 -5  SXY         1    4    1    2
 -5  SYZ         1    4    2    3
 -5  SZX         1    4    3    1
 -1         1 1.00000E+06 2.00000E+06 3.00000E+06 4.00000E+05 5.00000E+05-6.00000E+05
 -1         2-1.00000E+06 0.00000E+00 0.00000E+00 0.00000E+00 0.00000E+00 0.00000E+00
 -1         3 0.00000E+00 0.00000E+00 0.00000E+00 0.00000E+00 0.00000E+00 1.23450E+02
 -3
 9999
";

#[test]
fn reads_frd() {
    let frd = read_frd(FRD).unwrap();
    assert_eq!(frd.nodes.len(), 3);
    assert_eq!(frd.nodes[&3], [0.0, 1e-3, -2.5e-4]);
    let u = frd.displacements(3).unwrap();
    assert_eq!(u[1], [1.5e-6, -2e-7, 3e-8]);
    assert_eq!(u[2], [-1e-6, 4e-6, 0.0]);
    let s = frd.stresses(3).unwrap();
    assert_eq!(s[0], [1e6, 2e6, 3e6, 4e5, 5e5, -6e5]);
    assert_eq!(s[2][5], 123.45);
    assert_eq!(frd.last("STRESS").unwrap().components, ["SXX", "SYY", "SZZ", "SXY", "SYZ", "SZX"]);
    let c = compare(&u, &s, &u, &s);
    assert!(c.displacement == 0.0 && c.von_mises == 0.0);
}

#[test]
fn reads_short_format_frd() {
    let short = "    2C                             1                                     0
 -1    1 1.00000E+00 2.00000E+00 3.00000E+00
 -3
  100CL  101 1.000000000           1                     0    1           0
 -4  DISP        4    1
 -5  D1          1    2    1    0
 -5  D2          1    2    2    0
 -5  D3          1    2    3    0
 -5  ALL         1    2    0    0    1ALL
 -1    1 1.00000E-03 2.00000E-03 3.00000E-03
 -3
 9999
";
    let frd = read_frd(short).unwrap();
    assert_eq!(frd.nodes[&1], [1.0, 2.0, 3.0]);
    assert_eq!(frd.displacements(1).unwrap()[0], [1e-3, 2e-3, 3e-3]);
}

/// Runs CalculiX (ccx on PATH, or the simlab-reference:1 Docker image) on
/// an anisotropic cantilever with every load type and compares node by node.
#[test]
#[ignore]
fn calculix_matches() {
    let mesh = box_mesh(20, 3, 3, |p| [p[0] * 0.06, p[1] * 0.01, p[2] * 0.008], Midside::Mapped);
    let mut mat = Material::isotropic("m", "m", 1270.0, 2000.0, 0.38, 50.0);
    mat.e_z_mpa = 1400.0;
    mat.uts_z_mpa = 30.0;
    let c = cfg(
        Support::Fixed { patches: vec![1] },
        vec![
            Load::Force { patches: vec![2], force_n: [0.0, 3.0, -20.0], label: String::new() },
            Load::Acceleration { g: [0.0, 0.0, -5.0], label: String::new() },
            Load::Pressure { patches: vec![6], pressure_pa: 2e4, label: String::new() },
            Load::Spin { rpm: 3000.0, axis: [0.0, 0.0, 1.0], center_mm: [0.0, 5.0, 0.0], label: String::new() },
        ],
        [0.3, 0.0, 1.0],
    );
    let input = StrengthInput { mesh: &mesh, material: &mat, config: &c, model_unit_m: 0.001, face_tractions: None };
    let prep = prepare(&input).unwrap();
    let opts = simlab_fea::SolveOptions { stiffening: simlab_fea::Stiffening::Off, ..Default::default() };
    let r = simlab_fea::solve_strength(&input, &opts, &simlab_core::Silent).unwrap();
    let cmp = ccx_compare("all-loads", &mesh, &prep, &r, true).expect("CalculiX not available");
    assert!(cmp.displacement < 1e-3 && cmp.von_mises < 1e-3, "{cmp:?}");
}
