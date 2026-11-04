//! Reads a propeller STL, finds its axis (or takes one), prints the blade
//! geometry and an RPM sweep.
//!
//!   cargo run --release -p simlab-prop --example prop_stl -- prop.stl
//!   ... -- prop.stl --axis 0,0,1 --center 0,0,0 --rpm 10000:45000:8 --speed 0,10 --unit-mm 1
//!   ... -- prop.stl --at 45800 (adds the spanwise loads at that rpm)

use simlab_core::{Air, Vec3};
use simlab_prop::*;
use std::time::Instant;

fn vec3(s: &str) -> Vec3 {
    let v: Vec<f64> = s.split(',').map(|x| x.trim().parse().expect("number")).collect();
    assert!(v.len() == 3, "need x,y,z");
    [v[0], v[1], v[2]]
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: prop_stl <file.stl> [--unit-mm 1] [--axis x,y,z] [--center x,y,z] [--blades n] [--rpm min:max:steps] [--speed v1,v2] [--at rpm]");
        std::process::exit(2);
    }
    let path = &args[0];
    let mut unit_mm = 1.0;
    let mut axis = None;
    let mut center = None;
    let mut blades = None;
    let mut rpm = (5000.0, 45000.0, 9u32);
    let mut speeds = vec![0.0];
    let mut at = None;
    let mut i = 1;
    while i < args.len() {
        let v = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--unit-mm" => unit_mm = v.parse()?,
            "--axis" => axis = Some(vec3(&v)),
            "--center" => center = Some(vec3(&v)),
            "--blades" => blades = Some(v.parse()?),
            "--rpm" => {
                let p: Vec<f64> = v.split(':').map(|x| x.parse().unwrap()).collect();
                rpm = (p[0], p[1], p.get(2).copied().unwrap_or(9.0) as u32);
            }
            "--speed" => speeds = v.split(',').map(|x| x.parse().unwrap()).collect(),
            "--at" => at = Some(v.parse::<f64>()?),
            o => anyhow::bail!("unknown option {o}"),
        }
        i += 2;
    }
    let t0 = Instant::now();
    let mesh = simlab_core::stl::read_stl(&std::fs::read(path)?)?;
    let t_read = t0.elapsed();
    let (axis, center) = match (axis, center) {
        (Some(a), Some(c)) => (a, c),
        _ => {
            let t = Instant::now();
            let g = detect_prop(&mesh).ok_or_else(|| anyhow::anyhow!("this does not look like a propeller; pass --axis and --center"))?;
            println!(
                "auto-detect ({:.0} ms): axis [{:.4}, {:.4}, {:.4}], centre [{:.3}, {:.3}, {:.3}] mm, {} blades, {:.2} mm radius, spins {}, confidence {:.2}",
                t.elapsed().as_secs_f64() * 1e3,
                g.axis[0],
                g.axis[1],
                g.axis[2],
                g.center[0] * unit_mm,
                g.center[1] * unit_mm,
                g.center[2] * unit_mm,
                g.blades,
                g.radius * unit_mm,
                g.rotation.as_str(),
                g.confidence
            );
            (axis.unwrap_or(g.axis), center.unwrap_or(g.center))
        }
    };
    let t1 = Instant::now();
    let ex = extract_geometry(&mesh, unit_mm * 1e-3, axis, center, &ExtractOptions { stations: 25, blades })?;
    let t_ex = t1.elapsed();
    let g = &ex.geometry;
    println!(
        "\n{} ({} triangles): {} blades, diameter {:.2} mm, blades join the hub at r = {:.2} mm, bore {}, spins {} seen from the thrust side",
        path,
        mesh.triangles.len(),
        g.blades,
        g.radius * 2e3,
        ex.hub_join_radius * 1e3,
        ex.bore_radius.map(|b| format!("{:.2} mm dia", b * 2e3)).unwrap_or("none".into()),
        g.rotation.as_str()
    );
    println!("pitch at 75 %: {:.1} mm ({:.2} in), solidity {:.3}", g.pitch_75() * 1e3, g.pitch_75() / 0.0254, g.solidity_75());
    println!("\n  r mm   r/R   chord mm  pitch deg  pitch mm  t/c %  camber %  chord spread  pitch spread");
    for (s, sp) in g.stations.iter().zip(ex.spread.iter().map(Some).chain(std::iter::repeat(None))) {
        println!(
            "{:6.2}  {:.3}  {:8.3}  {:9.2}  {:8.1}  {:5.1}  {:+7.2}   {}",
            s.r * 1e3,
            s.r / g.radius,
            s.chord * 1e3,
            s.beta.to_degrees(),
            s.pitch() * 1e3,
            s.thickness * 100.0,
            s.camber * 100.0,
            sp.map(|p| format!("{:5.1}%        {:.2} deg", p.chord_spread * 100.0, p.beta_spread_deg)).unwrap_or_default()
        );
    }
    for w in &ex.warnings {
        println!("warning: {w}");
    }
    let air = Air::default();
    let opts = ModelOptions::default();
    let rpms = rpm_steps(rpm.0, rpm.1, rpm.2);
    let t2 = Instant::now();
    let pts = sweep(g, &rpms, &speeds, &air, &opts);
    let t_sw = t2.elapsed();
    println!("\n   rpm   V m/s  thrust g  thrust N  torque mN m  power W     CT      CP      J    eff    FM   g/W  tip Mach  Re75");
    for p in &pts {
        println!(
            "{:6.0}  {:6.1}  {:8.1}  {:8.3}  {:11.3}  {:7.2}  {:.4}  {:.4}  {:.3}  {}  {}  {:5.2}  {:.3}  {:6.0}",
            p.rpm,
            p.airspeed_m_s,
            p.thrust_g,
            p.thrust_n,
            p.torque_nm * 1e3,
            p.power_w,
            p.ct,
            p.cp,
            p.advance_ratio,
            p.efficiency.map(|e| format!("{e:.3}")).unwrap_or("  -  ".into()),
            p.figure_of_merit.map(|e| format!("{e:.3}")).unwrap_or("  -  ".into()),
            if p.power_w > 0.0 { p.thrust_g / p.power_w } else { 0.0 },
            p.tip_mach,
            p.reynolds_75
        );
    }
    if let Some(n) = at {
        let p = solve(g, n, speeds[0], &air, &opts);
        println!("\nspanwise at {n:.0} rpm, {:.1} m/s:\n  r mm  dT/dr N/m  dQ/dr N m/m  alpha  phi   cl     cd      Re", speeds[0]);
        for e in &p.elements {
            println!(
                "{:6.2}  {:9.3}  {:10.5}  {:5.1}  {:4.1}  {:.3}  {:.4}  {:6.0}",
                e.r * 1e3,
                e.dt_dr,
                e.dq_dr,
                e.alpha_deg,
                e.phi_deg,
                e.cl,
                e.cd,
                e.reynolds
            );
        }
        println!("total: {:.1} g, {:.3} mN m, {:.1} W", p.thrust_g, p.torque_nm * 1e3, p.power_w);
    }
    println!(
        "\ntimes: read {:.0} ms, extract {:.0} ms, sweep of {} points {:.2} ms",
        t_read.as_secs_f64() * 1e3,
        t_ex.as_secs_f64() * 1e3,
        pts.len(),
        t_sw.as_secs_f64() * 1e3
    );
    Ok(())
}
