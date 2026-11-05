//! Runs OpenFOAM for real. Needs Docker (Colima) running and the
//! simlab-reference:1 image built; run with
//! `cargo test -p simlab-reference --release -- --ignored --test-threads 1`.
//! Case folders go to <repo>/target/reference-tests (Colima shares only the
//! home folder, so they must not be in /tmp).

use simlab_core::{Air, Cancelled, GridQuality, Progress, WindConfig};
use simlab_reference::{geom::icosphere, run_openfoam};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/reference-tests").join(name)
}

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8)
}

/// Schiller–Naumann: Cd = 24/Re (1 + 0.15 Re^0.687), within ~5 % of data up to Re 800.
fn schiller_naumann(re: f64) -> f64 {
    24.0 / re * (1.0 + 0.15 * re.powf(0.687))
}

struct Quiet;
impl Progress for Quiet {
    fn report(&self, f: f32, m: &str) {
        if !m.starts_with("Solving") {
            eprintln!("[{:5.1}%] {m}", f * 100.0);
        }
    }
    fn cancelled(&self) -> bool {
        false
    }
}

#[test]
#[ignore]
fn sphere_re100_drag() {
    let d = 0.02;
    let air = Air::default();
    let u = 100.0 * air.kinematic_viscosity / d;
    let cfg = WindConfig { wind_dir: [1.0, 0.0, 0.0], speed_m_s: u, air, spin: None, quality: GridQuality::Draft, duration: None };
    let s = icosphere([0.0; 3], d / 2.0, 5);
    let r = run_openfoam(&s, &cfg, 0.001, &case("sphere-re100"), threads(), &Quiet).unwrap();
    let cd = r.cd.unwrap();
    let want = schiller_naumann(100.0);
    eprintln!("Cd {cd:.4} vs {want:.4}, {} cells, {:.0} s", r.cells, r.seconds);
    assert!(r.converged);
    assert!((cd - want).abs() / want < 0.05, "Cd {cd} vs {want}");
    assert!(r.surface_pressure.iter().all(|p| p.is_finite()));
    // stagnation pressure ≈ ½ρU² (a little more at this low Reynolds number)
    let q = 0.5 * air.density * u * u;
    let pmax = r.surface_pressure.iter().copied().fold(f32::MIN, f32::max) as f64;
    assert!(pmax / q > 0.95 && pmax / q < 1.25, "stagnation Cp {}", pmax / q);
    assert_eq!(r.slices.len(), 2);
    for sl in &r.slices {
        assert_eq!(sl.fields[0].values.len(), sl.nu * sl.nv);
        assert!(sl.fields[0].values.iter().any(|v| v.is_nan()), "the sphere shows as NaN");
    }
}

/// Stopping a run kills the container and returns Cancelled.
#[test]
#[ignore]
fn cancel_kills_the_container() {
    struct StopWhenSolving(AtomicBool);
    impl Progress for StopWhenSolving {
        fn report(&self, _f: f32, m: &str) {
            if m.starts_with("Solving") {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        fn cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let cfg = WindConfig {
        wind_dir: [1.0, 0.0, 0.0],
        speed_m_s: 5.0,
        air: Air::default(),
        spin: None,
        quality: GridQuality::Draft,
        duration: None,
    };
    let s = icosphere([0.0; 3], 0.01, 4);
    let p = StopWhenSolving(AtomicBool::new(false));
    let t = std::time::Instant::now();
    let e = run_openfoam(&s, &cfg, 0.001, &case("cancel"), threads(), &p).unwrap_err();
    assert!(e.downcast_ref::<Cancelled>().is_some(), "{e}");
    let docker = simlab_reference::find_docker().unwrap();
    let out = std::process::Command::new(docker)
        .envs(simlab_reference::docker_env())
        .args(["ps", "-q", "--filter", "name=simlab-ref-"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty(), "a container is still running");
    eprintln!("cancelled after {:.0} s", t.elapsed().as_secs_f64());
}
