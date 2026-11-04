//! BEMT against measured small-propeller data from the UIUC Propeller
//! Database, Volume 2 (R. W. Deters, G. K. Ananda, M. S. Selig, "Reynolds
//! Number Effects on the Performance of Small-Scale Propellers", AIAA
//! 2014-2151; data: https://m-selig.ae.illinois.edu/props/volume-2/propDB-volume-2.html).
//! The files in tests/data/uiuc are verbatim copies; see SOURCE.txt there.
//!
//! Run `cargo test -p simlab-prop --test uiuc -- --nocapture` for the tables.

mod common;

use common::*;
use simlab_core::Air;
use simlab_prop::*;

#[test]
fn uiuc_static_and_advancing() {
    let air = Air::default();
    let opts = ModelOptions::default();
    let mut summary = vec![];
    for case in cases() {
        let g = case.geometry();
        let r = compare(&case, &g, &air, &opts);
        eprintln!("{}", r.table());
        summary.push(r);
    }
    eprintln!("\n{}", summary_table(&summary));
    for r in &summary {
        // regression guards set just above today's agreement (see the table).
        // The 3D-printed DA props have a published airfoil; the moulded ones
        // use one assumed section, so they get a looser bound.
        let known = r.section_note.contains("stated");
        let (ct, cp, adv) = if known { (0.12, 0.30, 0.15) } else { (0.45, 0.47, 0.32) };
        assert!(r.static_ct_mean.abs() < ct, "{}: static CT off by {:.0}%", r.name, 100.0 * r.static_ct_mean);
        assert!(r.static_cp_mean.abs() < cp, "{}: static CP off by {:.0}%", r.name, 100.0 * r.static_cp_mean);
        assert!(r.adv_ct_rms < adv && r.adv_cp_rms < adv, "{}: J-sweep off by {:.0}% / {:.0}%", r.name, 100.0 * r.adv_ct_rms, 100.0 * r.adv_cp_rms);
    }
}

/// Model options compared on the same set (for choosing defaults).
#[test]
#[ignore]
fn uiuc_option_study() {
    let air = Air::default();
    for (label, opts) in [
        ("default", ModelOptions::default()),
        ("no hub loss", ModelOptions { hub_loss: false, ..Default::default() }),
        ("no rotational augmentation", ModelOptions { rotational_augmentation: false, ..Default::default() }),
        ("no compressibility", ModelOptions { compressibility: false, ..Default::default() }),
        ("80 elements", ModelOptions { elements: 80, ..Default::default() }),
    ] {
        let rs: Vec<_> = cases().iter().map(|c| compare(c, &c.geometry(), &air, &opts)).collect();
        eprintln!("\n== {label}\n{}", summary_table(&rs));
    }
}

/// A full study sweep (12 RPMs x 4 airspeeds, 40 elements) must be quick.
#[test]
fn sweep_is_fast() {
    let case = cases().into_iter().find(|c| c.stem == "gwsdd_3x2").unwrap();
    let g = case.geometry();
    let rpms = rpm_steps(5000.0, 45000.0, 12);
    let t = std::time::Instant::now();
    let pts = sweep(&g, &rpms, &[0.0, 5.0, 10.0, 20.0], &Air::default(), &ModelOptions::default());
    let dt = t.elapsed().as_secs_f64();
    eprintln!("sweep of {} points: {:.2} ms", pts.len(), dt * 1e3);
    assert!(pts.iter().all(|p| p.unconverged == 0));
    assert!(dt < 0.25, "{dt} s");
}
