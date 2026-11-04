//! The user's earlier OpenFOAM study of 3-inch props (steady RANS, MRF,
//! k-omega SST, ~1.86 M cells, incompressible, rho 1.204, nu 1.5e-5):
//! extraction + BEMT from the same STLs against its thrust and torque. This
//! is code-to-code (the CFD is fully turbulent and itself unvalidated).
//!
//! The STLs are not in the repo: set SIMLAB_PROP_STUDY to the folder that
//! holds `stl/E-v5.stl` etc., then
//!   cargo test --release -p simlab-prop --test cfd_3inch -- --ignored --nocapture

use simlab_core::Air;
use simlab_prop::*;

/// (test, STL, rpm, thrust g, torque mN m) from "All test results (tests.csv)"
const CFD: &[(&str, &str, f64, f64, f64)] = &[
    ("T003", "E-v5", 17000.0, 45.8, 3.12),
    ("T001", "E-v5", 25000.0, 101.0, 6.67),
    ("T002", "E-v5", 45800.0, 346.0, 22.09),
    ("T010", "A-v2", 17000.0, 47.0, 3.65),
    ("T008", "A-v2", 45200.0, 342.5, 24.96),
    ("T011", "P-v1", 17000.0, 46.4, 3.93),
    ("T009", "P-v1", 45200.0, 335.0, 26.89),
    ("T023", "P-v2-e011", 17000.0, 49.9, 3.45),
    ("T021", "P-v2-e011", 45800.0, 378.4, 24.66),
    ("T020", "P-v2", 17000.0, 48.0, 3.82),
    ("T019", "P-v2", 45800.0, 362.7, 27.35),
    ("T018", "A3-p18", 17000.0, 50.1, 3.46),
    ("T014", "A3-p18", 45800.0, 379.0, 24.63),
    ("T017", "A3-p20", 45800.0, 412.6, 28.01),
    ("T012", "E-v5-T110", 45800.0, 344.7, 21.78),
    ("T013", "E-v5-T90", 45800.0, 342.6, 21.55),
];

#[test]
#[ignore]
fn openfoam_3inch_props() {
    let Ok(dir) = std::env::var("SIMLAB_PROP_STUDY") else {
        eprintln!("SIMLAB_PROP_STUDY not set; skipping");
        return;
    };
    // the CFD was incompressible: compare like with like, and also show the
    // default (compressible) model
    let air = Air { density: 1.204, kinematic_viscosity: 1.5e-5 };
    let incompressible = ModelOptions { compressibility: false, ..Default::default() };
    let default = ModelOptions::default();
    let mut cache: std::collections::HashMap<&str, BladeGeometry> = Default::default();
    println!("| test | prop | rpm | CFD thrust g | model g | err | CFD torque mN m | model | err | model g (with Mach) | err |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let (mut et, mut eq) = (vec![], vec![]);
    for &(test, stl, rpm, tg, tq) in CFD {
        let g = cache.entry(stl).or_insert_with(|| {
            let path = format!("{dir}/stl/{stl}.stl");
            let mesh = simlab_core::stl::read_stl(&std::fs::read(&path).expect(&path)).unwrap();
            let guess = detect_prop(&mesh).expect("prop");
            let ex = extract_geometry(&mesh, 1e-3, guess.axis, guess.center, &ExtractOptions::default()).unwrap();
            eprintln!("{stl}: {} blades, D {:.2} mm, pitch75 {:.1} mm, axis {:?}", ex.geometry.blades, ex.geometry.radius * 2e3, ex.geometry.pitch_75() * 1e3, guess.axis);
            ex.geometry
        });
        let p = solve(g, rpm, 0.0, &air, &incompressible);
        let pm = solve(g, rpm, 0.0, &air, &default);
        let (a, b, c) = ((p.thrust_g - tg) / tg, (p.torque_nm * 1e3 - tq) / tq, (pm.thrust_g - tg) / tg);
        et.push(a);
        eq.push(b);
        println!(
            "| {test} | {stl} | {rpm:.0} | {tg:.1} | {:.1} | {:+.1}% | {tq:.2} | {:.2} | {:+.1}% | {:.1} | {:+.1}% |",
            p.thrust_g,
            100.0 * a,
            p.torque_nm * 1e3,
            100.0 * b,
            pm.thrust_g,
            100.0 * c
        );
    }
    let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
    println!("\nRMS error vs CFD (incompressible model): thrust {:.1}%, torque {:.1}%", 100.0 * rms(&et), 100.0 * rms(&eq));
}
