//! Geometry extraction round trip: loft a 3-D propeller from a known
//! geometry (UIUC DA4002 5x3.75 with its stated SDA1075 airfoil, and a
//! 3-blade cambered-plate prop), read the geometry back from the mesh, and
//! compare. Then run BEMT on the extracted geometry against the measured
//! UIUC data.

mod common;

use common::*;
use simlab_core::Air;
use simlab_prop::loft::{Airfoil, LoftOptions, loft};
use simlab_prop::*;

const SDA1075_DAT: &str = include_str!("data/sda1075.dat");

struct Errors {
    chord_max: f64,
    chord_rms: f64,
    beta_max: f64,
    beta_rms: f64,
}

fn compare_geometry(input: &BladeGeometry, got: &BladeGeometry, from: f64, to: f64) -> Errors {
    let (mut cm, mut cs, mut bm, mut bs, mut n) = (0.0f64, 0.0, 0.0f64, 0.0, 0.0);
    eprintln!("    r/R    chord in   chord out  err  |  beta in  beta out  err  |  t/c   camber");
    for s in &got.stations {
        let x = s.r / input.radius;
        if x < from || x > to {
            continue;
        }
        let want = input.at(s.r);
        let ec = (s.chord - want.chord) / want.chord;
        let eb = (s.beta - want.beta) / want.beta;
        eprintln!(
            "    {:.3}  {:7.3} mm {:7.3} mm {:+5.2}% | {:6.2}  {:6.2}  {:+5.2}% | {:.3} {:+.3}",
            x,
            want.chord * 1e3,
            s.chord * 1e3,
            100.0 * ec,
            want.beta.to_degrees(),
            s.beta.to_degrees(),
            100.0 * eb,
            s.thickness,
            s.camber
        );
        cm = cm.max(ec.abs());
        bm = bm.max(eb.abs());
        cs += ec * ec;
        bs += eb * eb;
        n += 1.0;
    }
    Errors { chord_max: cm, chord_rms: (cs / n).sqrt(), beta_max: bm, beta_rms: (bs / n).sqrt() }
}

#[test]
fn da4002_loft_extract_and_bemt() {
    let case = cases().into_iter().find(|c| c.stem == "da4002_5x3.75").unwrap();
    let input = case.geometry();
    let frame = Frame::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let mut input_ccw = input.clone();
    input_ccw.rotation = Rotation::Ccw;
    let hub_r = input.stations[0].r; // 0.3 R
    let mesh_m = loft(
        &input_ccw,
        &Airfoil::parse_dat(SDA1075_DAT),
        &frame,
        &LoftOptions { hub: Some((hub_r, 0.004, 0.0015)), root_overlap: 0.5e-3, rings: 90, section_points: 120, ..Default::default() },
    );
    // the engine hands meshes over in model units: use millimetres
    let mesh_mm = mesh_m.scaled(1e3);
    let t0 = std::time::Instant::now();
    let ex = extract_geometry(&mesh_mm, 1e-3, [0.0, 0.0, 1.0], [0.0, 0.0, 0.0], &ExtractOptions::default()).unwrap();
    eprintln!("extraction: {:.1} ms, {} triangles", t0.elapsed().as_secs_f64() * 1e3, mesh_mm.triangles.len());
    let g = &ex.geometry;
    eprintln!(
        "blades {} radius {:.3} mm (in {:.3}) hub join {:.2} mm, rotation {:?}, warnings {:?}",
        g.blades,
        g.radius * 1e3,
        input.radius * 1e3,
        ex.hub_join_radius * 1e3,
        g.rotation,
        ex.warnings
    );
    assert_eq!(g.blades, 2);
    assert_eq!(g.rotation, Rotation::Ccw);
    assert!((g.radius - input.radius).abs() < 0.01 * input.radius);
    let e = compare_geometry(&input, g, 0.35, 0.95);
    eprintln!(
        "DA4002 round trip, r/R 0.35-0.95: chord max {:.2}% rms {:.2}%, pitch max {:.2}% rms {:.2}%",
        100.0 * e.chord_max,
        100.0 * e.chord_rms,
        100.0 * e.beta_max,
        100.0 * e.beta_rms
    );
    assert!(e.chord_max < 0.03 && e.beta_max < 0.03);
    // thickness / camber of the SDA1075
    let mid = g.at(0.7 * g.radius);
    assert!((mid.thickness - 0.12).abs() < 0.01, "t/c {}", mid.thickness);
    assert!(mid.camber > 0.01 && mid.camber < 0.035, "camber {}", mid.camber);

    // BEMT from the extracted geometry against the measured data
    let air = Air::default();
    let opts = ModelOptions::default();
    let from_table = compare(&case, &input, &air, &opts);
    let from_mesh = compare(&case, g, &air, &opts);
    eprintln!("{}", from_mesh.table());
    eprintln!("\nfrom the table:\n{}\nfrom the extracted mesh:\n{}", summary_table(&[from_table]), summary_table(&[from_mesh]));
}

#[test]
fn three_blade_cambered_plate_cw() {
    // a 3-inch 3-blade FPV-like prop: tapered, 1.6 in pitch, cambered plate
    let r = 0.03825;
    let xs: Vec<f64> = (0..=16).map(|i| 0.2 + 0.05 * i as f64).collect();
    let c: Vec<f64> = xs.iter().map(|x| 0.16 + 0.12 * (1.0 - x) - 0.1 * (x - 0.9).max(0.0) * 10.0 * 0.5).collect();
    let beta: Vec<f64> = xs.iter().map(|x| (1.6 * 0.0254 / (std::f64::consts::TAU * x * r)).atan().to_degrees()).collect();
    let mut input = BladeGeometry::from_table(3, r, &xs, &c, &beta, |x| SectionShape { thickness: 0.09 - 0.03 * x, camber: 0.04 });
    input.rotation = Rotation::Cw;
    let axis = simlab_core::normalize([0.3, -0.2, 1.0]);
    let center = [0.01, -0.02, 0.005];
    let frame = Frame::new(center, axis);
    let mesh = loft(&input, &Airfoil::Cambered, &frame, &LoftOptions { hub: Some((0.2 * r, 0.0027, 0.00075)), root_overlap: 0.3e-3, ..Default::default() });
    let ex = extract_geometry(&mesh, 1.0, axis, center, &ExtractOptions::default()).unwrap();
    let g = &ex.geometry;
    assert_eq!(g.blades, 3);
    assert_eq!(g.rotation, Rotation::Cw);
    let e = compare_geometry(&input, g, 0.3, 0.95);
    eprintln!(
        "3-blade cambered plate round trip: chord max {:.2}% rms {:.2}%, pitch max {:.2}% rms {:.2}%; bore {:?} mm",
        100.0 * e.chord_max,
        100.0 * e.chord_rms,
        100.0 * e.beta_max,
        100.0 * e.beta_rms,
        ex.bore_radius.map(|b| b * 1e3)
    );
    assert!(e.chord_max < 0.03 && e.beta_max < 0.03);
    let mid = g.at(0.6 * r);
    assert!((mid.camber - 0.04).abs() < 0.008, "camber {}", mid.camber);
    assert!((mid.thickness - (0.09 - 0.03 * 0.6)).abs() < 0.01, "t/c {}", mid.thickness);

    // auto-detect finds the same axis, centre, blades and spin
    let guess = detect_prop(&mesh).expect("looks like a prop");
    eprintln!("detect: {guess:?}");
    assert_eq!(guess.blades, 3);
    assert_eq!(guess.rotation, Rotation::Cw);
    assert!(simlab_core::dot(guess.axis, axis) > 0.999, "{:?}", guess.axis);
    let off = simlab_core::sub(guess.center, center);
    let radial = simlab_core::sub(off, simlab_core::scale(axis, simlab_core::dot(off, axis)));
    assert!(simlab_core::norm(radial) < 0.01 * r, "centre off by {:?}", radial);
    // flipping the model over flips the suggested thrust direction too
    let flipped = TriMesh_flip(&mesh);
    let g2 = detect_prop(&flipped).unwrap();
    assert!(simlab_core::dot(g2.axis, [-axis[0], axis[1], -axis[2]]) > 0.999, "{:?}", g2.axis);
}

/// the same mesh turned upside down (180 deg about y)
#[allow(non_snake_case)]
fn TriMesh_flip(m: &simlab_core::TriMesh) -> simlab_core::TriMesh {
    simlab_core::TriMesh {
        positions: m.positions.iter().map(|p| [-p[0], p[1], -p[2]]).collect(),
        triangles: m.triangles.clone(),
        patch: m.patch.clone(),
    }
}

#[test]
fn tractions_reproduce_band_loads() {
    let case = cases().into_iter().find(|c| c.stem == "da4002_5x3.75").unwrap();
    let mut g = case.geometry();
    g.rotation = Rotation::Cw;
    let frame = Frame::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let mesh = loft(&g, &Airfoil::Cambered, &frame, &LoftOptions { hub: Some((g.stations[0].r, 0.004, 0.0015)), ..Default::default() });
    let p = solve(&g, 8000.0, 0.0, &Air::default(), &ModelOptions::default());
    let faces: Vec<BoundaryFace> = (0..mesh.triangles.len())
        .map(|t| {
            let [a, b, c] = mesh.triangles[t];
            let (a, b, c) = (mesh.positions[a as usize], mesh.positions[b as usize], mesh.positions[c as usize]);
            BoundaryFace {
                centroid: [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0],
                normal: mesh.triangle_normal(t),
                area: mesh.triangle_area(t),
            }
        })
        .collect();
    let tr = face_tractions(&p, &frame, g.rotation, &faces);
    let (mut fz, mut mz, mut fxy) = (0.0, 0.0, [0.0; 2]);
    for (f, t) in faces.iter().zip(&tr) {
        let force = simlab_core::scale(*t, f.area);
        fz += force[2];
        // moment about +z: x Fy - y Fx
        mz += f.centroid[0] * force[1] - f.centroid[1] * force[0];
        fxy[0] += force[0];
        fxy[1] += force[1];
    }
    eprintln!("thrust {:.4} N (BEMT {:.4}), torque {:.6} N m (BEMT {:.6}), net side force {:?}", fz, p.thrust_n, mz, p.torque_nm, fxy);
    assert!((fz - p.thrust_n).abs() < 1e-9 * p.thrust_n.abs().max(1.0));
    // air drag torque on a CW prop (spinning -z) points +z
    assert!((mz - p.torque_nm).abs() < 1e-6 * p.torque_nm, "{mz} vs {}", p.torque_nm);
    assert!(fxy[0].hypot(fxy[1]) < 1e-3 * p.thrust_n);
    // per band
    for e in &p.elements {
        let mut band = 0.0;
        for (f, t) in faces.iter().zip(&tr) {
            let r = f.centroid[0].hypot(f.centroid[1]);
            if r >= e.r_lo && r < e.r_hi {
                band += t[2] * f.area;
            }
        }
        let want = e.dt_dr * (e.r_hi - e.r_lo);
        if band != 0.0 {
            assert!((band - want).abs() < 1e-9 + 1e-6 * want.abs(), "band at {} : {band} vs {want}", e.r);
        }
    }
}

#[test]
fn a_box_is_not_a_prop() {
    // 20 x 20 x 20 cube from 12 triangles, subdivided enough to pass the size check
    let mut m = simlab_core::TriMesh::default();
    let c = [[0.0, 0.0, 0.0], [20.0, 0.0, 0.0], [20.0, 20.0, 0.0], [0.0, 20.0, 0.0], [0.0, 0.0, 20.0], [20.0, 0.0, 20.0], [20.0, 20.0, 20.0], [0.0, 20.0, 20.0]];
    m.positions = c.to_vec();
    let f = [[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]];
    for _ in 0..2 {
        m.triangles.extend_from_slice(&f);
    }
    m.patch = vec![0; m.triangles.len()];
    assert!(detect_prop(&m).is_none());
}

#[test]
fn whole_study_from_a_mesh() {
    // the 3-blade test prop, in millimetres, through the engine entry point
    let r = 0.03825;
    let xs: Vec<f64> = (0..=16).map(|i| 0.2 + 0.05 * i as f64).collect();
    let c: Vec<f64> = xs.iter().map(|x| 0.16 + 0.12 * (1.0 - x)).collect();
    let beta: Vec<f64> = xs.iter().map(|x| (1.6 * 0.0254 / (std::f64::consts::TAU * x * r)).atan().to_degrees()).collect();
    let g = BladeGeometry::from_table(3, r, &xs, &c, &beta, |_| SectionShape { thickness: 0.08, camber: 0.04 });
    let frame = Frame::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let mesh = loft(&g, &Airfoil::Cambered, &frame, &LoftOptions { hub: Some((0.2 * r, 0.0027, 0.00075)), root_overlap: 0.3e-3, ..Default::default() }).scaled(1e3);
    let cfg = simlab_core::PropConfig {
        axis: [0.0, 0.0, 1.0],
        center_mm: [0.0, 0.0, 0.0],
        rpm_min: 10000.0,
        rpm_max: 40000.0,
        rpm_steps: 7,
        airspeeds_m_s: vec![0.0, 10.0],
        air: Air::default(),
        blades: None,
    };
    let t = std::time::Instant::now();
    let run = simlab_prop::run(&mesh, 1e-3, &cfg, &simlab_core::Silent).unwrap();
    eprintln!("study: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    assert_eq!(run.points.len(), 14);
    let s = run.summary();
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"kind\":\"prop\"") && json.contains("\"rotation\":\"ccw\""));
    eprintln!("{}", s.headline);
    let hover40 = &run.points[6];
    assert!(hover40.thrust_g > 100.0 && hover40.thrust_g < 600.0, "{}", hover40.thrust_g);
    assert!(run.points[13].thrust_g < hover40.thrust_g);
}
