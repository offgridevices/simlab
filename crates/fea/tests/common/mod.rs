#![allow(dead_code)]
use simlab_core::{Load, Material, MeshQuality, Silent, StrengthConfig, Support, TetMesh, Vec3};
use simlab_fea::{SolveOptions, SolverChoice, StrengthInput, StrengthResult, solve_strength};

pub fn cfg(support: Support, loads: Vec<Load>, up: Vec3) -> StrengthConfig {
    StrengthConfig { material_id: "test".into(), print_up: up, support, loads, quality: MeshQuality::Normal, regions: vec![] }
}

pub fn run(mesh: &TetMesh, mat: &Material, c: &StrengthConfig, solver: SolverChoice) -> StrengthResult {
    let input = StrengthInput { mesh, material: mat, config: c, model_unit_m: 0.001, face_tractions: None };
    let opts = SolveOptions { solver, ..Default::default() };
    solve_strength(&input, &opts, &Silent).expect("solve")
}

pub fn steel() -> Material {
    Material::isotropic("steel", "steel", 7850.0, 210e3, 0.3, 400.0)
}

pub fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs()
}

/// Nodes whose position satisfies `f`.
pub fn nodes_where(mesh: &TetMesh, f: impl Fn(Vec3) -> bool) -> Vec<usize> {
    (0..mesh.nodes.len()).filter(|&i| f(mesh.nodes[i])).collect()
}

pub fn mesher() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mesher");
    let py = root.join(".venv/bin/python");
    let script = root.join("simlab_mesher.py");
    if py.exists() && script.exists() { Some((py, script)) } else { None }
}

pub fn scratch() -> std::path::PathBuf {
    let p = std::env::var("SIMLAB_FEA_SCRATCH").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("simlab-fea-tests"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Runs a gmsh python snippet (OCC) that writes `path` (a STEP file).
pub fn gmsh_step(py: &std::path::Path, code: &str, path: &std::path::Path) {
    let full = format!(
        "import gmsh, math\ngmsh.initialize()\ngmsh.option.setNumber('General.Terminal', 0)\ngmsh.model.add('m')\n{code}\ngmsh.model.occ.synchronize()\ngmsh.write(r'{}')\ngmsh.finalize()\n",
        path.display()
    );
    let out = std::process::Command::new(py).arg("-c").arg(full).output().expect("python");
    assert!(out.status.success(), "gmsh failed: {}", String::from_utf8_lossy(&out.stderr));
}

pub fn mesh_step(py: &std::path::Path, script: &std::path::Path, step: &std::path::Path, out: &std::path::Path, size: f64, unit: f64) -> TetMesh {
    let o = std::process::Command::new(py)
        .arg(script)
        .arg("volume")
        .arg(step)
        .arg(out)
        .arg("--size")
        .arg(size.to_string())
        .arg("--unit")
        .arg(unit.to_string())
        .output()
        .expect("mesher");
    assert!(o.status.success(), "mesher failed: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    TetMesh::read_dir(out).expect("read mesh")
}

/// Runs CalculiX on `<dir>/<job>.inp`: `ccx` on PATH if present, else the
/// `simlab-reference:1` Docker image (Colima socket). Returns false when
/// neither is available.
pub fn run_ccx(dir: &std::path::Path, job: &str) -> bool {
    let ccx = std::env::var("CCX").unwrap_or_else(|_| "ccx".into());
    if let Ok(o) = std::process::Command::new(&ccx).arg("-i").arg(job).current_dir(dir).output() {
        assert!(o.status.success(), "ccx failed: {}", String::from_utf8_lossy(&o.stdout));
        return true;
    }
    let host = std::env::var("DOCKER_HOST").unwrap_or_else(|_| format!("unix://{}/.colima/default/docker.sock", std::env::var("HOME").unwrap()));
    let probe = std::process::Command::new("docker").env("DOCKER_HOST", &host).args(["image", "inspect", "simlab-reference:1"]).output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        return false;
    }
    // stream the deck in and the results out (Colima only shares $HOME with the VM)
    use std::io::Write;
    let mut child = std::process::Command::new("docker")
        .env("DOCKER_HOST", &host)
        .args([
            "run",
            "--rm",
            "-i",
            "simlab-reference:1",
            "sh",
            "-c",
            &format!("cd /tmp && cat > {job}.inp && ccx -i {job} > ccx.log 2>&1; if [ ! -s {job}.frd ] || grep -q ERROR ccx.log; then cat ccx.log >&2; exit 1; fi; cat {job}.frd"),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("docker");
    let deck = std::fs::read(dir.join(format!("{job}.inp"))).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&deck).unwrap();
    });
    let o = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(o.status.success(), "ccx in docker failed: {}", String::from_utf8_lossy(&o.stderr));
    std::fs::write(dir.join(format!("{job}.frd")), &o.stdout).unwrap();
    true
}

/// Writes the deck, runs CalculiX, compares node by node with our result.
/// None when CalculiX is not available.
pub fn ccx_compare(name: &str, mesh: &TetMesh, prep: &simlab_fea::Prepared, ours: &StrengthResult, native: bool) -> Option<simlab_fea::calculix::Comparison> {
    use simlab_fea::calculix::*;
    let dir = scratch().join("ccx").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    write_inp_file(&dir.join("job.inp"), mesh, prep, &InpOptions { native_loads: native, ..Default::default() }).unwrap();
    if !run_ccx(&dir, "job") {
        return None;
    }
    let frd = read_frd_file(&dir.join("job.frd")).unwrap();
    let n = mesh.nodes.len();
    let mut ru = frd.displacements(n).unwrap();
    if let Some(ir) = &ours.inertia_relief {
        // our free-flying displacements have the rigid motion removed; do the same
        simlab_fea::strength::remove_rigid_motion(mesh, prep.elastic.density, &prep.mass, ir.center_m, &mut ru);
    }
    let c = compare(&ours.displacement, &ours.stress, &ru, &frd.stresses(n).unwrap());
    println!(
        "CalculiX vs SimLab [{name}{}]: displacement max diff {:.3e} of peak; von Mises max diff {:.3e}, RMS {:.3e} of peak; peaks vM {:.6e} vs {:.6e}, |u| {:.6e} vs {:.6e}",
        if native { ", CalculiX load cards" } else { ", identical nodal loads" },
        c.displacement,
        c.von_mises,
        c.von_mises_rms,
        c.peak_von_mises.0,
        c.peak_von_mises.1,
        c.peak_displacement.0,
        c.peak_displacement.1
    );
    Some(c)
}
