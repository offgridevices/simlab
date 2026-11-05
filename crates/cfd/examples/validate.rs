//! Validation runs from the command line, e.g.
//!   cargo run --release -p simlab-cfd --example validate -- sphere 100 32 20
//!   cargo run --release -p simlab-cfd --example validate -- rot 1 24 4
//!   cargo run --release -p simlab-cfd --example validate -- prop 20000 64 8
//! Options after the numbers: fp16, side=<margin>, up=<..>, down=<..>, smag=<..>, u=<lattice speed>, tol=<..>, velsides

use simlab_cfd::lattice::Storage;
use simlab_cfd::shapes::PropSpec;
use simlab_cfd::tunnel::{FaceBcChoice, SolverOptions};
use simlab_cfd::validation::*;
use simlab_core::Progress;

struct P;
impl Progress for P {
    fn report(&self, f: f32, m: &str) {
        if std::env::var_os("QUIET").is_none() {
            eprintln!("[{:5.1}%] {m}", f * 100.0);
        }
    }
    fn cancelled(&self) -> bool {
        false
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize| -> f64 { a[i].parse().unwrap() };
    let mut o = SolverOptions::default();
    for s in &a {
        if s == "fp16" {
            o.storage = Some(Storage::Fp16);
        }
        if s == "fp32" {
            o.storage = Some(Storage::Fp32);
        }
        if s == "p0" {
            o.total_pressure_inlet = true;
        }
        if s == "absu" {
            o.absorb_velocity = true;
        }
        if s == "velsides" {
            o.side_bc = FaceBcChoice::Velocity;
        }
        if let Some((k, v)) = s.split_once('=') {
            let v: f64 = v.parse()?;
            match k {
                "side" => o.wind_margins.side = v,
                "up" => o.wind_margins.upstream = v,
                "down" => o.wind_margins.downstream = v,
                "smag" => o.smagorinsky = v,
                "u" => {
                    o.max_lattice_speed = v;
                    o.max_tip_speed = v;
                }
                "tol" => o.tolerance = v,
                "tau" => o.max_tau = v,
                "radial" => o.still_margins.side = v,
                "axial" => {
                    o.still_margins.upstream = v;
                    o.still_margins.downstream = v;
                }
                "voff" => o.vertex_offset = v,
                "sigma" => o.sponge_sigma = v,
                "spin" => o.sponge_widths[0] = v,
                "spout" => o.sponge_widths[1] = v,
                "spside" => o.sponge_widths[2] = v,
                _ => {}
            }
        }
    }
    match a[0].as_str() {
        "sphere" => {
            let (re, d, dur) = (num(1), num(2) as u32, num(3));
            let (cd, r) = sphere_drag(re, d, dur, &o, &P)?;
            let s = &r.summary;
            let refc = sphere_cd_cgw(re);
            println!(
                "SPHERE Re={re} D={d} cells: Cd={cd:.4} ref(CGW)={refc:.4} SN={:.4} err={:+.2}% lift={:.2e} conv={} passes={:.1} avg={:.1} grid={:?} blockage={:.2}% tau={:.4} MLUPs={:.0} {:?} t={:.0}s",
                sphere_cd_sn(re),
                (cd / refc - 1.0) * 100.0,
                s.lift_n / s.drag_n,
                s.converged,
                s.simulated,
                s.averaged_over,
                s.grid,
                s.blockage * 100.0,
                s.tau,
                s.mlups,
                s.storage,
                s.seconds
            );
            for w in &s.warnings {
                println!("  warning: {w}");
            }
            if std::env::var_os("HIST").is_some() {
                let a = std::f64::consts::PI * SPHERE_D * SPHERE_D / 4.0;
                let q = 0.5 * 1.2 * SPHERE_U * SPHERE_U * a;
                for h in r.history.iter().step_by((r.history.len() / 40).max(1)) {
                    println!("  t={:.2} Cd={:.4} Cl={:.4} Cs={:.4}", h.0, h.1[0] / q, h.1[2] / q, h.1[1] / q);
                }
            }
        }
        "rot" => {
            let (re, d, revs) = (num(1), num(2) as u32, num(3));
            let (t, stokes, r) = rotating_sphere(re, d, revs, &o, &P)?;
            let s = &r.summary;
            println!(
                "ROTSPHERE Re={re} D={d}: torque={:.4e} stokes={:.4e} ratio={:.4} err={:+.2}% thrust={:.2e} conv={} grid={:?} tau={:.3} MLUPs={:.0}",
                t,
                stokes,
                -t / stokes,
                (-t / stokes - 1.0) * 100.0,
                s.thrust_n.unwrap_or(0.0),
                s.converged,
                s.grid,
                s.tau,
                s.mlups
            );
        }
        "prop" => {
            let (rpm, cells, revs) = (num(1), num(2) as u32, num(3));
            let r = prop_hover(&PropSpec::five_inch(), rpm, cells, revs, &o, &P)?;
            let s = &r.summary;
            println!(
                "PROP rpm={rpm} cells={cells}: thrust={:.4} N torque={:.4e} N·m power={:.2} W conv={} revs={:.1} avg={:.1} grid={:?} dx={:.3}mm tau={:.5} Ma={:.3} MLUPs={:.0} mem={:.2}GB t={:.0}s",
                s.thrust_n.unwrap(),
                s.torque_nm.unwrap(),
                s.power_w.unwrap(),
                s.converged,
                s.simulated,
                s.averaged_over,
                s.grid,
                s.dx_mm,
                s.tau,
                s.mach_lattice,
                s.mlups,
                s.memory_bytes as f64 / 1e9,
                s.seconds
            );
            for w in &s.warnings {
                println!("  warning: {w}");
            }
            for h in r.history.iter().step_by((r.history.len() / 20).max(1)) {
                println!("  t={:.2} Fz={:.4} Mz={:.4e}", h.0, h.1[2], h.2[2]);
            }
        }
        _ => anyhow::bail!("unknown case"),
    }
    Ok(())
}
