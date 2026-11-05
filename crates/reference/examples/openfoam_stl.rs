//! Runs an OpenFOAM reference wind tunnel on an STL and prints a summary.
//!
//! ```text
//! cargo run --release -p simlab-reference --example openfoam_stl -- \
//!     <part.stl in mm | sphere:<diameter_mm>> <config.json> [case_dir] [threads]
//! ```
//!
//! `config.json` is a WindConfig, e.g.
//! `{"wind_dir":[1,0,0],"speed_m_s":10,"quality":{"type":"normal"}}` or, for a
//! hovering propeller,
//! `{"wind_dir":[0,0,-1],"speed_m_s":0,"spin":{"rpm":17000,"axis":[0,0,1],"center_mm":[0,0,0]},"quality":{"type":"normal"}}`.
//! Needs Docker (Colima) running and the simlab-reference:1 image built.
//! `--full` runs every iteration (no early stop) and averages the last 300,
//! the protocol of the user's 2026 propeller study.
//! `--dry-run` writes the case and prints its plan without running it.
//! Create a file named STOP in the case directory to cancel the run.
//! The full result (with surface pressure and slices) is written to
//! <case_dir>/result.json.

use simlab_core::{Progress, WindConfig};
use simlab_reference::{geom::icosphere, read_stl_mm};
use std::path::PathBuf;
use std::sync::Mutex;

struct Print {
    stop_file: PathBuf,
    last: Mutex<(f32, String)>,
}
impl Progress for Print {
    fn report(&self, f: f32, m: &str) {
        let mut last = self.last.lock().unwrap();
        let head = m.split(':').next().unwrap_or("").to_string();
        if f - last.0 >= 0.01 || head != last.1 || f >= 1.0 {
            eprintln!("[{:5.1}%] {m}", f * 100.0);
            *last = (f, head);
        }
    }
    fn cancelled(&self) -> bool {
        self.stop_file.exists()
    }
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let dry = args.iter().any(|a| a == "--dry-run");
    // --full: no early stop, average the last 300 iterations (the propeller study's protocol)
    let full = args.iter().any(|a| a == "--full");
    args.retain(|a| a != "--dry-run" && a != "--full");
    if args.len() < 3 {
        eprintln!("usage: openfoam_stl <part.stl (mm) | sphere:<d_mm>> <config.json> [case_dir] [threads]");
        std::process::exit(2);
    }
    let surface = if let Some(d) = args[1].strip_prefix("sphere:") {
        let d: f64 = d.parse()?;
        icosphere([0.0; 3], d * 0.0005, 5)
    } else {
        read_stl_mm(args[1].as_ref())?
    };
    let cfg: WindConfig = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let case_dir = PathBuf::from(args.get(3).cloned().unwrap_or_else(|| "openfoam-case".into()));
    let threads: usize = args.get(4).map(|t| t.parse()).transpose()?.unwrap_or(8);
    std::fs::create_dir_all(&case_dir)?;
    let stop_file = case_dir.join("STOP");
    let _ = std::fs::remove_file(&stop_file);
    let progress = Print { stop_file, last: Mutex::new((-1.0, String::new())) };
    eprintln!("part: {} triangles, {} vertices", surface.triangles.len(), surface.positions.len());
    if dry {
        let prep = simlab_reference::prepare_case(&surface, &cfg, 0.001, &case_dir, threads)?;
        println!("{}", std::fs::read_to_string(case_dir.join("case.json"))?);
        println!("estimated cells {}", prep.estimated_cells);
        return Ok(());
    }
    let mut opts = simlab_reference::RunOptions::default();
    if full {
        opts.early_stop = false;
        opts.average_last = 300;
    }
    let r = simlab_reference::run_openfoam_with(&opts, &surface, &cfg, 0.001, &case_dir, threads, &progress)?;
    std::fs::write(case_dir.join("result.json"), serde_json::to_vec(&r)?)?;

    println!("cells          {}", r.cells);
    println!("iterations     {} (averaged over the last {}), converged: {}", r.iterations, r.averaged_over, r.converged);
    println!("run time       {:.0} s", r.seconds);
    println!("turbulence     {}   Reynolds {:.0}", r.turbulence, r.reynolds);
    println!("force (N)      [{:.6}, {:.6}, {:.6}]", r.force_n[0], r.force_n[1], r.force_n[2]);
    println!("moment (N·m)   [{:.6e}, {:.6e}, {:.6e}] about {:?}", r.moment_nm[0], r.moment_nm[1], r.moment_nm[2], r.moment_center_m);
    println!("drag/lift/side {:.6} / {:.6} / {:.6} N", r.drag_n, r.lift_n, r.side_n);
    if let Some(cd) = r.cd {
        println!("Cd             {:.4}  (frontal area {:.3} mm²)", cd, r.ref_area_m2 * 1e6);
    }
    if let (Some(t), Some(q), Some(p)) = (r.thrust_n, r.torque_nm, r.power_w) {
        println!("thrust         {:.4} N = {:.1} g", t, t / 9.80665e-3);
        println!("torque         {:.3} mN·m   power {:.2} W", q * 1e3, p);
    }
    if let Some(y) = r.y_plus {
        println!("y+ on part     min {:.2}  avg {:.2}  max {:.1}", y[0], y[1], y[2]);
    }
    let sp: Vec<f32> = r.surface_pressure.iter().copied().filter(|x| x.is_finite()).collect();
    println!(
        "surface p      {} of {} vertices, {:.1} .. {:.1} Pa",
        sp.len(),
        r.surface_pressure.len(),
        sp.iter().copied().fold(f32::INFINITY, f32::min),
        sp.iter().copied().fold(f32::NEG_INFINITY, f32::max)
    );
    for s in &r.slices {
        let nan = s.fields[0].values.iter().filter(|x| x.is_nan()).count();
        println!("slice {:6}   {}×{}, {} points inside the part, speed max {:.2} m/s", s.name, s.nu, s.nv, nan, s.fields[0].max);
    }
    for w in &r.warnings {
        println!("warning: {w}");
    }
    println!("case: {}  log: {}", r.case_dir.display(), r.log_path.display());
    Ok(())
}
