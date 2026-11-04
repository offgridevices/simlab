//! Simplifies a dense surface for volume meshing and writes it in the
//! mesher's surface format.
//!
//! ```text
//! cargo run --release -p simlab-fea --example decimate_stl -- <in.stl | surface_dir> <out_dir>
//!     --size <element size, model units> [--error <max RMS error, model units>]
//! ```
//! `surface_dir` is the output of `simlab_mesher.py surface` (keeps its patches).

use anyhow::{Result, bail};
use simlab_core::decimate::{DecimateOptions, decimate_with, open_and_nonmanifold_edges, surface_defects, target_tris_for, write_mesher_surface};
use simlab_core::{TriMesh, read_le};
use std::path::Path;
use std::time::Instant;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        bail!("usage: decimate_stl <in.stl | surface_dir> <out_dir> --size h [--error e]");
    }
    let mut size = 0.6;
    let mut err = None;
    let mut i = 2;
    while i + 1 < a.len() {
        match a[i].as_str() {
            "--size" => size = a[i + 1].parse()?,
            "--error" => err = Some(a[i + 1].parse::<f64>()?),
            o => bail!("unknown option {o}"),
        }
        i += 2;
    }
    let t = Instant::now();
    let input = Path::new(&a[0]);
    let m: TriMesh = if input.is_dir() {
        let p = read_le::<f64>(&input.join("surface_positions.f64"))?;
        let tr = read_le::<u32>(&input.join("surface_triangles.u32"))?;
        let pa = read_le::<u32>(&input.join("surface_patch.u32"))?;
        TriMesh { positions: p.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(), triangles: tr.chunks(3).map(|c| [c[0], c[1], c[2]]).collect(), patch: pa }
    } else {
        simlab_core::stl::read_stl(&std::fs::read(input)?)?
    };
    let area: f64 = (0..m.triangles.len()).map(|t| m.triangle_area(t)).sum();
    let target = target_tris_for(area, size);
    // h/150 keeps root fillets: E-v5 peak within 0.8 % of the 50k-facet mesh (0.02·h lost 2.6 %)
    let max_error = err.unwrap_or(size / 150.0);
    eprintln!(
        "input: {} triangles, {} vertices, area {:.1}, volume {:.1}, open/non-manifold edges {:?} (read {:.1} s)",
        m.triangles.len(),
        m.positions.len(),
        area,
        m.volume(),
        open_and_nonmanifold_edges(&m),
        t.elapsed().as_secs_f64()
    );
    let t = Instant::now();
    let d = decimate_with(&m, &DecimateOptions { target_tris: target, max_error, max_edge: size });
    let a2: f64 = (0..d.triangles.len()).map(|t| d.triangle_area(t)).sum();
    println!(
        "decimated to {} triangles (target {target}) in {:.1} s; area {:.1} ({:+.2} %), volume {:.2} ({:+.2} %), open/non-manifold edges {:?}",
        d.triangles.len(),
        t.elapsed().as_secs_f64(),
        a2,
        100.0 * (a2 / area - 1.0),
        d.volume(),
        100.0 * (d.volume() / m.volume() - 1.0),
        open_and_nonmanifold_edges(&d)
    );
    if std::env::var("CHECK").is_ok() {
        println!("defects (crossing pairs, flat folds): input {:?}, output {:?}", surface_defects(&m), surface_defects(&d));
    }
    write_mesher_surface(Path::new(&a[1]), &d)?;
    Ok(())
}
