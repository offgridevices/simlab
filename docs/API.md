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
| GET/PUT | `/api/settings` | `{ memory_budget_gb, cpu_threads, default_units_mm, reference_engine_enabled }` |

`Check = { id, label, status: "ok"|"missing"|"error"|"working"|"optional", detail, fix?: { label, asks: string } }`
— `asks` is the plain-words consent question the UI shows before running a fix
(e.g. "Start Colima? It uses 8 GB of memory while it runs.").

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
| POST | `/api/projects/:id/models` | multipart: `file` (STL, STEP/STP, OBJ, 3MF), `name?`, `unit_mm?` (mm per file unit, default 1) → `Model` with `status: "processing"` |
| GET | `/api/models/:id` | `Model` |
| PATCH / DELETE | `/api/models/:id` | `{ name?, unit_mm?, patch_labels?: { [patchId]: string } }` |
| GET | `/api/models/:id/surface/:buffer` | `positions.f32` (xyz, model mm), `normals.f32`, `indices.u32`, `patch.u32` (one per triangle) |
| GET | `/api/models/:id/file` | the original upload |

```
Model = {
  id, project_id, name, file_name, format: "stl"|"step"|"obj"|"3mf",
  unit_mm, status: "processing"|"ready"|"failed", message,
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
| POST | `/api/studies/:id/estimate` | → `{ fits, memory_bytes, budget_bytes, size: { elements? , nodes?, cells? }, seconds, note }` |
| POST | `/api/studies/:id/runs` | `{ engine?: "native"|"reference" }` → `Run` (queued) |
| GET | `/api/runs/:id` | `Run` |
| POST | `/api/runs/:id/stop` | stops it |
| DELETE | `/api/runs/:id` | deletes it and its files |
| GET | `/api/runs/:id/mesh/:buffer` | result surface: `positions.f32` (model mm), `normals.f32`, `indices.u32` |
| GET | `/api/runs/:id/field/:name` | `f32` per result-surface vertex |
| GET | `/api/runs/:id/slice/:name` | `Slice` JSON |
| GET | `/api/runs/:id/slice/:name/:field` | `f32[nu*nv]`, row-major in v then u; `NaN` inside the part |
| GET | `/api/runs/:id/log` | text |

```
Study = { id, project_id, model_id, name, kind: "strength"|"wind"|"prop",
          config: StudyConfig, created_at, updated_at, runs: RunBrief[] }
RunBrief = { id, status, engine, progress, created_at, finished_at?, headline? }
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

Summaries (all also carry `headline`: one plain sentence, e.g. "Safe: 3.1× stronger than this load needs" or "Breaks at about 28,500 rpm, at the blade root"):

```
StrengthSummary = { kind: "strength", headline, verdict: "safe"|"marginal"|"breaks",
  safety_factor, max_von_mises_mpa, max_principal_mpa, max_displacement_mm,
  critical_point_mm, critical_patch?, breaks_at?: { load_factor, rpm?, text },
  mass_g, elements, nodes, dofs, solver, solve_seconds, mesh_seconds,
  reactions_n?: [x,y,z], warnings: string[] }
WindSummary = { kind: "wind", headline, force_n: [x,y,z], moment_nm: [x,y,z],
  drag_n, lift_n, side_n, cd?, ref_area_mm2, thrust_n?, torque_nm?, power_w?,
  reynolds, cells, steps, mlups, seconds, converged, warnings }
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
wait for a run, read summaries and field statistics, compare runs.

## Areas picked by shape (regions)

A strength config may carry `regions: [{ id, label, shape }]` for areas that are
not a patch of their own (a smooth propeller hub, "3 mm around where I
clicked"). `shape` is `{type:"sphere", center_mm, radius_mm}`,
`{type:"cylinder", axis, center_mm, radius_mm, half_length_mm?}` or
`{type:"box", min_mm, max_mm}`. Ids start at 1,000,000 and are listed in
`support.patches` or a load's `patches` like any patch id. Surface faces whose
centre lies inside the shape belong to the region.
