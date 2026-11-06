# SimLab engine HTTP API

The engine listens on `http://127.0.0.1:7311`. JSON everywhere except the
binary mesh/field downloads (raw little-endian arrays). Study configs are the
types in `crates/core/src/config.rs`, serialized with serde (tagged enums use
`"kind"` / `"type"` in snake_case). Times are ISO 8601 UTC strings.

Errors: HTTP 4xx/5xx with `{ "error": "plain words" }`.

## System

| Method | Path | Body / result |
|---|---|---|
| GET | `/api/health` | `{ version, install: "source"|"download", data_dir, machine: { cpu, cores, ram_bytes, gpu, gpu_memory_bytes }, checks: Check[] }` |
| POST | `/api/system/update` | → `{ message, url?, latest? }`. Built from source (`health.install == "source"`): pulls the newest SimLab and rebuilds (takes minutes; then restart). Downloaded (`"download"`): only asks GitHub for the newest release; `url` is where to download it when it is newer |
| POST | `/api/system/fix` | `{ id }` → starts the fix for a check (only ones with `fix` set); progress arrives as `check` events |
| GET/PUT | `/api/settings` | `{ memory_budget_gb, cpu_threads, default_units_mm, reference_engine_enabled }` (`cpu_threads`: cores for strength tests, OpenFOAM and CalculiX; `reference_engine_enabled`: offer the CalculiX double-check on strength results) |

`Check = { id, label, status: "ok"|"missing"|"error"|"working"|"optional", detail, fix?: { label, asks: string } }`
— `asks` is the plain-words consent question the UI shows before running a fix
(e.g. "Start Colima? It uses 8 GB of memory while it runs.").

Checks: `machine`, `gpu` (quick air previews), `mesher` (Gmsh), `docker`, `disk`.
`docker` is **needed for accurate air tests** (OpenFOAM) and CalculiX checks;
strength and propeller tests work without it. Its fix starts Colima (an
existing Colima keeps its own CPU and memory settings) or builds the
`simlab-reference:1` image. A Colima SimLab started is stopped again when the
engine quits; one that was already running is left alone. Reference
containers a crashed engine left behind are killed at start-up.

## Library

| Method | Path | Result |
|---|---|---|
| GET | `/api/materials` | `Material[]` (built-in + custom; custom have `custom: true`) |
| POST | `/api/materials` | `Material` → saved custom material |
| DELETE | `/api/materials/:id` | custom only |
| GET | `/api/motors` | `Motor[]` |
| POST / DELETE | `/api/motors`, `/api/motors/:id` | custom only |

## Projects and models

| Method | Path | Result |
|---|---|---|
| GET | `/api/projects` | `ProjectSummary[]` = `{ id, name, notes, created_at, updated_at, models, studies, runs, last_run?: RunBrief, thumbnail_model_id? }` |
| POST | `/api/projects` | `{ name, notes? }` → `Project` |
| GET | `/api/projects/:id` | `{ project, models: Model[], studies: Study[] }` |
| PATCH / DELETE | `/api/projects/:id` | `{ name?, notes? }` |
| POST | `/api/projects/:id/models` | multipart: `file` (STL, STEP/STP, OBJ, 3MF), `name?`, `unit_mm?` (mm per file unit, default 1) → `Model` with `status: "processing"` (or `"waiting"` when Gmsh is not set up yet: the file is kept and opens by itself after `POST /api/system/fix { id: "mesher" }`) |
| GET | `/api/models/:id` | `Model` |
| PATCH / DELETE | `/api/models/:id` | `{ name?, unit_mm?, patch_labels?: { [patchId]: string } }` |
| GET | `/api/models/:id/surface/:buffer` | `positions.f32` (xyz, model mm), `normals.f32`, `indices.u32`, `patch.u32` (one per triangle) |
| GET | `/api/models/:id/file` | the original upload |

```
Model = {
  id, project_id, name, file_name, format: "stl"|"step"|"obj"|"3mf",
  unit_mm, status: "processing"|"ready"|"failed"|"waiting", message,
  created_at,
  bbox_mm?: { min: [x,y,z], max: [x,y,z] }, size_mm?: [dx,dy,dz],
  volume_mm3?, area_mm2?, triangles?, watertight?,
  patches?: Patch[],
  suggestions?: { looks_like: "propeller"|"frame"|"part", prop?: { axis, center_mm, radius_mm, blades }, motor_mounts?: MountGuess[] }
}
Patch = { id, kind: "plane"|"cylinder"|"cone"|"sphere"|"other", area_mm2, center_mm,
          normal?: [x,y,z], axis?: [x,y,z], radius_mm?, label? }
MountGuess = { center_mm, axis, bolt_circle_mm, patches: number[] }   // a motor's 4 bolt holes
```

## Studies and runs

| Method | Path | Result |
|---|---|---|
| POST | `/api/projects/:id/studies` | `{ name, model_id, config: StudyConfig }` → `Study` |
| GET / PATCH / DELETE | `/api/studies/:id` | `Study`; PATCH `{ name?, config? }` |
| POST | `/api/studies/:id/estimate` | `{ engine? }` (optional body) → `Estimate` |
| POST | `/api/studies/:id/runs` | `{ engine? }` (optional body) → `Run` (queued). 409 when the engine needs Docker and it is not ready: `{ error, needs: Check }` (ask `needs.fix.asks`, run `POST /api/system/fix { id: needs.id }`, wait for its `check` event to turn `ok`, start again) |
| GET | `/api/runs/:id` | `Run` |
| POST | `/api/runs/:id/stop` | stops it |
| DELETE | `/api/runs/:id` | deletes it and its files |
| GET | `/api/runs/:id/mesh/:buffer` | result surface: `positions.f32` (model mm), `normals.f32`, `indices.u32` |
| GET | `/api/runs/:id/field/:name` | `f32` per result-surface vertex |
| GET | `/api/runs/:id/slice/:name` | `Slice` JSON |
| GET | `/api/runs/:id/slice/:name/:field` | `f32[nu*nv]`, row-major in v then u; `NaN` inside the part |
| GET | `/api/runs/:id/log` | text |

**Engines.** `engine` picks the solver:

| test | `"reference"` | `"native"` |
|---|---|---|
| wind tunnel | **Accurate (OpenFOAM)**, the default: OpenFOAM v2606 in Docker (snappyHexMesh, steady RANS k-ω SST, MRF for spinning parts) on the CPU queue with `cpu_threads` cores. Propeller thrust within ~3 %, torque ~7 %; sphere drag 0.2–1.3 %. ~4 / 10 / 26 min for a 3-inch prop at Draft / Normal / Fine. | **Quick preview (GPU)**: the lattice-Boltzmann tunnel. Still parts: drag within ~3 % on simple shapes at low Reynolds number, less certain above; spinning parts: a rough picture only, thrust can be 30–90 % off (E-v5 previews read 32–50 g against OpenFOAM's 345 g). Every preview's headline, `preview_note`, first warning and `accuracy` say so. |
| strength | **CalculiX check**: the same mesh, loads and supports solved by CalculiX 2.21 in Docker (geometrically nonlinear when a held part spins); the summary's `check` compares it with the latest SimLab run of the same setup. | SimLab's own solver, the default. |
| propeller | – | blade element model, the only engine |

A quick preview is always a preview: the engine picks the finest GPU grid
(up to what the quality asks) whose expected time is under 3 minutes and
under half the accurate run's, and says so in a warning when it had to go
coarser (`preview_cells_across` vs `quality_cells_across`). A custom grid
size (`{type: "custom", cells_across}`) is kept as given.

**Slender parts.** Both engines size a still part's grid from its largest
size *and* its thinner size across the wind (`thin_mm`: a 150 × 12 × 6 mm
arm broadside to the wind is 12 or 6 mm thick across it, not 150): the grid
is refined until that size has 16 / 24 / 32 cells across it (Draft /
Normal / Fine; summary and GPU estimate `cells_thin`), and the refinement
round the part reaches a few times that size instead of the whole length.
The refinement stops at the memory budget and at about what a compact part
of the same quality costs (GPU: at most 8× the quality's cells across;
OpenFOAM: at most 3 more surface levels, about 0.7 / 2 / 6 M planned cells),
and a warning (GPU estimates: `thin_warning`) says so when that leaves fewer
cells across than asked. A custom GPU grid is kept and warned about below
8 cells across.

An accurate run keeps its OpenFOAM case for ParaView (`case_dir`, open
`case.foam`; `case_bytes` on disk), trimmed to the reconstructed mesh, the
final time step, the dictionaries, `Allrun`, `case.json`, the logs and
`postProcessing/`. Deleting the run deletes it.

Accurate (OpenFOAM) runs of a still part pick the turbulence model by the
Reynolds number U·L/ν (L the part's largest size): laminar below 300, k-ω
SST to 5000, k-ω SST with Langtry–Menter transition above (summary
`turbulence`), all steady. A wind config with `duration` (flow passes over
the part) asks for a time-accurate OpenFOAM run instead (summary `solver`,
`averaged_flow_passes`, `drag_fluctuation_n`); spinning parts are always
steady (MRF). Each engine labels its OpenFOAM/CalculiX containers with its
data folder (`simlab.owner`), and at start and exit stops only its own.

A spinning wind config may leave out `spin.center_mm`: the engine then uses
the hub the model analysis found (`suggestions.prop.center_mm`), not the
bounding-box centre (on a 3-blade prop the box centre is several mm off the
hub). A centre that is given is kept; the run warns when it is over 1 mm
from the detected hub.

Queues: one GPU job at a time (quick previews); one CPU job at a time
(strength tests, OpenFOAM, CalculiX), so two never fight over the cores;
propeller runs (under a second) have their own queue.

```
Estimate = { fits, memory_bytes, budget_bytes, size: { elements?, nodes?, dofs?, cells? }, seconds, note,
             engine?, label?, available?, docker_ready?,
             engines?: { reference: Estimate, native: Estimate } }   // wind tunnel: both, for the choice
```
For OpenFOAM `budget_bytes` is Docker's memory and `cells` the expected final
mesh; time and memory are calibrated on the validation runs (see
`crates/engine/src/wind.rs`). For the GPU it is the memory budget setting.

```
Study = { id, project_id, model_id, name, kind: "strength"|"wind"|"prop",
          config: StudyConfig, created_at, updated_at, runs: RunBrief[] }
RunBrief = { id, status, engine: "native"|"reference", progress, created_at, finished_at?, headline?, short?, verdict? }
Run = RunBrief & {
  study_id, message, started_at?, config: StudyConfig (as run), material?: Material,
  summary?: StrengthSummary | WindSummary | PropSummary,
  fields: { name, label, unit, min, max, scale: "linear"|"log", diverging?: bool }[],
  slices: { name, label }[],
  metrics: { [name]: [step, value][] }, error?
}
Slice = { name, label, origin_mm, u_mm, v_mm (axis vectors whose length is the slice size),
          nu, nv, fields: { name, label, unit, min, max }[] }
```

Wind results (both engines): the result surface is the model's surface;
fields `pressure` (Pa above ambient, per vertex; for display, forces come
from the solver) and, for the GPU, `cp`. Slices: still parts `side` (wind ×
lift) and `top` (wind × side); spinning parts `axial` (through the spin
axis) and `disc` (the rotor plane); fields `speed` (m/s) and `pressure`
(Pa), plus `u`/`v` from the GPU. Live metrics while running: OpenFOAM
`residual_U`, `residual_p`, `residual_k`, `residual_omega` and `thrust_n`,
`torque_nm` (spinning) or `drag_n`, `lift_n`, `side_n`; the GPU `thrust_n`,
`torque_nm` or `drag_n`, `lift_n`.

Summaries (all also carry `headline`: one plain sentence, e.g. "Safe: 3.1× stronger than this load needs" or "Breaks at about 28,500 rpm, at the blade root"):

```
StrengthSummary = { kind: "strength", headline, verdict: "safe"|"marginal"|"breaks",
  safety_factor, max_von_mises_mpa, max_principal_mpa, max_displacement_mm,
  critical_point_mm, critical_patch?, breaks_at?: { load_factor, rpm?, text },
  mass_g, elements, nodes, dofs, solver, solve_seconds, mesh_seconds,
  reactions_n?: [x,y,z], warnings: string[], accuracy,
  engine?, equilibrium_error?,   // CalculiX checks: how far CalculiX's own nodal forces miss the loads (relative; above 0.02 the check is not trusted)
  check?: { native_run_id, text, agrees, same_mesh,   // agrees: every difference within 0.1% and CalculiX balanced
            native: {max_von_mises_mpa, safety_factor, max_displacement_mm, elements},
            calculix: {...}, diff_pct: {max_von_mises, safety_factor, max_displacement} } }   // CalculiX checks
WindSummary = { kind: "wind", engine: "reference"|"native", method: "Accurate (OpenFOAM)"|"Quick preview (GPU)",
  headline, short, force_n: [x,y,z], moment_nm: [x,y,z],
  drag_n, lift_n, side_n, cd?, ref_area_mm2, thrust_n?, thrust_g?, torque_nm?, power_w?, rpm?,
  reynolds, cells, steps, seconds, converged, warnings, accuracy,
  preview_note?,                                  // quick previews: how rough, in one sentence
  mlups?,                                         // GPU
  preview_cells_across?, quality_cells_across?,   // GPU
  thin_mm?, cells_thin?,                          // still parts: thinner size across the wind, grid cells across it
  iterations?, averaged_over?, turbulence?, y_plus?: [min, avg, max], moment_center_mm?,
  solver?,                  // OpenFOAM: "steady (simpleFoam)" or "time-accurate (pimpleFoam), forces averaged over time"
  averaged_flow_passes?,    // OpenFOAM time-accurate runs: averaging time in flow passes over the part (L/U)
  drag_fluctuation_n?,      // OpenFOAM time-accurate runs: standard deviation of the drag over that time (N)
  case_dir?, case_bytes? }                        // OpenFOAM
Accuracy = { method, checked_against, typical_error }   // "How sure" in the UI
PropSummary = { kind: "prop", headline, blades, radius_mm, hub_radius_mm, rotation: "cw"|"ccw",
  geometry: { r_mm, chord_mm, pitch_deg }[],
  points: { rpm, airspeed_m_s, thrust_n, thrust_g, torque_nm, power_w, ct, cp, efficiency? }[],
  warnings }
```

## Live events

`GET /api/events` — Server-Sent Events, one JSON object per `data:` line:

```
{ type: "run", run: RunBrief & { study_id, project_id, message } }
{ type: "metric", run_id, name, step, value }
{ type: "model", model: Model }
{ type: "project", project_id }          // something in it changed (an AI may be working)
{ type: "check", check: Check }
```

## MCP

`POST /mcp` (Streamable HTTP, JSON responses) and `simlab-engine mcp` (stdio).
Tools mirror the API: list/create projects, import a model from a file path,
describe a model's patches, list materials and motors, create and run studies,
wait for a run, read summaries and field statistics, compare runs. The tool
descriptions tell Claude that wind tunnel runs default to Accurate (OpenFOAM)
and that `engine: "native"` is only a quick preview.

## Areas picked by shape (regions)

A strength config may carry `regions: [{ id, label, shape }]` for areas that are
not a patch of their own (a smooth propeller hub, "3 mm around where I
clicked"). `shape` is `{type:"sphere", center_mm, radius_mm}`,
`{type:"cylinder", axis, center_mm, radius_mm, half_length_mm?}` or
`{type:"box", min_mm, max_mm}`. Ids start at 1,000,000 and are listed in
`support.patches` or a load's `patches` like any patch id. Surface faces whose
centre lies inside the shape belong to the region.
