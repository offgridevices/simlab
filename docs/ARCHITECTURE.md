# SimLab architecture

SimLab by OffGrid tests 3D-printed drone parts on your own Mac: how strong a
part is in a given filament or resin, how air flows around it, and how much a
propeller pushes. Everything runs locally; nothing is uploaded.

## Pieces

```
SimLab.app (Tauri v2 window, macOS WebKit)
   └─ starts simlab-engine (one Rust process, localhost:7311)
        ├─ HTTP API + live events (/api, /events)      ← the web UI (web/)
        ├─ MCP server (/mcp, and `simlab-engine mcp`)   ← Claude Code / Claude Desktop
        ├─ SQLite store + files (~/Library/Application Support/SimLab)
        ├─ job queues: one GPU job (quick air preview), one CPU job (strength,
        │              OpenFOAM or CalculiX) and propeller runs, each one at a time
        ├─ crates/fea    stress test: quadratic tetrahedra (tet10), CPU, all cores
        ├─ crates/reference  accurate air tests: OpenFOAM v2606 in Docker (the default)
        ├─ crates/cfd    quick air previews: lattice Boltzmann on the GPU (Metal via wgpu)
        ├─ crates/prop   propeller performance: blade element momentum theory
        ├─ mesher/       Gmsh (official PyPI wheel, hash-locked by uv) for
        │                STEP/STL → surface patches and tet10 volume meshes
        └─ docker/       the simlab-reference:1 image: OpenFOAM v2606 + CalculiX
                         2.21, run in Colima/Docker (needed for accurate air
                         tests and CalculiX checks; strength and propeller
                         tests work without it)
```

Quitting the app stops the engine; nothing runs in the background. All data
lives in `~/Library/Application Support/SimLab/` and survives quitting,
updates and reinstalls.

## Crates

| Crate | Job |
|---|---|
| `crates/core` (`simlab-core`) | shared types: meshes, materials, study configs, result buffers, units. No heavy deps. |
| `crates/fea` (`simlab-fea`) | linear elastic FEA on tet10 meshes: anisotropic (print direction) materials, fixed supports, forces, pressures, motor loads, gravity/g-loads, spin (centrifugal), inertia relief for free-flying parts, Tsai-Hill failure index, safety factor, "breaks at" load. Sparse Cholesky (faer) with a PCG fallback when memory is short. CalculiX `.inp` export for reference runs. |
| `crates/reference` (`simlab-reference`) | the Accurate air test: plans and writes an OpenFOAM case (snappyHexMesh, simpleFoam k-ω SST, MRF for spinning parts), runs it in Docker with live progress, stops early once the forces settle, reads forces, surface pressure and slices back. |
| `crates/cfd` (`simlab-cfd`) | the Quick air preview: D3Q19 lattice Boltzmann on the GPU (wgpu → Metal) with LES turbulence, interpolated bounce-back for static parts, moving-wall bounce-back for spinning ones, momentum-exchange forces, surface pressure, flow slices. |
| `crates/prop` (`simlab-prop`) | reads blade chord and twist from the mesh at radial stations, then BEMT with Prandtl tip loss and low-Reynolds airfoil polars: thrust, torque, power, efficiency vs RPM and airspeed; also the spanwise load used by the prop stress test. |
| `crates/engine` (`simlab-engine`) | axum server, SQLite, job runner, mesher calls, system check, MCP. The binary. |
| `app/` | Tauri shell: opens a window on the engine. |

## Units

Inside every solver: SI (m, kg, s, N, Pa). Models arrive in millimetres by
default (STL has no units; the UI asks when a part looks too big or small).
The API speaks SI too, except where a field name says otherwise (`_mm`,
`_mpa`, `_rpm`, `_g` for grams-force).

## Storage

```
SimLab/
  simlab.db                      SQLite (WAL)
  projects/<project>/models/<model>/
      source.<stl|step|obj|3mf>   the file as uploaded
      surface.bin + surface.json  display mesh: positions f32×3, normals f32×3,
                                  indices u32×3, patch id u32 per triangle
      patches.json                surface groups the user clicks (faces, holes)
  runs/<run>/
      run.json                    config as run, summary, timings
      mesh.bin                    result surface (same layout as surface.bin)
      field.<name>.bin            f32 per surface vertex (stress, pressure, ...)
      slice.<name>.bin            flow slices: header + f32 grid
      log.txt
  runtime/venv                    uv-managed Python with Gmsh (hash-locked)
```

Tables: `projects`, `models`, `studies`, `runs`, `materials` (custom),
`motors` (custom). Studies keep their full config as JSON so a run is
reproducible and comparable.

## Study kinds

1. **Strength** (`strength`): material + print direction, supports (fixed
   patches or "free-flying" inertia relief), loads (force, pressure, motor
   thrust/torque at mount patches, g-load, spin). Results: von Mises, max
   principal, failure index and safety factor maps, displacement, where it
   breaks first and at what load or RPM.
2. **Wind tunnel** (`wind`): wind speed and direction, air, optional spin.
   Results: forces and moments (drag, lift, thrust, torque), drag
   coefficient, surface pressure, flow slices.
3. **Propeller** (`prop`): RPM range and airspeed. Results: thrust, torque,
   power and efficiency curves, spanwise loads.

Each run records which engine solved it: `reference` (wind tunnel:
Accurate, OpenFOAM, the default; strength: the CalculiX check) or `native`
(wind tunnel: Quick preview on the GPU; strength and propeller: SimLab's own
solvers, the default). OpenFOAM cases live in the run's folder when the data
folder is under the home folder (Colima only shares that), otherwise in
`~/Library/Caches/com.offgridevices.simlab/reference/<run>`; CalculiX
streams its deck and results through the container, so it needs no folder.

## Performance and memory

Settings hold a memory budget (default: half the Mac's RAM). Before a run the
engine estimates memory from the mesh/grid size and picks the finest quality
that fits; the UI shows the estimate. The wind tunnel runs on the GPU and
uses unified memory; the stress test uses every performance core.

## Validation

`validation/` holds cases with known answers (closed-form, NAFEMS, measured
propeller data). `cargo test` runs the quick ones; `scripts/validate.sh`
runs all of them and writes `docs/VALIDATION.md`.
