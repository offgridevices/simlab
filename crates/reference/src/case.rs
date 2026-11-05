//! Writes an OpenFOAM case directory from a [`Plan`]. Plain text files only;
//! nothing here needs OpenFOAM or Docker.
//!
//! Layout written:
//!
//! ```text
//! case/
//!   Allrun                       the whole pipeline (runs inside the container)
//!   case.json                    the plan, for people and for SimLab
//!   constant/triSurface/part.stl the part, metres
//!   constant/{transportProperties,turbulenceProperties,MRFProperties}
//!   system/{controlDict,fvSchemes,fvSolution,blockMeshDict,snappyHexMeshDict,
//!           surfaceFeatureExtractDict,decomposeParDict,sampleDict,*.points}
//!   0.orig/{U,p,k,omega,nut}     starting fields (copied to 0/ after meshing)
//! ```

use crate::plan::{Kind, Plan, Region, Turbulence};
use anyhow::{Context, Result};
use simlab_core::{TriMesh, Vec3, stl::write_binary_stl};
use std::fmt::Write as _;
use std::path::Path;

pub const PART: &str = "part";

fn v(p: Vec3) -> String {
    format!("({} {} {})", g(p[0]), g(p[1]), g(p[2]))
}
/// full precision, compact
fn g(x: f64) -> String {
    if x == 0.0 { "0".into() } else { format!("{x:.9e}") }
}

fn header(class: &str, object: &str, location: &str) -> String {
    format!(
        "/*--------------------------------*- C++ -*----------------------------------*\\\n  \
         SimLab reference run (OpenFOAM v2606), generated - edits are overwritten\n\
         \\*---------------------------------------------------------------------------*/\n\
         FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n    location \"{location}\";\n    object {object};\n}}\n\n"
    )
}

fn write(dir: &Path, rel: &str, body: &str) -> Result<()> {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, body).with_context(|| format!("write {}", p.display()))
}

fn dict(dir: &Path, rel: &str, class: &str, body: &str) -> Result<()> {
    let (loc, obj) = rel.rsplit_once('/').unwrap_or(("", rel));
    write(dir, rel, &(header(class, obj, loc) + body))
}

/// Names of the six outer faces in frame order.
pub const FAR: [&str; 6] = ["inlet", "outlet", "sideA", "sideB", "sideC", "sideD"];

/// Writes everything except the sample point files (see `post`).
pub fn write_case(dir: &Path, surface: &TriMesh, p: &Plan, threads: usize) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    write(dir, &format!("constant/triSurface/{PART}.stl"), "")?;
    std::fs::write(dir.join(format!("constant/triSurface/{PART}.stl")), write_binary_stl(surface))?;
    dict(dir, "system/blockMeshDict", "dictionary", &block_mesh(p))?;
    dict(dir, "system/snappyHexMeshDict", "dictionary", &snappy(p))?;
    dict(
        dir,
        "system/surfaceFeatureExtractDict",
        "dictionary",
        &format!("{PART}.stl\n{{\n    extractionMethod extractFromSurface;\n    includedAngle 150;\n    writeObj no;\n}}\n"),
    )?;
    dict(dir, "system/decomposeParDict", "dictionary", &format!("numberOfSubdomains {};\nmethod scotch;\n", threads.max(1)))?;
    write(dir, "system/controlDict", &control_dict_file(p, false))?;
    dict(dir, "system/fvSchemes", "dictionary", FV_SCHEMES)?;
    dict(dir, "system/fvSolution", "dictionary", FV_SOLUTION)?;
    dict(dir, "constant/transportProperties", "dictionary", &format!("transportModel Newtonian;\nnu {};\n", g(p.nu)))?;
    let turb = match p.turbulence {
        Turbulence::Laminar => "simulationType laminar;\n".to_string(),
        Turbulence::KOmegaSst { .. } => "simulationType RAS;\nRAS\n{\n    RASModel kOmegaSST;\n    turbulence on;\n    printCoeffs on;\n}\n".into(),
    };
    dict(dir, "constant/turbulenceProperties", "dictionary", &turb)?;
    if let Some(r) = &p.rotor {
        dict(
            dir,
            "constant/MRFProperties",
            "dictionary",
            &format!(
                "rotor\n{{\n    cellZone rotorZone;\n    active yes;\n    nonRotatingPatches ();\n    origin {};\n    axis {};\n    omega {}; // {:.1} rpm\n}}\n",
                v(r.center),
                v(r.axis),
                g(r.omega),
                r.omega * 60.0 / std::f64::consts::TAU
            ),
        )?;
    } else {
        let _ = std::fs::remove_file(dir.join("constant/MRFProperties"));
    }
    write_fields(dir, p)?;
    write(dir, "Allrun", &allrun(threads))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("Allrun"), std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub fn block_mesh(p: &Plan) -> String {
    let (lo, hi) = (p.lo, p.hi);
    let corners = [
        [lo[0], lo[1], lo[2]],
        [hi[0], lo[1], lo[2]],
        [hi[0], hi[1], lo[2]],
        [lo[0], hi[1], lo[2]],
        [lo[0], lo[1], hi[2]],
        [hi[0], lo[1], hi[2]],
        [hi[0], hi[1], hi[2]],
        [lo[0], hi[1], hi[2]],
    ];
    let mut s = String::from("// a box aligned with the flow (e1), in model axes (metres)\nconvertToMeters 1;\n\nvertices\n(\n");
    for c in corners {
        let _ = writeln!(s, "    {}", v(p.frame.to_model(p.center, c)));
    }
    let _ = write!(
        s,
        ");\n\nblocks\n(\n    hex (0 1 2 3 4 5 6 7) ({} {} {}) simpleGrading (1 1 1)\n);\n\nedges ();\n\nboundary\n(\n",
        p.cells[0], p.cells[1], p.cells[2]
    );
    let faces = ["(0 4 7 3)", "(1 2 6 5)", "(0 1 5 4)", "(3 7 6 2)", "(0 3 2 1)", "(4 5 6 7)"];
    for (name, f) in FAR.iter().zip(faces) {
        let _ = writeln!(s, "    {name} {{ type patch; faces ({f}); }}");
    }
    s.push_str(");\n\nmergePatchPairs ();\n");
    s
}

pub fn snappy(p: &Plan) -> String {
    let mut geo = format!("    {PART}.stl {{ type triSurfaceMesh; name {PART}; }}\n");
    let mut regions = String::new();
    for r in &p.regions {
        match r {
            Region::Cylinder { name, p1, p2, radius, level } => {
                let _ = writeln!(geo, "    {name} {{ type cylinder; point1 {}; point2 {}; radius {}; }}", v(*p1), v(*p2), g(*radius));
                let _ = writeln!(regions, "        {name} {{ mode inside; levels ((1e15 {level})); }}");
            }
            Region::NearPart { levels } => {
                let l: Vec<String> = levels.iter().map(|(d, lv)| format!("({} {lv})", g(*d))).collect();
                let _ = writeln!(regions, "        {PART} {{ mode distance; levels ({}); }}", l.join(" "));
            }
        }
    }
    let mut surfaces = format!(
        "        {PART} {{ level ({} {}); patchInfo {{ type wall; }} }}\n",
        p.surface_level.0, p.surface_level.1
    );
    if let Some(r) = &p.rotor {
        let _ = writeln!(
            geo,
            "    rotorZone {{ type cylinder; point1 {}; point2 {}; radius {}; }}",
            v(r.zone_p1),
            v(r.zone_p2),
            g(r.zone_radius)
        );
        let _ = writeln!(
            surfaces,
            "        rotorZone {{ level ({l} {l}); faceZone rotorZone; cellZone rotorZone; cellZoneInside inside; faceType internal; }}",
            l = r.level
        );
    }
    let layers = if p.layers > 0 {
        format!("        {PART} {{ nSurfaceLayers {}; }}\n", p.layers)
    } else {
        String::new()
    };
    format!(
        "castellatedMesh true;\nsnap true;\naddLayers {add};\n\n\
geometry\n{{\n{geo}}};\n\n\
castellatedMeshControls\n{{\n    maxLocalCells 8000000;\n    maxGlobalCells 40000000;\n    minRefinementCells 10;\n    maxLoadUnbalance 0.10;\n    nCellsBetweenLevels 3;\n\
    features ( {{ file \"{PART}.eMesh\"; level {fl}; }} );\n\
    refinementSurfaces\n    {{\n{surfaces}    }}\n    resolveFeatureAngle 25;\n\
    refinementRegions\n    {{\n{regions}    }}\n\
    locationInMesh {lim};\n    allowFreeStandingZoneFaces true;\n}}\n\n\
snapControls\n{{\n    nSmoothPatch 3;\n    tolerance 2.0;\n    nSolveIter 50;\n    nRelaxIter 5;\n    nFeatureSnapIter 10;\n    implicitFeatureSnap true;\n    explicitFeatureSnap false;\n    multiRegionFeatureSnap false;\n}}\n\n\
addLayersControls\n{{\n    relativeSizes true;\n    layers\n    {{\n{layers}    }}\n    expansionRatio 1.2;\n    finalLayerThickness 0.4;\n    minThickness 0.1;\n    nGrow 0;\n    featureAngle 120;\n    slipFeatureAngle 30;\n    nRelaxIter 3;\n    nSmoothSurfaceNormals 1;\n    nSmoothNormals 3;\n    nSmoothThickness 10;\n    maxFaceThicknessRatio 0.5;\n    maxThicknessToMedialRatio 0.3;\n    minMedialAxisAngle 90;\n    nBufferCellsNoExtrude 0;\n    nLayerIter 50;\n}}\n\n\
meshQualityControls\n{{\n    maxNonOrtho 65;\n    maxBoundarySkewness 20;\n    maxInternalSkewness 4;\n    maxConcave 80;\n    minVol 1e-30;\n    minTetQuality 1e-30;\n    minArea -1;\n    minTwist 0.02;\n    minDeterminant 0.001;\n    minFaceWeight 0.05;\n    minVolRatio 0.01;\n    minTriangleTwist -1;\n    nSmoothScale 4;\n    errorReduction 0.75;\n    relaxed {{ maxNonOrtho 75; }}\n}}\n\n\
mergeTolerance 1e-6;\n",
        add = if p.layers > 0 { "true" } else { "false" },
        fl = p.feature_level,
        lim = v(p.location_in_mesh),
    )
}

/// The whole controlDict file.
pub fn control_dict_file(p: &Plan, stop: bool) -> String {
    header("dictionary", "controlDict", "system") + &control_dict(p, stop)
}

/// `stop`: ask a running solver to write and finish now (the engine edits
/// this file when the forces have settled; runTimeModifiable picks it up).
pub fn control_dict(p: &Plan, stop: bool) -> String {
    let mut f = format!(
        "    forces\n    {{\n        type forces;\n        libs (forces);\n        writeControl timeStep;\n        writeInterval 1;\n        log true;\n        patches ({PART});\n        rho rhoInf;\n        rhoInf {rho};\n        CofR {cofr};\n    }}\n",
        rho = g(p.rho),
        cofr = v(p.cof_r)
    );
    if p.speed > 0.0 {
        let _ = write!(
            f,
            "    forceCoeffs\n    {{\n        type forceCoeffs;\n        libs (forces);\n        writeControl timeStep;\n        writeInterval 1;\n        log false;\n        patches ({PART});\n        rho rhoInf;\n        rhoInf {};\n        CofR {};\n        liftDir {};\n        dragDir {};\n        pitchAxis {};\n        magUInf {};\n        lRef {};\n        Aref {};\n    }}\n",
            g(p.rho),
            v(p.cof_r),
            v(p.lift_dir),
            v(p.drag_dir),
            v(p.side_dir),
            g(p.speed),
            g(p.ref_length),
            g(p.ref_area.max(1e-30))
        );
    }
    if matches!(p.turbulence, Turbulence::KOmegaSst { .. }) {
        f.push_str("    yPlus\n    {\n        type yPlus;\n        libs (fieldFunctionObjects);\n        writeControl writeTime;\n    }\n");
    }
    format!(
        "application simpleFoam;\nstartFrom latestTime;\nstartTime 0;\nstopAt {stop};\nendTime {end};\ndeltaT 1;\n\
writeControl timeStep;\nwriteInterval 500;\npurgeWrite 2;\nwriteFormat binary;\nwritePrecision 8;\nwriteCompression off;\n\
timeFormat general;\ntimePrecision 6;\nrunTimeModifiable true;\n\nfunctions\n{{\n{f}}}\n",
        stop = if stop { "writeNow" } else { "endTime" },
        end = p.max_iterations,
    )
}

const FV_SCHEMES: &str = "ddtSchemes { default steadyState; }\n\
gradSchemes { default cellLimited Gauss linear 1; }\n\
divSchemes\n{\n    default none;\n    div(phi,U) bounded Gauss linearUpwindV grad(U);\n    div(phi,k) bounded Gauss upwind;\n    div(phi,omega) bounded Gauss upwind;\n    div((nuEff*dev2(T(grad(U))))) Gauss linear;\n}\n\
laplacianSchemes { default Gauss linear limited 0.5; }\n\
interpolationSchemes { default linear; }\n\
snGradSchemes { default limited 0.5; }\n\
wallDist { method meshWave; }\n";

const FV_SOLUTION: &str = "solvers\n{\n    p { solver GAMG; smoother GaussSeidel; tolerance 1e-7; relTol 0.05; }\n    \"(U|k|omega)\" { solver smoothSolver; smoother symGaussSeidel; tolerance 1e-8; relTol 0.1; }\n}\n\n\
SIMPLE\n{\n    nNonOrthogonalCorrectors 1;\n    consistent yes;\n    residualControl { p 1e-5; U 1e-6; \"(k|omega)\" 1e-6; }\n}\n\n\
relaxationFactors\n{\n    equations { U 0.7; \".*\" 0.7; }\n    fields { p 0.3; }\n}\n";

/// Boundary conditions per field: (patch → entry).
fn write_fields(dir: &Path, p: &Plan) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir.join("0.orig"));
    let hover = p.kind == Kind::SpinHover;
    let u0 = if hover { [0.0; 3] } else { p.u_inf };
    let uv = v(u0);
    // U
    let mut b = format!("    {PART} {{ type noSlip; }}\n");
    for (i, n) in FAR.iter().enumerate() {
        let e = if hover {
            "type pressureInletOutletVelocity; value uniform (0 0 0);".to_string()
        } else if i == 0 {
            format!("type fixedValue; value uniform {uv};")
        } else if i == 1 {
            format!("type inletOutlet; inletValue uniform (0 0 0); value uniform {uv};")
        } else {
            "type slip;".into()
        };
        let _ = writeln!(b, "    {n} {{ {e} }}");
    }
    field(dir, "U", "volVectorField", "[0 1 -1 0 0 0 0]", &format!("uniform {uv}"), &b)?;
    // p (kinematic, m²/s²; 0 = ambient)
    let mut b = format!("    {PART} {{ type zeroGradient; }}\n");
    for (i, n) in FAR.iter().enumerate() {
        let e = if hover {
            "type totalPressure; p0 uniform 0; value uniform 0;"
        } else if i == 0 {
            "type zeroGradient;"
        } else if i == 1 {
            "type fixedValue; value uniform 0;"
        } else {
            "type slip;"
        };
        let _ = writeln!(b, "    {n} {{ {e} }}");
    }
    field(dir, "p", "volScalarField", "[0 2 -2 0 0 0 0]", "uniform 0", &b)?;
    if let Turbulence::KOmegaSst { k, omega } = p.turbulence {
        for (name, val, wall, dim) in [
            ("k", k, "kqRWallFunction", "[0 2 -2 0 0 0 0]"),
            ("omega", omega, "omegaWallFunction", "[0 0 -1 0 0 0 0]"),
        ] {
            let x = g(val);
            let mut b = format!("    {PART} {{ type {wall}; value uniform {x}; }}\n");
            for (i, n) in FAR.iter().enumerate() {
                let e = if hover || i == 1 {
                    format!("type inletOutlet; inletValue uniform {x}; value uniform {x};")
                } else if i == 0 {
                    format!("type fixedValue; value uniform {x};")
                } else {
                    "type slip;".into()
                };
                let _ = writeln!(b, "    {n} {{ {e} }}");
            }
            field(dir, name, "volScalarField", dim, &format!("uniform {x}"), &b)?;
        }
        let mut b = format!("    {PART} {{ type nutUSpaldingWallFunction; value uniform 0; }}\n");
        for n in FAR {
            let _ = writeln!(b, "    {n} {{ type calculated; value uniform 0; }}");
        }
        field(dir, "nut", "volScalarField", "[0 2 -1 0 0 0 0]", "uniform 0", &b)?;
    }
    Ok(())
}

fn field(dir: &Path, name: &str, class: &str, dims: &str, internal: &str, boundary: &str) -> Result<()> {
    write(
        dir,
        &format!("0.orig/{name}"),
        &(header(class, name, "0") + &format!("dimensions {dims};\n\ninternalField {internal};\n\nboundaryField\n{{\n{boundary}}}\n")),
    )
}

/// The pipeline, run inside the container as `bash /case/Allrun`. Each step
/// prints `@@SIMLAB stage <name>` first and tees its output to log.<name>,
/// so the engine can follow along from the container's output.
pub fn allrun(threads: usize) -> String {
    let n = threads.max(1);
    let par = n > 1;
    let mpi = format!("mpirun --allow-run-as-root --oversubscribe -np {n}");
    let mut s = String::from(
        "#!/bin/bash\n# SimLab reference run: OpenFOAM v2606 in simlab-reference:1. Generated.\n\
# Re-run by hand:  docker run --rm -v \"$PWD\":/case --entrypoint bash simlab-reference:1 /case/Allrun\n\
for f in /usr/lib/openfoam/openfoam*/etc/bashrc; do . \"$f\"; done\n\
set -eo pipefail\ncd /case\n\
step() { local name=$1; shift; echo \"@@SIMLAB stage $name\"; \"$@\" 2>&1 | tee log.$name; }\n\
rm -rf processor* 0 [1-9]* postProcessing log.* constant/polyMesh constant/extendedFeatureEdgeMesh constant/triSurface/*.eMesh\n\
step feature surfaceFeatureExtract\nstep blockMesh blockMesh\n",
    );
    if par {
        let _ = write!(
            s,
            "step decomposeMesh decomposePar -force\n\
step snappy {mpi} snappyHexMesh -parallel -overwrite\n\
step reconstructMesh reconstructParMesh -constant -mergeTol 1e-6\n\
rm -rf processor*\n"
        );
    } else {
        s.push_str("step snappy snappyHexMesh -overwrite\n");
    }
    s.push_str("step checkMesh checkMesh -constant || true\ncp -r 0.orig 0\n");
    if par {
        let _ = write!(
            s,
            "step decompose decomposePar -force\n\
step solve {mpi} simpleFoam -parallel\n\
step sample {mpi} postProcess -parallel -dict system/sampleDict -fields '(p U)' -latestTime\n\
step reconstruct reconstructPar -latestTime\n\
rm -rf processor*\n"
        );
    } else {
        s.push_str("step solve simpleFoam\nstep sample postProcess -dict system/sampleDict -fields '(p U)' -latestTime\n");
    }
    s.push_str("echo \"@@SIMLAB stage done\"\n");
    s
}

/// The sampling dictionary: surface pressure on the part's faces, pressure
/// just off each vertex of the part (`vertices.points`), and the flow
/// slices (`<name>.points`).
pub fn sample_dict(slice_names: &[String]) -> String {
    let mut sets = String::from("            vertices { type cloud; axis xyz; #include \"vertices.points\" }\n");
    for n in slice_names {
        let _ = writeln!(sets, "            {n} {{ type cloud; axis xyz; #include \"{n}.points\" }}");
    }
    header("dictionary", "sampleDict", "system")
        + &format!(
            "functions\n{{\n    partSurface\n    {{\n        type surfaces;\n        libs (sampling);\n        interpolationScheme cellPoint;\n        surfaceFormat raw;\n        fields (p);\n        surfaces\n        {{\n            {PART} {{ type patch; patches ({PART}); interpolate false; }}\n        }}\n    }}\n\
    probes\n    {{\n        type sets;\n        libs (sampling);\n        interpolationScheme cellPoint;\n        setFormat raw;\n        fields (p U);\n        sets\n        {{\n{sets}        }}\n    }}\n}}\n"
        )
}

pub fn points_file(points: &[Vec3]) -> String {
    let mut s = String::with_capacity(points.len() * 48 + 16);
    let _ = writeln!(s, "points\n{}\n(", points.len());
    for p in points {
        let _ = writeln!(s, "({:.9e} {:.9e} {:.9e})", p[0], p[1], p[2]);
    }
    s.push_str(");\n");
    s
}
