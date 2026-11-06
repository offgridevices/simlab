//! Whole-pipeline validation through the public API (`run_with`).
//! Quick tests run by default; the slow ones are `#[ignore]`:
//!   cargo test --release -p simlab-cfd --test tunnel -- --ignored --nocapture --test-threads=1

use simlab_cfd::shapes::PropSpec;
use simlab_cfd::tunnel::SolverOptions;
use simlab_cfd::validation::*;
use simlab_cfd::{estimate, finest_fit};
use simlab_core::{Air, GridQuality, Silent, Spin, WindConfig};

fn sphere_check(re: f64, d: u32, passes: f64, tol: f64) {
    let (cd, r) = sphere_drag(re, d, passes, &SolverOptions::default(), &Silent).unwrap();
    let refc = if re > 250.0 { sphere_cd_cgw(re) } else { 0.5 * (sphere_cd_cgw(re) + sphere_cd_sn(re)) };
    let s = &r.summary;
    eprintln!(
        "sphere Re={re} D={d} cells: Cd {cd:.4} vs {refc:.4} ({:+.2} %), converged {}, {:.1} passes, blockage {:.1} %, {:.0} MLUPs",
        (cd / refc - 1.0) * 100.0,
        s.converged,
        s.simulated,
        s.blockage * 100.0,
        s.mlups
    );
    assert!((cd / refc - 1.0).abs() < tol, "Cd {cd} vs {refc}");
    // steady wakes are symmetric; a chaotic wake (Re >= 1000) keeps a side
    // force of a few percent of drag over a finite window
    let side = (s.lift_n * s.lift_n + s.side_n * s.side_n).sqrt() / s.drag_n.abs();
    eprintln!("  side force / drag {side:.3}");
    assert!(side < if re < 500.0 { 0.05 } else { 0.15 });
}

#[test]
fn sphere_re100_quick() {
    sphere_check(100.0, 16, 15.0, 0.05);
}

#[test]
#[ignore]
fn sphere_re20() {
    sphere_check(20.0, 32, 30.0, 0.05);
}

// 48+ cells across: still parts run on the refined D3Q27 solver
#[test]
#[ignore]
fn sphere_re100() {
    sphere_check(100.0, 48, 30.0, 0.03);
}

#[test]
#[ignore]
fn sphere_re300() {
    sphere_check(300.0, 48, 40.0, 0.06);
}

#[test]
#[ignore]
fn sphere_re1000() {
    sphere_check(1000.0, 96, 50.0, 0.05);
}

#[test]
#[ignore]
fn sphere_re10000() {
    // boundary layer thinner than a cell; the refined grid gets within ~5 %
    sphere_check(1e4, 96, 50.0, 0.08);
}

#[test]
#[ignore]
fn disk_normal_to_flow_re10000() {
    // sharp-edged plate: the wake is wide and slow to settle; reads ~10 % low
    let (cd, r) = disk_drag(1e4, 96, 50.0, &SolverOptions::default(), &Silent).unwrap();
    eprintln!("disk Re 1e4: Cd {cd:.4} vs 1.17 ({:+.1} %), converged {}, {} cells", (cd / 1.17 - 1.0) * 100.0, r.summary.converged, r.summary.cells);
    assert!((cd / 1.17 - 1.0).abs() < 0.15);
}

#[test]
#[ignore]
fn rotating_sphere_stokes_torque() {
    let mut o = SolverOptions::default();
    o.still_margins.side = 2.0;
    let (t, stokes, r) = rotating_sphere(1.0, 24, 6.0, &o, &Silent).unwrap();
    eprintln!("rotating sphere: torque {:.4e} vs 8 pi mu R^3 omega {:.4e} ({:+.2} %), thrust {:.2e}", -t, stokes, (-t / stokes - 1.0) * 100.0, r.summary.thrust_n.unwrap());
    assert!((-t / stokes - 1.0).abs() < 0.05);
}

#[test]
fn reversed_spin_mirrors_torque() {
    // negative rpm turns the part the other way: torque flips sign
    let spec = PropSpec::five_inch();
    let run = |rpm: f64| prop_hover(&spec, rpm, 24, 1.5, &SolverOptions::default(), &Silent).unwrap().summary;
    let a = run(20000.0);
    let b = run(-20000.0);
    eprintln!("torque +rpm {:.4}, -rpm {:.4}; power {:.1} / {:.1} W", a.torque_nm.unwrap(), b.torque_nm.unwrap(), a.power_w.unwrap(), b.power_w.unwrap());
    assert!(a.torque_nm.unwrap() < 0.0 && b.torque_nm.unwrap() > 0.0);
    assert!(a.power_w.unwrap() > 0.0 && b.power_w.unwrap() > 0.0);
}

#[test]
fn spinning_prop_smoke() {
    // a coarse, short run: thrust along +z, torque against the rotation
    let r = prop_hover(&PropSpec::five_inch(), 20000.0, 32, 2.0, &SolverOptions::default(), &Silent).unwrap();
    let s = &r.summary;
    eprintln!("prop smoke: thrust {:.3} N, torque {:.4} N·m, power {:.1} W", s.thrust_n.unwrap(), s.torque_nm.unwrap(), s.power_w.unwrap());
    assert!(s.thrust_n.unwrap() > 0.0);
    assert!(s.torque_nm.unwrap() < 0.0);
    assert!(s.power_w.unwrap() > 0.0);
    assert!(r.surface_pressure_pa.iter().filter(|p| p.is_finite()).count() > r.surface_pressure_pa.len() / 2);
}

#[test]
fn estimate_and_budget() {
    let (cfg, m) = sphere_case(1000.0, 32);
    let e = estimate(&cfg, &m, 1e-3, 8_000_000_000).unwrap();
    assert!(e.fits && e.cells > 100_000 && e.seconds > 0.0);
    let c8 = finest_fit(&cfg, &m, 1e-3, 8_000_000_000).unwrap();
    let c32 = finest_fit(&cfg, &m, 1e-3, 32_000_000_000).unwrap();
    eprintln!("sphere: finest cells across in 8 GB {c8}, in 32 GB {c32}");
    assert!(c32 > c8);
    let spin = WindConfig {
        wind_dir: [0.0, 0.0, 1.0],
        speed_m_s: 0.0,
        air: Air::default(),
        spin: Some(Spin { rpm: 20000.0, axis: [0.0, 0.0, 1.0], center_mm: [0.0; 3] }),
        quality: GridQuality::Normal,
        duration: None,
    };
    let e = estimate(&spin, &PropSpec::five_inch().mesh(), 1e-3, 32_000_000_000).unwrap();
    eprintln!("5-inch prop, Normal: {}", e.note);
    assert!(e.steps > 1000);
}

#[test]
fn headline_numbers_use_significant_figures() {
    use simlab_cfd::tunnel::sig;
    assert_eq!(sig(0.000123456, 3), "0.000123");
    assert_eq!(sig(1234.5, 3), "1230");
    assert_eq!(sig(2.0, 3), "2.00");
    assert_eq!(sig(-0.0456, 2), "-0.046");
}

fn wind_cfg(q: GridQuality) -> WindConfig {
    WindConfig { wind_dir: [1.0, 0.0, 0.0], speed_m_s: 15.0, air: Air::default(), spin: None, quality: q, duration: None }
}

#[test]
fn slender_parts_get_cells_across_their_thickness() {
    use simlab_cfd::shapes::{cuboid, sphere};
    use simlab_cfd::{cells_for_thin, thin_cells_target};
    // a 150 x 12 x 6 mm arm broadside to the wind: 12 mm thick across it
    let arm = cuboid([-0.003, -0.075, -0.006], [0.003, 0.075, 0.006]);
    for q in [GridQuality::Draft, GridQuality::Normal] {
        let e = estimate(&wind_cfg(q.clone()), &arm, 1e-3, 32_000_000_000).unwrap();
        eprintln!("arm {q:?}: {} across, {:.1} across the 12 mm, {}", e.cells_across, e.cells_thin, e.note);
        assert!((e.thin_mm - 12.0).abs() < 0.01);
        assert!(e.cells_thin >= thin_cells_target(&q) as f64 - 0.5, "{} cells across the thickness", e.cells_thin);
        assert!(e.thin_warning.is_none());
    }
    // a sphere is not slender: the quality's grid, unchanged
    let ball = sphere([0.0; 3], 0.01, 4);
    let e = estimate(&wind_cfg(GridQuality::Draft), &ball, 1e-3, 32_000_000_000).unwrap();
    assert_eq!(e.cells_across, 64);
    // a small memory budget caps the refinement, and the estimate says so
    let e = estimate(&wind_cfg(GridQuality::Normal), &arm, 1e-3, 600_000_000).unwrap();
    eprintln!("arm Normal in 0.6 GB: {:.1} across the 12 mm: {:?}", e.cells_thin, e.thin_warning);
    assert!(e.cells_thin < 24.0 && e.thin_warning.is_some() && e.memory_bytes <= 600_000_000);
    // a custom grid is the caller's choice: kept, with a warning when too coarse
    let e = estimate(&wind_cfg(GridQuality::Custom { cells_across: 64 }), &arm, 1e-3, 32_000_000_000).unwrap();
    assert_eq!(e.cells_across, 64);
    assert!(e.thin_warning.is_some());
    assert_eq!(cells_for_thin(64, 16, 0.15, 0.012), 200);
    assert_eq!(cells_for_thin(64, 16, 0.02, 0.02), 64);
    assert_eq!(cells_for_thin(64, 16, 1.0, 0.001), 64 * simlab_cfd::THIN_MAX_BOOST);
}
