//! Importing a model: keep the original, have Gmsh split its surface into
//! patches, measure each patch (flat face, hole, ...), check it is closed,
//! guess what it is (a propeller? a frame with motor mounts?) and write the
//! display buffers the UI draws.

use crate::db::{ModelRow, new_id, now};
use crate::state::App;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use simlab_core::{TriMesh, Vec3, cross, dot, norm, normalize, read_le, sub};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

pub const MAX_UPLOAD: usize = 512 * 1024 * 1024;

pub fn format_of(file_name: &str) -> Option<&'static str> {
    let ext = Path::new(file_name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "stl" => "stl",
        "step" | "stp" => "step",
        "obj" => "obj",
        "3mf" => "3mf",
        "iges" | "igs" => "iges",
        _ => return None,
    })
}

/// Saves an upload and starts processing it. Returns the new model row.
pub fn create(app: &Arc<App>, project_id: &str, file_name: &str, name: Option<String>, unit_mm: f64, bytes: &[u8]) -> Result<ModelRow> {
    let Some(format) = format_of(file_name) else {
        bail!("SimLab reads STL, STEP, IGES, OBJ and 3MF files.");
    };
    if bytes.len() > MAX_UPLOAD {
        bail!("that file is over 512 MB");
    }
    app.db().project(project_id)?;
    let id = new_id();
    let dir = app.model_dir(project_id, &id);
    std::fs::create_dir_all(&dir)?;
    let ext = if format == "iges" { "igs" } else { format };
    std::fs::write(dir.join(format!("source.{ext}")), bytes)?;
    let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("Part");
    let row = ModelRow {
        id,
        project_id: project_id.into(),
        name: name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| stem.replace(['_', '-'], " ")),
        file_name: file_name.into(),
        format: format.into(),
        unit_mm,
        status: "processing".into(),
        message: "Reading the part".into(),
        info: json!({}),
        patch_labels: json!({}),
        created_at: now(),
    };
    app.db().insert_model(&row)?;
    app.db().touch_project(project_id)?;
    let app2 = app.clone();
    let mid = row.id.clone();
    tokio::task::spawn_blocking(move || {
        let result = process(&app2, &mid);
        let mut m = match app2.db().model(&mid) {
            Ok(m) => m,
            Err(_) => return, // deleted meanwhile
        };
        match result {
            Ok(info) => {
                m.status = "ready".into();
                m.message = String::new();
                m.info = info;
            }
            Err(e) => {
                m.status = "failed".into();
                m.message = format!("{e:#}");
            }
        }
        let _ = app2.db().update_model(&m);
        app2.emit(json!({"type": "model", "model": m.to_json()}));
        app2.emit(json!({"type": "project", "project_id": m.project_id}));
    });
    Ok(row)
}

/// The file the mesher reads (STEP/IGES as is; OBJ converted to STL).
pub fn mesher_input(dir: &Path, format: &str) -> PathBuf {
    match format {
        "step" => dir.join("source.step"),
        "iges" => dir.join("source.igs"),
        _ => dir.join("source.stl"),
    }
}

fn progress_model(app: &App, id: &str, msg: &str) {
    let found = app.db().model(id);
    if let Ok(mut m) = found {
        m.message = msg.into();
        let _ = app.db().update_model(&m);
        app.emit(json!({"type": "model", "model": m.to_json()}));
    }
}

pub fn process(app: &App, model_id: &str) -> Result<Value> {
    let m = app.db().model(model_id)?;
    let dir = app.model_dir(&m.project_id, &m.id);
    if m.format == "obj" {
        let mesh = read_obj(&std::fs::read(dir.join("source.obj"))?)?;
        std::fs::write(dir.join("source.stl"), simlab_core::stl::write_binary_stl(&mesh))?;
    }
    if m.format == "3mf" {
        progress_model(app, model_id, "Reading the 3MF");
        let (src, out) = (dir.join("source.3mf"), dir.join("source.stl"));
        run_mesher(app, &["to-stl".as_ref(), src.as_os_str(), out.as_os_str()], |_, _| {})?;
    }
    if m.format == "stl" || m.format == "3mf" {
        // fail early, in plain words, on a broken file
        simlab_core::stl::read_stl(&std::fs::read(dir.join("source.stl"))?).context("this STL could not be read")?;
    }
    progress_model(app, model_id, "Finding the part's faces and holes");
    let out = dir.join("surface");
    run_mesher(app, &["surface".as_ref(), mesher_input(&dir, &m.format).as_os_str(), out.as_os_str()], |_, msg| progress_model(app, model_id, msg))?;
    let surf = read_surface(&out, m.unit_mm)?;
    progress_model(app, model_id, "Measuring");
    let header: Value = serde_json::from_slice(&std::fs::read(out.join("surface.json"))?)?;
    let mut info = analyse(&surf, &header);
    if header["source"] != "cad" {
        let (crossing, folds) = simlab_core::decimate::surface_defects(&surf);
        if crossing + folds > 0 {
            let w = format!(
                "This file has {} places where the surface crosses or folds onto itself. The wind tunnel and propeller tests still work; the strength test may fail to mesh it. Repairing it first helps (PrusaSlicer: right-click → Fix by Netfabb).",
                crossing + folds
            );
            match info["warnings"].as_array_mut() {
                Some(a) => a.push(json!(w)),
                None => info["warnings"] = json!([w]),
            }
            info["surface_defects"] = json!({"crossing": crossing, "folds": folds});
        }
    }
    simlab_core::write_display_mesh(&dir, "surface", &surf)?;
    Ok(info)
}

/// The surface from the mesher, in model millimetres (file units × unit_mm).
pub fn read_surface(out: &Path, unit_mm: f64) -> Result<TriMesh> {
    let pos = read_le::<f64>(&out.join("surface_positions.f64"))?;
    let tri = read_le::<u32>(&out.join("surface_triangles.u32"))?;
    let patch = read_le::<u32>(&out.join("surface_patch.u32"))?;
    Ok(TriMesh {
        positions: pos.chunks_exact(3).map(|c| [c[0] * unit_mm, c[1] * unit_mm, c[2] * unit_mm]).collect(),
        triangles: tri.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
        patch,
    })
}

/// The model's surface for the solvers, in model mm (from the stored display buffers).
pub fn load_surface_mm(app: &App, m: &ModelRow) -> Result<TriMesh> {
    let dir = app.model_dir(&m.project_id, &m.id);
    let pos = read_le::<f32>(&dir.join("surface.positions.f32"))?;
    let idx = read_le::<u32>(&dir.join("surface.indices.u32"))?;
    let patch = read_le::<u32>(&dir.join("surface.patch.u32"))?;
    Ok(TriMesh {
        positions: pos.chunks_exact(3).map(|c| [c[0] as f64, c[1] as f64, c[2] as f64]).collect(),
        triangles: idx.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
        patch,
    })
}

/// Runs the Gmsh helper, passing its JSON progress lines to `on_progress`.
pub fn run_mesher(app: &App, args: &[&std::ffi::OsStr], mut on_progress: impl FnMut(f32, &str)) -> Result<()> {
    let py = app.python();
    if !py.exists() {
        bail!("the Gmsh mesher is not set up yet: open Settings → System check and press \"Set up Gmsh\"");
    }
    let mut child = Command::new(&py)
        .arg("-I")
        .arg(app.resources.mesher.join("simlab_mesher.py"))
        .args(args)
        .arg("--threads")
        .arg(crate::system::performance_cores().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start the mesher")?;
    let stdout = child.stdout.take().unwrap();
    let mut error = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if let Some(e) = v["error"].as_str() {
                error = Some(e.to_string());
            } else if let Some(p) = v["progress"].as_f64() {
                on_progress(p as f32, v["message"].as_str().unwrap_or(""));
            }
        }
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").to_string();
        bail!("{}", error.unwrap_or(if last.is_empty() { "the mesher failed".into() } else { last }));
    }
    Ok(())
}

fn read_obj(bytes: &[u8]) -> Result<TriMesh> {
    let s = String::from_utf8_lossy(bytes);
    let mut v: Vec<Vec3> = vec![];
    let mut tris: Vec<[Vec3; 3]> = vec![];
    for line in s.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f64> = it.take(3).filter_map(|x| x.parse().ok()).collect();
                if c.len() == 3 {
                    v.push([c[0], c[1], c[2]]);
                }
            }
            Some("f") => {
                let idx: Vec<usize> = it
                    .filter_map(|w| w.split('/').next()?.parse::<i64>().ok())
                    .map(|i| if i < 0 { (v.len() as i64 + i) as usize } else { (i - 1) as usize })
                    .collect();
                for k in 1..idx.len().saturating_sub(1) {
                    if let (Some(a), Some(b), Some(c)) = (v.get(idx[0]), v.get(idx[k]), v.get(idx[k + 1])) {
                        tris.push([*a, *b, *c]);
                    }
                }
            }
            _ => {}
        }
    }
    if tris.is_empty() {
        bail!("the OBJ has no faces");
    }
    Ok(simlab_core::stl::weld(&tris))
}

// ------------------------------------------------------------- analysis

struct PatchAcc {
    area: f64,
    centroid: Vec3,
    nn: [[f64; 3]; 3],
    nsum: Vec3,
    tris: Vec<usize>,
}

/// Measures the part and its patches; guesses what it is.
pub fn analyse(m: &TriMesh, header: &Value) -> Value {
    let (lo, hi) = m.bounds();
    let size = sub(hi, lo);
    let mut area = 0.0;
    let mut acc: HashMap<u32, PatchAcc> = HashMap::new();
    for t in 0..m.triangles.len() {
        let a = m.triangle_area(t);
        if a <= 0.0 {
            continue;
        }
        area += a;
        let n = m.triangle_normal(t);
        let [i, j, k] = m.triangles[t];
        let c = [0, 1, 2].map(|d| (m.positions[i as usize][d] + m.positions[j as usize][d] + m.positions[k as usize][d]) / 3.0);
        let p = acc.entry(m.patch[t]).or_insert(PatchAcc { area: 0.0, centroid: [0.0; 3], nn: [[0.0; 3]; 3], nsum: [0.0; 3], tris: vec![] });
        p.area += a;
        for d in 0..3 {
            p.centroid[d] += a * c[d];
            p.nsum[d] += a * n[d];
            for e in 0..3 {
                p.nn[d][e] += a * n[d] * n[e];
            }
        }
        p.tris.push(t);
    }
    // CAD files name each face's shape; for STL the patches are found by
    // angle and their shape is measured below
    let from_cad = header["source"] == "cad";
    let cad_kind: HashMap<u64, String> = header["patches"]
        .as_array()
        .map(|a| a.iter().filter_map(|p| Some((p["id"].as_u64()?, p["kind"].as_str()?.to_ascii_lowercase()))).collect())
        .unwrap_or_default();
    let cad_kind = if from_cad { cad_kind } else { HashMap::new() };

    let mut patches = vec![];
    let mut ids: Vec<u32> = acc.keys().copied().collect();
    ids.sort();
    for id in ids {
        let p = &acc[&id];
        let c = p.centroid.map(|x| x / p.area);
        let mean_n = p.nsum.map(|x| x / p.area);
        let mut v = json!({"id": id, "area_mm2": round(p.area, 3), "center_mm": round3(c)});
        let flatness = norm(mean_n); // 1 for a flat face
        let kind = cad_kind.get(&(id as u64)).cloned();
        let (axis, spread) = smallest_eigvec(p.nn.map(|r| r.map(|x| x / p.area)));
        let is_plane = kind.as_deref().map(|k| k == "plane").unwrap_or(flatness > 0.995);
        let is_cyl = kind.as_deref().map(|k| k == "cylinder").unwrap_or(!is_plane && spread < 0.01);
        if is_plane {
            v["kind"] = json!("plane");
            v["normal"] = json!(round3(normalize(mean_n)));
        } else if is_cyl {
            // radius and centre line from the triangles: points around the axis
            let (center, radius, inward) = fit_cylinder(m, &p.tris, axis);
            v["kind"] = json!("cylinder");
            v["axis"] = json!(round3(axis));
            v["radius_mm"] = json!(round(radius, 4));
            v["center_mm"] = json!(round3(center));
            v["hole"] = json!(inward);
        } else {
            let k = kind.unwrap_or_default();
            v["kind"] = json!(match k.as_str() { "cone" | "sphere" | "torus" => k.as_str(), _ => "other" });
        }
        patches.push(v);
    }

    let (watertight, open_edges) = watertight(m);
    let volume = m.volume().abs();
    let mut info = json!({
        "bbox_mm": {"min": round3(lo), "max": round3(hi)},
        "size_mm": round3(size),
        "volume_mm3": round(volume, 3),
        "area_mm2": round(area, 3),
        "triangles": m.triangles.len(),
        "watertight": watertight,
        "patches": patches,
    });
    if !watertight {
        info["warnings"] = json!([format!("The surface has {open_edges} open edges, so it is not a closed solid. The wind tunnel still works; the stress test may fail until it is repaired.")]);
    }
    let longest = size.iter().cloned().fold(0.0, f64::max);
    if longest < 2.0 || longest > 2000.0 {
        info["unit_question"] = json!(format!(
            "This part is {:.3} units across. Is that millimetres? If the file was made in inches or metres, change its units.",
            longest
        ));
    }
    info["suggestions"] = suggest(m, &patches);
    info
}

fn round(x: f64, d: i32) -> f64 {
    let s = 10f64.powi(d);
    (x * s).round() / s
}
fn round3(v: Vec3) -> [f64; 3] {
    v.map(|x| round(x, 4))
}

/// Eigenvector of the smallest eigenvalue of a symmetric 3×3 (Jacobi), with
/// that eigenvalue. For a cylinder's normals it is the axis.
fn smallest_eigvec(mut a: [[f64; 3]; 3]) -> (Vec3, f64) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let (mut p, mut q, mut big) = (0, 1, 0.0);
        for i in 0..3 {
            for j in i + 1..3 {
                if a[i][j].abs() > big {
                    big = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if big < 1e-14 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let t = if theta == 0.0 { 1.0 } else { t };
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        for k in 0..3 {
            let (akp, akq) = (a[k][p], a[k][q]);
            a[k][p] = c * akp - s * akq;
            a[k][q] = s * akp + c * akq;
        }
        for k in 0..3 {
            let (apk, aqk) = (a[p][k], a[q][k]);
            a[p][k] = c * apk - s * aqk;
            a[q][k] = s * apk + c * aqk;
        }
        for k in 0..3 {
            let (vkp, vkq) = (v[k][p], v[k][q]);
            v[k][p] = c * vkp - s * vkq;
            v[k][q] = s * vkp + c * vkq;
        }
    }
    let i = (0..3).min_by(|&i, &j| a[i][i].partial_cmp(&a[j][j]).unwrap()).unwrap();
    (normalize([v[0][i], v[1][i], v[2][i]]), a[i][i])
}

/// Least-squares circle in the plane across `axis`: centre (at the patch's
/// mid-height), radius, and whether the normals point at the axis (a hole).
fn fit_cylinder(m: &TriMesh, tris: &[usize], axis: Vec3) -> (Vec3, f64, bool) {
    let u = normalize(if axis[0].abs() < 0.9 { cross(axis, [1.0, 0.0, 0.0]) } else { cross(axis, [0.0, 1.0, 0.0]) });
    let w = cross(axis, u);
    // Kåsa fit: x² + y² = 2ax + 2by + c
    let (mut s, mut rhs) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
    let mut hsum = 0.0;
    let mut n = 0.0;
    let mut pts = vec![];
    for &t in tris {
        for &i in &m.triangles[t] {
            let p = m.positions[i as usize];
            let (x, y) = (dot(p, u), dot(p, w));
            pts.push((x, y, t));
            let row = [2.0 * x, 2.0 * y, 1.0];
            let b = x * x + y * y;
            for r in 0..3 {
                rhs[r] += row[r] * b;
                for c in 0..3 {
                    s[r][c] += row[r] * row[c];
                }
            }
            hsum += dot(p, axis);
            n += 1.0;
        }
    }
    let sol = solve3(s, rhs).unwrap_or([0.0; 3]);
    let (a, b) = (sol[0], sol[1]);
    let r = (sol[2] + a * a + b * b).max(0.0).sqrt();
    let center = [0, 1, 2].map(|d| a * u[d] + b * w[d] + hsum / n * axis[d]);
    // a hole's surface normals point toward its axis
    let mut inward = 0.0;
    for &t in tris {
        let nrm = m.triangle_normal(t);
        let [i, _, _] = m.triangles[t];
        let p = m.positions[i as usize];
        let radial = sub(p, center);
        let radial = sub(radial, simlab_core::scale(axis, dot(radial, axis)));
        inward += -dot(nrm, radial) * m.triangle_area(t);
    }
    (center, r, inward > 0.0)
}

fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(a);
    if d.abs() < 1e-300 {
        return None;
    }
    let mut out = [0.0; 3];
    for k in 0..3 {
        let mut m = a;
        for r in 0..3 {
            m[r][k] = b[r];
        }
        out[k] = det(m) / d;
    }
    Some(out)
}

fn watertight(m: &TriMesh) -> (bool, usize) {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::with_capacity(m.triangles.len() * 3 / 2);
    for t in &m.triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    let open = edges.values().filter(|&&c| c != 2).count();
    (open == 0, open)
}

/// Guesses: a propeller (via the blade model's detector) and motor mounts
/// (four equal small holes on a square: 9×9, 12×12, 16×16, 19×19, 25.5 mm…).
fn suggest(m: &TriMesh, patches: &[Value]) -> Value {
    let mut out = json!({"looks_like": "part"});
    // small through-holes, M2–M3 sized
    let holes: Vec<(u64, Vec3, Vec3)> = patches
        .iter()
        .filter(|p| p["kind"] == "cylinder" && p["hole"] == true)
        .filter(|p| p["radius_mm"].as_f64().map(|r| (0.7..=1.9).contains(&r)).unwrap_or(false))
        .filter_map(|p| {
            let c = p["center_mm"].as_array()?;
            let a = p["axis"].as_array()?;
            Some((p["id"].as_u64()?, [c[0].as_f64()?, c[1].as_f64()?, c[2].as_f64()?], [a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?]))
        })
        .collect();
    let mut mounts = vec![];
    let mut used = vec![false; holes.len()];
    for i in 0..holes.len() {
        if used[i] {
            continue;
        }
        // nearest three holes with a matching axis
        let mut near: Vec<(f64, usize)> = (0..holes.len())
            .filter(|&j| j != i && !used[j] && dot(holes[i].2, holes[j].2).abs() > 0.98)
            .map(|j| (norm(sub(holes[j].1, holes[i].1)), j))
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if near.len() < 3 {
            continue;
        }
        let quad = [i, near[0].1, near[1].1, near[2].1];
        let c = [0, 1, 2].map(|d| quad.iter().map(|&q| holes[q].1[d]).sum::<f64>() / 4.0);
        let radii: Vec<f64> = quad.iter().map(|&q| norm(sub(holes[q].1, c))).collect();
        let rmean = radii.iter().sum::<f64>() / 4.0;
        let side = rmean * std::f64::consts::SQRT_2; // square pattern side
        let square = radii.iter().all(|r| (r - rmean).abs() < 0.05 * rmean) && (5.0..=32.0).contains(&side);
        if square {
            for &q in &quad {
                used[q] = true;
            }
            mounts.push(json!({
                "center_mm": round3(c), "axis": round3(holes[i].2),
                "bolt_circle_mm": round(2.0 * rmean, 2), "pattern_mm": round(side, 1),
                "patches": quad.iter().map(|&q| holes[q].0).collect::<Vec<_>>(),
            }));
        }
    }
    if !mounts.is_empty() {
        out["looks_like"] = json!("frame");
        out["motor_mounts"] = json!(mounts);
    }
    if let Some(p) = crate::solve::detect_prop(m) {
        if mounts.is_empty() {
            out["looks_like"] = json!("propeller");
        }
        out["prop"] = p;
    }
    out
}
