//! Runs the wind tunnel on an STL from the command line and prints the summary.
//!
//!   cargo run --release -p simlab-cfd --example run_stl -- part.stl config.json [options]
//!
//! `config.json` is a `WindConfig` (a file path or the JSON itself), e.g.
//!   {"wind_dir":[1,0,0],"speed_m_s":15,"quality":{"type":"normal"}}
//!   {"wind_dir":[0,0,1],"speed_m_s":0,"spin":{"rpm":17000,"axis":[0,0,1],"center_mm":[8.58,-0.22,0]},"quality":{"type":"custom","cells_across":128}}
//! Options:
//!   --unit-mm <x>     millimetres per STL unit (default 1)
//!   --fp16 | --fp32   population storage (default: FP32 unless --budget-gb forces FP16)
//!   --budget-gb <x>   memory budget for the estimate and storage choice
//!   --estimate        only print the estimate
//!   --out <dir>       write surface pressure, slices and history there
//!   --history         print the force history
//!   --axial <d> --radial <d>   still-air margins in rotor diameters (default 2.5, 1)
//!   --tip <u>         lattice speed of the blade tip (default 0.1)
//!   --smag <c>        Smagorinsky constant (default 0.1)
//!   --wind-margins <up,down,side>  wind domain margins in frontal sizes (default 2.5,5,1.5)
//!   --no-refine       one uniform D3Q19 grid instead of the refined D3Q27 solver
//!   --velocity-inlet  fix the inlet velocity instead of its total pressure
//!   --cells <n>       cells across the part's largest size (overrides the quality's)

use simlab_cfd::{SolverOptions, Storage, estimate_with, run_with};
use simlab_core::{Progress, WindConfig};
use std::io::Write;

struct P;
impl Progress for P {
    fn report(&self, f: f32, m: &str) {
        eprintln!("[{:5.1}%] {m}", f * 100.0);
    }
    fn metric(&self, _name: &str, _step: u64, _value: f64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        anyhow::bail!("usage: run_stl <part.stl> <config.json | json> [--unit-mm x] [--fp16] [--budget-gb x] [--estimate] [--out dir]");
    }
    let stl = std::fs::read(&args[0])?;
    let cfg_text = if std::path::Path::new(&args[1]).exists() { std::fs::read_to_string(&args[1])? } else { args[1].clone() };
    let cfg: WindConfig = serde_json::from_str(&cfg_text)?;
    let mut unit_mm = 1.0;
    let mut opt = SolverOptions::default();
    let mut budget: Option<u64> = None;
    let mut only_estimate = false;
    let mut out: Option<std::path::PathBuf> = None;
    let mut history = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--unit-mm" => {
                unit_mm = args[i + 1].parse()?;
                i += 1;
            }
            "--fp16" => opt.storage = Some(Storage::Fp16),
            "--fp32" => opt.storage = Some(Storage::Fp32),
            "--budget-gb" => {
                budget = Some((args[i + 1].parse::<f64>()? * 1e9) as u64);
                i += 1;
            }
            "--estimate" => only_estimate = true,
            // still-air domain margins (rotor diameters) and lattice tip speed
            "--axial" => {
                let v: f64 = args[i + 1].parse()?;
                opt.still_margins.upstream = v;
                opt.still_margins.downstream = v;
                i += 1;
            }
            "--radial" => {
                opt.still_margins.side = args[i + 1].parse()?;
                i += 1;
            }
            "--tip" => {
                opt.max_tip_speed = args[i + 1].parse()?;
                i += 1;
            }
            // wind margins in frontal sizes (default 2.5 upstream, 5 downstream, 1.5 each side)
            "--wind-margins" => {
                let v: Vec<f64> = args[i + 1].split(',').map(|x| x.parse()).collect::<Result<_, _>>()?;
                opt.wind_margins.upstream = v[0];
                opt.wind_margins.downstream = v[1];
                opt.wind_margins.side = v[2];
                i += 1;
            }
            "--velocity-inlet" => opt.total_pressure_inlet = false,
            "--no-refine" => opt.refine = false,
            "--cells" => {
                opt.cells_across = Some(args[i + 1].parse()?);
                i += 1;
            }
            "--smag" => {
                opt.smagorinsky = args[i + 1].parse()?;
                i += 1;
            }
            "--history" => history = true,
            "--out" => {
                out = Some(args[i + 1].clone().into());
                i += 1;
            }
            a => anyhow::bail!("unknown option {a}"),
        }
        i += 1;
    }
    opt.memory_budget = budget;
    let model = simlab_core::stl::read_stl(&stl)?;
    let unit_m = unit_mm * 1e-3;
    let surface = model.scaled(unit_m);
    let est = estimate_with(&cfg, &surface, unit_m, budget.unwrap_or(u64::MAX / 4), &opt)?;
    eprintln!("estimate: {}", serde_json::to_string(&est)?);
    if only_estimate {
        println!("{}", serde_json::to_string_pretty(&est)?);
        return Ok(());
    }
    let r = run_with(&cfg, &surface, unit_m, &opt, &P)?;
    println!("{}", serde_json::to_string_pretty(&r.summary)?);
    if history {
        for h in &r.history {
            println!("t={:.3} F=[{:.5}, {:.5}, {:.5}] M=[{:.6}, {:.6}, {:.6}]", h.0, h.1[0], h.1[1], h.1[2], h.2[0], h.2[1], h.2[2]);
        }
    }
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)?;
        simlab_core::write_le(&dir.join("pressure_pa.f32"), r.surface_pressure_pa.iter().copied())?;
        simlab_core::write_le(&dir.join("cp.f32"), r.surface_cp.iter().copied())?;
        for s in &r.slices {
            std::fs::write(dir.join(format!("slice.{}.json", s.name)), serde_json::to_vec_pretty(s)?)?;
            for f in &s.fields {
                simlab_core::write_le(&dir.join(format!("slice.{}.{}.f32", s.name, f.name)), f.data.iter().copied())?;
            }
        }
        let mut h = std::fs::File::create(dir.join("history.csv"))?;
        writeln!(h, "t,fx,fy,fz,mx,my,mz")?;
        for x in &r.history {
            writeln!(h, "{},{},{},{},{},{},{}", x.0, x.1[0], x.1[1], x.1[2], x.2[0], x.2[1], x.2[2])?;
        }
    }
    Ok(())
}
