// Demo mode's fake engine: the HTTP API of docs/API.md and its live events,
// answered in the browser from memory. The real API client talks to it
// through a fetch function, so every screen runs the same code it runs
// against the engine.
import type { Transport } from "../lib/api";
import type {
  Check,
  Engine,
  EngineEvent,
  Estimate,
  Material,
  Model,
  Motor,
  Project,
  ProjectSummary,
  Run,
  RunBrief,
  Settings,
  Study,
  StudyConfig,
  Vec3,
  WindSummary,
} from "../lib/types";
import { makeFrame, makeMount, makePropeller, type SampleMesh } from "../lib/samples";
import { CHECKS, MATERIALS, MOTORS, RAM, SETTINGS, health } from "./library";
import { prop, strength, volume, wind, type FieldSet, type SliceData } from "./physics";

// ---------------------------------------------------------------- state

interface ModelRec {
  model: Model;
  make: () => SampleMesh;
  base?: SampleMesh;
  scaled?: { unit: number; mesh: SampleMesh };
}
interface RunRec {
  run: Run;
  project_id: string;
  fields?: FieldSet;
  slices?: SliceData[];
  /** the full metric series; revealed as the run moves on */
  series: Record<string, [number, number][]>;
  log: string[];
}

const projects = new Map<string, Project>();
const models = new Map<string, ModelRec>();
const studies = new Map<string, Study>();
const runs = new Map<string, RunRec>();
const materials: Material[] = MATERIALS.map((m) => ({ ...m }));
const motors: Motor[] = MOTORS.map((m) => ({ ...m }));
let settings: Settings = { ...SETTINGS };
const checks: Check[] = CHECKS.map((c) => ({ ...c }));
const listeners = new Set<(e: EngineEvent) => void>();

let seq = 100;
const id = (p: string) => `${p}_${(seq++).toString(36)}`;
const now = () => new Date().toISOString();
const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const emit = (e: EngineEvent) => listeners.forEach((l) => l(e));

// ---------------------------------------------------------------- meshes

function scaleSample(m: SampleMesh, k: number): SampleMesh {
  if (k === 1) return m;
  const positions = new Float32Array(m.positions.length);
  for (let i = 0; i < positions.length; i++) positions[i] = m.positions[i]! * k;
  const s = (v: Vec3): Vec3 => [v[0] * k, v[1] * k, v[2] * k];
  return {
    ...m,
    positions,
    patches: m.patches.map((p) => ({ ...p, area_mm2: p.area_mm2 * k * k, center_mm: s(p.center_mm), ...(p.radius_mm ? { radius_mm: p.radius_mm * k } : {}) })),
    suggestions: {
      ...m.suggestions,
      ...(m.suggestions.prop ? { prop: { ...m.suggestions.prop, center_mm: s(m.suggestions.prop.center_mm), radius_mm: m.suggestions.prop.radius_mm * k } } : {}),
      ...(m.suggestions.motor_mounts ? { motor_mounts: m.suggestions.motor_mounts.map((g) => ({ ...g, center_mm: s(g.center_mm), bolt_circle_mm: g.bolt_circle_mm * k })) } : {}),
    },
    inside: (p) => m.inside([p[0] / k, p[1] / k, p[2] / k]),
  };
}

function meshOf(rec: ModelRec): SampleMesh {
  rec.base ??= rec.make();
  const unit = rec.model.unit_mm;
  if (!rec.scaled || rec.scaled.unit !== unit) rec.scaled = { unit, mesh: scaleSample(rec.base, unit) };
  return rec.scaled.mesh;
}

/** fill in what the engine works out when it reads a model */
function describe(rec: ModelRec) {
  const m = meshOf(rec);
  const min: Vec3 = [Infinity, Infinity, Infinity];
  const max: Vec3 = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < m.positions.length; i += 3)
    for (let k = 0; k < 3; k++) {
      min[k] = Math.min(min[k]!, m.positions[i + k]!);
      max[k] = Math.max(max[k]!, m.positions[i + k]!);
    }
  const r = (v: number) => Number(v.toFixed(3));
  rec.model = {
    ...rec.model,
    status: "ready",
    message: "",
    bbox_mm: { min: min.map(r) as Vec3, max: max.map(r) as Vec3 },
    size_mm: [0, 1, 2].map((k) => r(max[k]! - min[k]!)) as Vec3,
    volume_mm3: volume(m),
    area_mm2: m.patches.reduce((s, p) => s + p.area_mm2, 0),
    triangles: m.indices.length / 3,
    watertight: true,
    patches: m.patches,
    suggestions: m.suggestions,
  };
}

function pickMaker(fileName: string): () => SampleMesh {
  const f = fileName.toLowerCase();
  if (/dense|2m|scan/.test(f)) return () => makePropeller({ stations: 1200, around: 280, hubSegments: 256, onePiece: true });
  if (/prop/.test(f)) return () => makePropeller();
  if (/mount/.test(f)) return makeMount;
  return makeFrame;
}

// ---------------------------------------------------------------- runs

const DURATION: Record<StudyConfig["kind"], number> = { strength: 9000, wind: 12000, prop: 3500 };

function brief(r: Run): RunBrief {
  const verdict = r.summary?.kind === "strength" && r.status === "done" ? r.summary.verdict : null;
  return { id: r.id, status: r.status, engine: r.engine, progress: r.progress, created_at: r.created_at, finished_at: r.finished_at ?? null, headline: r.headline ?? null, verdict };
}

function solve(rec: RunRec) {
  const study = studies.get(rec.run.study_id)!;
  const mrec = models.get(study.model_id)!;
  const mesh = meshOf(mrec);
  const cfg = rec.run.config;
  if (cfg.kind === "strength") {
    const mat = materials.find((m) => m.id === cfg.material_id) ?? materials[0]!;
    const { fields, summary: own } = strength(mesh, mrec.model, cfg, mat);
    let summary = own;
    if (rec.run.engine === "reference") {
      // CalculiX on the same mesh lands within a fraction of a percent
      const ccx = { ...own, engine: "reference" as const, max_von_mises_mpa: own.max_von_mises_mpa * 1.002, safety_factor: own.safety_factor / 1.001, solver: "CalculiX 2.21: SPOOLES direct solver, linear" };
      const native = [...runs.values()].filter((r) => r.run.study_id === rec.run.study_id && r.run.engine === "native" && r.run.status === "done" && r.run.summary?.kind === "strength").at(-1);
      const ns = native?.run.summary?.kind === "strength" ? native.run.summary : null;
      const pct = (a: number, b: number) => (100 * (a - b)) / b;
      summary = ns
        ? {
            ...ccx,
            check: {
              native_run_id: native!.run.id,
              text: `CalculiX agrees with SimLab: peak stress ${pct(ccx.max_von_mises_mpa, ns.max_von_mises_mpa).toFixed(1)}%, safety factor ${pct(ccx.safety_factor, ns.safety_factor).toFixed(1)}%.`,
              agrees: true,
              native: { max_von_mises_mpa: ns.max_von_mises_mpa, safety_factor: ns.safety_factor, max_displacement_mm: ns.max_displacement_mm },
              calculix: { max_von_mises_mpa: ccx.max_von_mises_mpa, safety_factor: ccx.safety_factor, max_displacement_mm: ccx.max_displacement_mm },
              diff_pct: { max_von_mises: pct(ccx.max_von_mises_mpa, ns.max_von_mises_mpa), safety_factor: pct(ccx.safety_factor, ns.safety_factor), max_displacement: 0 },
            },
          }
        : ccx;
    }
    rec.fields = fields;
    rec.run.material = mat;
    rec.run.summary = summary;
    rec.run.fields = fields.infos;
    rec.series = { residual: decay(70, 1, 1e-9) };
  } else if (cfg.kind === "wind") {
    const { fields, summary: base, slices } = wind(mesh, mrec.model, cfg);
    const spin = !!(cfg.spin && cfg.spin.rpm > 0);
    const preview = rec.run.engine === "native";
    const note = spin ? "Rough picture only: thrust can be 30–90% off (quick previews of a 3-inch prop read 32–50 g where OpenFOAM gets 345 g); use Accurate for numbers." : "Quick preview: drag on rounded parts came within about 7% (a sphere from Re 20 to 100,000), but sharp-edged parts read low on a preview's coarse grid: about 12% on a flat plate, 20–30% on a cube or a thin bar. Lift, side force and anything that depends on thin boundary layers (airfoils, wings, small fins) are less certain, and can be off by 20% or more. Use the OpenFOAM test for numbers you will rely on.";
    // the preview reads differently from the accurate answer, as the real GPU tunnel does on propellers
    const k = preview && spin ? 0.62 : 1;
    const summary: WindSummary = preview
      ? {
          ...base,
          thrust_n: base.thrust_n ? base.thrust_n * k : base.thrust_n,
          headline: `${spin ? "Rough preview:" : "Quick preview:"} ${base.headline}`,
          engine: "native",
          method: "Quick preview (GPU)",
          preview_cells_across: 57,
          quality_cells_across: { draft: 64, normal: 128, fine: 192, custom: 128 }[cfg.quality.type],
          preview_note: note,
          warnings: [note, ...base.warnings],
          accuracy: { method: "SimLab's GPU wind tunnel: lattice Boltzmann (D3Q19, LES)", checked_against: spin ? "OpenFOAM runs of two 3-inch props" : "Sphere drag from Reynolds number 20 to 10,000", typical_error: note },
        }
      : {
          ...base,
          engine: "reference",
          method: "Accurate (OpenFOAM)",
          mlups: null,
          steps: spin ? 774 : 1100,
          iterations: spin ? 774 : 1100,
          cells: Math.round(base.cells / 4.3),
          turbulence: "k-omega SST",
          seconds: spin ? 560 : 420,
          case_dir: `~/Library/Application Support/SimLab/runs/${rec.run.id}/openfoam`,
          case_bytes: spin ? 253_000_000 : 180_000_000,
          accuracy: {
            method: "OpenFOAM v2606: snappyHexMesh, steady RANS with k-ω SST, the spin as a moving reference frame",
            checked_against: spin ? "Your 2026 OpenFOAM propeller study (E-v5, P-v2)" : "Sphere drag at Reynolds number 100 and 1000",
            typical_error: spin ? "Thrust within about 3% and torque within about 7%; within 0.6% of your earlier OpenFOAM runs of the same props." : "Drag within about 1% on simple shapes (sphere 0.2–1.3%).",
          },
        };
    rec.fields = fields;
    rec.slices = slices;
    rec.run.summary = summary;
    rec.run.fields = fields.infos;
    rec.run.slices = slices.map((s) => ({ name: s.slice.name, label: s.slice.label }));
    const target = summary.thrust_n ?? summary.drag_n;
    const name = summary.thrust_n ? "thrust_n" : "drag_n";
    rec.series = {
      [name]: Array.from({ length: 80 }, (_, i) => [i * 150, target * (1 - Math.exp(-i / 9)) * (1 + 0.18 * Math.exp(-i / 22) * Math.sin(i / 2.2)) + target * 0.01 * Math.sin(i * 1.7)] as [number, number]),
      residual: decay(80, 0.3, 2e-5),
    };
  } else {
    const summary = prop(mrec.model, cfg);
    rec.run.summary = summary;
    rec.run.fields = [];
    rec.series = { thrust_g: summary.points.filter((p) => p.airspeed_m_s === (cfg.airspeeds_m_s?.[0] ?? 0)).map((p, i) => [i, p.thrust_g] as [number, number]) };
  }
}

function decay(n: number, from: number, to: number): [number, number][] {
  return Array.from({ length: n }, (_, i) => [i * 10, from * (to / from) ** (i / (n - 1)) * (1 + 0.35 * Math.abs(Math.sin(i * 1.3)))] as [number, number]);
}

const PHASES: Record<StudyConfig["kind"], [number, string][]> = {
  strength: [
    [0, "Getting the part ready"],
    [0.08, "Cutting the part into small pieces (meshing)"],
    [0.3, "Working out how every piece pushes on its neighbours"],
    [0.8, "Finding where it is weakest"],
    [0.95, "Saving the results"],
  ],
  wind: [
    [0, "Building the wind tunnel around the part"],
    [0.1, "Letting the air settle into a steady flow"],
    [0.85, "Measuring the forces"],
    [0.95, "Saving the flow pictures"],
  ],
  prop: [
    [0, "Reading the blade shape"],
    [0.2, "Working out the push at each speed"],
    [0.9, "Saving the curves"],
  ],
};

let ticking: ReturnType<typeof setInterval> | null = null;
function startTicking() {
  if (ticking) return;
  ticking = setInterval(tick, 250);
}

function lane(r: RunRec) {
  return r.run.config.kind === "wind" ? "gpu" : "cpu";
}

function tick() {
  const all = [...runs.values()];
  const active = all.filter((r) => r.run.status === "running" || r.run.status === "queued");
  if (!active.length) {
    clearInterval(ticking!);
    ticking = null;
    return;
  }
  for (const l of ["cpu", "gpu"]) {
    const mine = active.filter((r) => lane(r) === l);
    let cur = mine.find((r) => r.run.status === "running");
    if (!cur) {
      cur = mine.filter((r) => r.run.status === "queued").sort((a, b) => a.run.created_at.localeCompare(b.run.created_at))[0];
      if (!cur) continue;
      cur.run.status = "running";
      cur.run.started_at = now();
      solve(cur);
      cur.log.push(`${now()} started (${cur.run.engine})`);
    }
    advance(cur);
  }
}

function advance(rec: RunRec) {
  const r = rec.run;
  const kind = r.config.kind;
  const before = r.progress;
  r.progress = Math.min(1, r.progress + 250 / DURATION[kind]);
  r.message = [...PHASES[kind]].reverse().find(([p]) => r.progress >= p)?.[1] ?? "";
  if (kind === "strength" && r.progress > 0.3 && r.progress < 0.8) r.message = `Solving: ${Math.round(((r.progress - 0.3) / 0.5) * 100)}% done`;
  for (const [name, s] of Object.entries(rec.series)) {
    const from = Math.floor(before * s.length);
    const to = Math.floor(r.progress * s.length);
    const shown = (r.metrics[name] ??= []);
    for (let i = from; i < to; i++) {
      const [step, value] = s[i]!;
      shown.push([step, value]);
      emit({ type: "metric", run_id: r.id, name, step, value });
    }
  }
  if (r.progress >= 1) {
    r.status = "done";
    r.finished_at = now();
    r.message = "Finished";
    r.headline = r.summary?.headline ?? null;
    r.metrics = Object.fromEntries(Object.entries(rec.series).map(([k, v]) => [k, v.slice()]));
    rec.log.push(`${now()} finished: ${r.headline}`);
  }
  emitRun(rec);
}

function emitRun(rec: RunRec) {
  emit({ type: "run", run: { ...brief(rec.run), study_id: rec.run.study_id, project_id: rec.project_id, message: rec.run.message } });
  const st = studies.get(rec.run.study_id);
  if (st) {
    st.runs = st.runs.map((b) => (b.id === rec.run.id ? brief(rec.run) : b));
    st.updated_at = now();
  }
  const p = projects.get(rec.project_id);
  if (p) p.updated_at = now();
}

function newRun(study: Study, opts: { done?: boolean; at?: string; engine?: Run["engine"] } = {}): RunRec {
  const run: Run = {
    id: id("run"),
    study_id: study.id,
    status: "queued",
    engine: opts.engine ?? "native",
    progress: 0,
    created_at: opts.at ?? now(),
    finished_at: null,
    headline: null,
    message: "Waiting for its turn",
    started_at: null,
    config: structuredClone(study.config),
    material: null,
    summary: null,
    fields: [],
    slices: [],
    metrics: {},
    error: null,
  };
  const rec: RunRec = { run, project_id: study.project_id, series: {}, log: [`${run.created_at} queued`] };
  runs.set(run.id, rec);
  if (opts.done) {
    solve(rec);
    run.status = "done";
    run.progress = 1;
    run.started_at = run.created_at;
    run.finished_at = new Date(Date.parse(run.created_at) + DURATION[study.kind]).toISOString();
    run.headline = run.summary?.headline ?? null;
    run.message = "Finished";
    run.metrics = Object.fromEntries(Object.entries(rec.series).map(([k, v]) => [k, v.slice()]));
    rec.log.push(`${run.finished_at} finished: ${run.headline}`);
  }
  study.runs.push(brief(run));
  return rec;
}

// ---------------------------------------------------------------- seed

function addProject(name: string, notes: string, at: number): Project {
  const p: Project = { id: id("prj"), name, notes, created_at: minutesAgo(at + 60 * 24), updated_at: minutesAgo(at) };
  projects.set(p.id, p);
  return p;
}
function addModel(p: Project, name: string, file: string, make: () => SampleMesh, unit = 1, ready = true): ModelRec {
  const ext = (file.split(".").pop() ?? "stl").toLowerCase() as Model["format"];
  const rec: ModelRec = {
    model: { id: id("mdl"), project_id: p.id, name, file_name: file, format: ["stl", "step", "obj", "3mf"].includes(ext) ? ext : "stl", unit_mm: unit, status: "processing", message: "Reading the file", created_at: p.created_at },
    make,
  };
  models.set(rec.model.id, rec);
  if (ready) describe(rec);
  return rec;
}
function addStudy(p: Project, m: ModelRec, name: string, config: StudyConfig, at: string): Study {
  const s: Study = { id: id("std"), project_id: p.id, model_id: m.model.id, name, kind: config.kind, config, created_at: at, updated_at: at, runs: [] };
  studies.set(s.id, s);
  return s;
}

let seeded = false;
function seed() {
  if (seeded) return;
  seeded = true;
  const frameP = addProject("5-inch quad frame", "Freestyle frame, 6 mm arms.", 35);
  const frame = addModel(frameP, "Frame v3", "quad-frame-v3.step", makeFrame);
  const mounts = frame.model.suggestions?.motor_mounts ?? [];
  const motorLoads = (thrustG: number, massG: number) =>
    mounts.map((g, i) => ({ type: "motor" as const, patches: g.patches, thrust_n: (thrustG / 1000) * 9.80665, torque_nm: 0.16, axis: [0, 0, 1] as Vec3, motor_mass_g: massG, label: `Motor ${i + 1}` }));
  const stack = (frame.model.patches ?? []).filter((p) => /stack hole/i.test(p.label ?? "")).map((p) => p.id);
  const s1 = addStudy(frameP, frame, "Full-throttle punch-out", { kind: "strength", material_id: "pa12-cf", print_up: [0, 0, 1], support: { type: "free_flying" }, loads: motorLoads(1450, 33), quality: { type: "normal" } }, minutesAgo(200));
  newRun(s1, { done: true, at: minutesAgo(190) });
  newRun(s1, { done: true, at: minutesAgo(180), engine: "reference" });
  const s2 = addStudy(
    frameP,
    frame,
    "Crash landing, 50 g",
    { kind: "strength", material_id: "pla", print_up: [0, 0, 1], support: { type: "fixed", patches: stack }, loads: [...motorLoads(0, 33), { type: "acceleration", g: [0, 0, -50], label: "Crash" }], quality: { type: "normal" } },
    minutesAgo(120),
  );
  newRun(s2, { done: true, at: minutesAgo(110) });
  const s3 = addStudy(frameP, frame, "Forward flight, 20 m/s", { kind: "wind", wind_dir: [-1, 0, 0], speed_m_s: 20, spin: null, quality: { type: "normal" } }, minutesAgo(60));
  newRun(s3, { done: true, at: minutesAgo(35), engine: "reference" });

  const propP = addProject("Tri-blade 5-inch prop", "Resin print, testing how fast it can spin.", 300);
  const pm = addModel(propP, "Prop 5143", "prop-5143.stl", () => makePropeller());
  const pg = pm.model.suggestions!.prop!;
  const s4 = addStudy(propP, pm, "Thrust sweep", { kind: "prop", axis: pg.axis, center_mm: pg.center_mm, rpm_min: 5000, rpm_max: 32000, rpm_steps: 12, airspeeds_m_s: [0, 10, 20], blades: pg.blades }, minutesAgo(420));
  newRun(s4, { done: true, at: minutesAgo(410) });
  const s4b = addStudy(propP, pm, "Thrust sweep, as a 2-blade", { kind: "prop", axis: pg.axis, center_mm: pg.center_mm, rpm_min: 5000, rpm_max: 32000, rpm_steps: 12, airspeeds_m_s: [0, 10], blades: 2 }, minutesAgo(380));
  newRun(s4b, { done: true, at: minutesAgo(370) });
  const s4c = addStudy(propP, pm, "Hover at 32,000 rpm", { kind: "wind", wind_dir: [0, 0, -1], speed_m_s: 0, spin: { rpm: 32000, axis: pg.axis, center_mm: pg.center_mm }, quality: { type: "draft" } }, minutesAgo(360));
  newRun(s4c, { done: true, at: minutesAgo(355), engine: "native" });
  newRun(s4c, { done: true, at: minutesAgo(340), engine: "reference" });
  const hub = (pm.model.patches ?? []).filter((p) => /shaft hole|bottom face/i.test(p.label ?? "")).map((p) => p.id);
  const s5 = addStudy(
    propP,
    pm,
    "Spin test at 32,000 rpm",
    {
      kind: "strength",
      material_id: "resin-standard",
      print_up: [0, 0, 1],
      support: { type: "fixed", patches: hub },
      loads: [
        { type: "spin", rpm: 32000, axis: pg.axis, center_mm: pg.center_mm, label: "Spin" },
        { type: "prop_aero", rpm: 32000, airspeed_m_s: 0, axis: pg.axis, center_mm: pg.center_mm, label: "Air load" },
      ],
      quality: { type: "normal" },
    },
    minutesAgo(320),
  );
  newRun(s5, { done: true, at: minutesAgo(300) });

  const mountP = addProject("Motor mount", "Drawn in inches by mistake.", 60 * 26);
  addModel(mountP, "Mount plate", "motor-mount.stl", makeMount);

  const denseP = addProject("Prop scan (one smooth surface)", "A 2-million-triangle scan that comes in as one face: hold it with the brush or Hold the hub.", 60 * 24 * 3);
  const dense = addModel(denseP, "Scan, full detail", "prop-scan-dense.stl", () => makePropeller({ stations: 1200, around: 280, hubSegments: 256, onePiece: true }), 1, false);
  // described lazily: it takes a moment to make
  dense.model = { ...dense.model, status: "ready", message: "", triangles: 3 * 1199 * 280 * 2 + 3 * 280 + 4 * 256 * 2, size_mm: [127, 127, 14], bbox_mm: { min: [-63.5, -63.5, -7], max: [63.5, 63.5, 7] } };
  (dense as ModelRec & { lazy?: boolean }).lazy = true;
}

function ensureDescribed(rec: ModelRec) {
  const lazy = rec as ModelRec & { lazy?: boolean };
  if (lazy.lazy) {
    lazy.lazy = false;
    describe(rec);
  }
}

// ---------------------------------------------------------------- HTTP

class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });
const binary = (a: ArrayBufferView) => new Response(a.buffer.slice(a.byteOffset, a.byteOffset + a.byteLength) as ArrayBuffer, { headers: { "content-type": "application/octet-stream" } });
const must = <T,>(v: T | undefined, what: string): T => {
  if (v === undefined) throw new HttpError(404, `No such ${what}`);
  return v;
};

function summaryOf(p: Project): ProjectSummary {
  const ms = [...models.values()].filter((m) => m.model.project_id === p.id);
  const ss = [...studies.values()].filter((s) => s.project_id === p.id);
  const rs = ss.flatMap((s) => s.runs).sort((a, b) => b.created_at.localeCompare(a.created_at));
  return { ...p, models: ms.length, studies: ss.length, runs: rs.length, last_run: rs[0] ?? null, thumbnail_model_id: ms[0]?.model.id ?? null };
}

function estimate(study: Study, engine?: Engine): Estimate {
  if (study.config.kind !== "wind") return estimateOne(study, engine ?? "native");
  const docker = checks.find((c) => c.id === "docker");
  const reference = { ...estimateOne(study, "reference"), docker_ready: docker?.status === "ok" };
  const native = estimateOne(study, "native");
  return { ...(engine === "native" ? native : reference), engines: { reference, native } };
}

function estimateOne(study: Study, engine: Engine): Estimate {
  const m = models.get(study.model_id);
  const budget = settings.memory_budget_gb * 1024 ** 3;
  const c = study.config;
  let mem: number, secs: number, size: Estimate["size"];
  if (c.kind === "strength") {
    const el = { draft: 60_000, normal: 180_000, fine: 520_000, custom: Math.round(4e8 / Math.max(0.2, c.quality.type === "custom" ? c.quality.size_mm : 1) ** 3 / 1e3) }[c.quality.type];
    const nodes = Math.round(el * 1.55);
    mem = nodes * 3 * 300 * 8 * (m && (m.model.triangles ?? 0) > 500_000 ? 3 : 1);
    secs = el / 9000;
    size = { elements: el, nodes };
  } else if (c.kind === "wind" && engine === "reference") {
    // OpenFOAM: about 2 M cells at Normal, 10 minutes for a propeller
    const cells = { draft: 590_000, normal: 1_860_000, fine: 2_310_000, custom: 1_860_000 }[c.quality.type];
    mem = 0.4e9 + 1700 * cells;
    secs = 30 + cells * (c.quality.type === "fine" ? 6.6e-4 : c.quality.type === "draft" ? 3.8e-4 : 3.0e-4);
    size = { cells };
    const docker = 16 * 1024 ** 3;
    return { fits: mem <= docker, memory_bytes: mem, budget_bytes: docker, size, seconds: secs, note: `About ${cells.toLocaleString("en-US")} cells, 12 cores in Docker`, engine, label: "Accurate (OpenFOAM)", available: true };
  } else if (c.kind === "wind") {
    const cells = { draft: 2_000_000, normal: 8_000_000, fine: 24_000_000, custom: (c.quality.type === "custom" ? c.quality.cells_across : 200) ** 3 / 4 }[c.quality.type];
    mem = cells * 19 * 4 * 2 + 2e8;
    secs = (cells * 12_000) / 2.4e9;
    size = { cells };
  } else {
    mem = 5e7;
    secs = 2;
    size = {};
  }
  const fits = mem <= budget;
  return {
    fits,
    memory_bytes: mem,
    budget_bytes: budget,
    size,
    seconds: secs,
    note: fits ? "" : "This needs more memory than your budget. Try Draft, or raise the budget in Settings.",
    engine,
    label: c.kind === "wind" ? "Quick preview (GPU)" : engine === "reference" ? "CalculiX check" : undefined,
    available: true,
  };
}

async function handle(method: string, path: string, body: unknown): Promise<Response> {
  seed();
  const seg = path.replace(/^\/api\//, "").split("/").map(decodeURIComponent);
  const [a, b, c, d, e] = seg;

  if (a === "health" && method === "GET") return json(health(checks));
  if (a === "system" && b === "update" && method === "POST") {
    await new Promise((r) => setTimeout(r, 600));
    return json({ message: "SimLab 0.2.0 is out (you have 0.1.0-demo). Download it, then drag it into Applications to replace this one. Your projects stay.", url: "https://github.com/offgridevices/simlab/releases/latest", latest: "0.2.0" });
  }
  if (a === "system" && b === "fix" && method === "POST") {
    const chk = must(checks.find((x) => x.id === (body as { id: string }).id), "check");
    if (!chk.fix) throw new HttpError(400, "This check has nothing to fix");
    Object.assign(chk, { status: "working", detail: "Downloading… 0%" });
    emit({ type: "check", check: { ...chk } });
    let pct = 0;
    const t = setInterval(() => {
      pct += 20;
      if (pct >= 100) {
        clearInterval(t);
        Object.assign(chk, chk.id === "docker" ? { status: "ok", label: "Accurate air tests (OpenFOAM + CalculiX)", detail: "Ready in Docker · 8 cores, 8 GB" } : { status: "ok", detail: "Ready" });
        delete chk.fix;
      } else chk.detail = `Downloading… ${pct}%`;
      emit({ type: "check", check: { ...chk } });
    }, 700);
    return json({ ok: true });
  }
  if (a === "settings") {
    if (method === "GET") return json(settings);
    if (method === "PUT") {
      settings = { ...settings, ...(body as Settings) };
      return json(settings);
    }
  }
  if (a === "materials") {
    if (!b && method === "GET") return json(materials);
    if (!b && method === "POST") {
      const m = { ...(body as Material), custom: true };
      if (!m.name?.trim()) throw new HttpError(400, "Give the material a name");
      m.id ||= id("mat");
      const i = materials.findIndex((x) => x.id === m.id);
      if (i >= 0) {
        if (!materials[i]!.custom) throw new HttpError(400, "Built-in materials cannot be changed; save it under a new name");
        materials[i] = m;
      } else materials.push(m);
      return json(m);
    }
    if (b && method === "DELETE") {
      const i = materials.findIndex((x) => x.id === b);
      if (i < 0 || !materials[i]!.custom) throw new HttpError(400, "Only your own materials can be deleted");
      materials.splice(i, 1);
      return new Response(null, { status: 204 });
    }
  }
  if (a === "motors") {
    if (!b && method === "GET") return json(motors);
    if (!b && method === "POST") {
      const m = { ...(body as Motor), custom: true };
      if (!m.name?.trim()) throw new HttpError(400, "Give the motor a name");
      m.id ||= id("mot");
      const i = motors.findIndex((x) => x.id === m.id);
      if (i >= 0) motors[i] = m;
      else motors.push(m);
      return json(m);
    }
    if (b && method === "DELETE") {
      const i = motors.findIndex((x) => x.id === b);
      if (i < 0 || !motors[i]!.custom) throw new HttpError(400, "Only your own motors can be deleted");
      motors.splice(i, 1);
      return new Response(null, { status: 204 });
    }
  }
  if (a === "projects") {
    if (!b && method === "GET") return json([...projects.values()].map(summaryOf).sort((x, y) => y.updated_at.localeCompare(x.updated_at)));
    if (!b && method === "POST") {
      const { name, notes } = body as { name: string; notes?: string };
      const p: Project = { id: id("prj"), name: name?.trim() || "Untitled", notes: notes ?? "", created_at: now(), updated_at: now() };
      projects.set(p.id, p);
      return json(p, 201);
    }
    const p = must(projects.get(b!), "project");
    if (!c) {
      if (method === "GET") {
        const ms = [...models.values()].filter((m) => m.model.project_id === p.id);
        ms.forEach(ensureDescribed);
        return json({ project: p, models: ms.map((m) => m.model), studies: [...studies.values()].filter((s) => s.project_id === p.id) });
      }
      if (method === "PATCH") {
        Object.assign(p, body as object, { updated_at: now() });
        emit({ type: "project", project_id: p.id });
        return json(p);
      }
      if (method === "DELETE") {
        projects.delete(p.id);
        return new Response(null, { status: 204 });
      }
    }
    if (c === "models" && method === "POST") {
      const fd = body as FormData;
      const file = fd.get("file") as File | null;
      if (!file) throw new HttpError(400, "No file was sent");
      const fileName = file.name || "model.stl";
      if (!/\.(stl|step|stp|obj|3mf)$/i.test(fileName)) throw new HttpError(400, "SimLab reads STL, STEP, OBJ and 3MF files");
      const unit = Number(fd.get("unit_mm")) || settings.default_units_mm || 1;
      const rec = addModel(p, String(fd.get("name") || fileName.replace(/\.[^.]+$/, "")), fileName, pickMaker(fileName), unit, false);
      rec.model.message = "Reading the file and finding its faces";
      p.updated_at = now();
      setTimeout(() => {
        describe(rec);
        if (!/example|prop|frame|quad|mount|dense/i.test(fileName)) rec.model.message = "Demo mode: this is a made-up frame standing in for your file.";
        emit({ type: "model", model: rec.model });
        emit({ type: "project", project_id: p.id });
      }, 1200);
      return json(rec.model, 201);
    }
    if (c === "studies" && method === "POST") {
      const s = body as { name: string; model_id: string; config: StudyConfig };
      must(models.get(s.model_id), "model");
      const st = addStudy(p, models.get(s.model_id)!, s.name || "Untitled test", s.config, now());
      emit({ type: "project", project_id: p.id });
      return json(st, 201);
    }
  }
  if (a === "models") {
    const rec = must(models.get(b!), "model");
    ensureDescribed(rec);
    if (!c) {
      if (method === "GET") return json(rec.model);
      if (method === "PATCH") {
        const pb = body as { name?: string; unit_mm?: number; patch_labels?: Record<string, string> };
        if (pb.name) rec.model.name = pb.name;
        if (pb.unit_mm && pb.unit_mm !== rec.model.unit_mm) {
          rec.model.unit_mm = pb.unit_mm;
          describe(rec);
        }
        if (pb.patch_labels)
          for (const [k, v] of Object.entries(pb.patch_labels)) {
            const patch = rec.model.patches?.find((x) => x.id === Number(k));
            if (patch) patch.label = v;
          }
        emit({ type: "model", model: rec.model });
        return json(rec.model);
      }
      if (method === "DELETE") {
        models.delete(rec.model.id);
        return new Response(null, { status: 204 });
      }
    }
    if (c === "surface") {
      const m = meshOf(rec);
      const buf = { "positions.f32": m.positions, "normals.f32": m.normals, "indices.u32": m.indices, "patch.u32": m.patch }[d ?? ""];
      return binary(must(buf, "buffer"));
    }
    if (c === "file") return new Response("solid demo\nendsolid demo\n", { headers: { "content-type": "model/stl" } });
  }
  if (a === "studies") {
    const st = must(studies.get(b!), "test");
    if (!c) {
      if (method === "GET") return json(st);
      if (method === "PATCH") {
        const pb = body as { name?: string; config?: StudyConfig };
        if (pb.name) st.name = pb.name;
        if (pb.config) st.config = pb.config;
        st.updated_at = now();
        return json(st);
      }
      if (method === "DELETE") {
        studies.delete(st.id);
        for (const [k, r] of runs) if (r.run.study_id === st.id) runs.delete(k);
        emit({ type: "project", project_id: st.project_id });
        return new Response(null, { status: 204 });
      }
    }
    if (c === "estimate" && method === "POST") return json(estimate(st, (body as { engine?: Engine } | undefined)?.engine));
    if (c === "runs" && method === "POST") {
      // the engine's rule: Accurate (OpenFOAM) unless told otherwise for wind tunnel tests, SimLab's own for the rest
      const asked = (body as { engine?: string } | undefined)?.engine;
      if (asked && asked !== "native" && asked !== "reference") throw new HttpError(400, "engine must be reference or native");
      if (asked === "reference" && st.kind === "prop") throw new HttpError(400, "the propeller test has no reference engine");
      const engine: Engine = asked === "native" || asked === "reference" ? asked : st.kind === "wind" ? "reference" : "native";
      const docker = checks.find((x) => x.id === "docker");
      if (engine === "reference" && docker && docker.status !== "ok")
        return json({ error: "Accurate air tests and CalculiX checks run OpenFOAM and CalculiX in Docker, and Docker is not ready. Open Settings → System check and press Start Colima (or Build reference image), then run the test again.", needs: { ...docker } }, 409);
      const rec = newRun(st, { engine });
      emitRun(rec);
      startTicking();
      return json(rec.run, 201);
    }
  }
  if (a === "runs") {
    const rec = must(runs.get(b!), "run");
    const r = rec.run;
    if (!c) {
      if (method === "GET") return json(r);
      if (method === "DELETE") {
        runs.delete(r.id);
        const st = studies.get(r.study_id);
        if (st) st.runs = st.runs.filter((x) => x.id !== r.id);
        emit({ type: "project", project_id: rec.project_id });
        return new Response(null, { status: 204 });
      }
    }
    if (c === "stop" && method === "POST") {
      if (r.status === "queued" || r.status === "running") {
        r.status = "stopped";
        r.message = "Stopped";
        r.finished_at = now();
        emitRun(rec);
      }
      return json({ ok: true });
    }
    if (c === "log") return new Response(rec.log.join("\n") + "\n", { headers: { "content-type": "text/plain" } });
    if (r.status !== "done") throw new HttpError(409, "This run has no results yet");
    if (c === "mesh") {
      const st = must(studies.get(r.study_id), "test");
      const m = meshOf(must(models.get(st.model_id), "model"));
      const buf = { "positions.f32": m.positions, "normals.f32": m.normals, "indices.u32": m.indices }[d ?? ""];
      return binary(must(buf, "buffer"));
    }
    if (c === "field") return binary(must(rec.fields?.values[d!], "field"));
    if (c === "slice") {
      const sl = must(rec.slices?.find((s) => s.slice.name === d), "slice");
      if (!e) return json(sl.slice);
      return binary(must(sl.values[e], "slice field"));
    }
  }
  throw new HttpError(404, `The demo engine has no ${method} ${path}`);
}

/** the fake engine as a transport for the API client */
export function mockTransport(opts: { latency?: number } = {}): Transport {
  const latency = opts.latency ?? 120;
  return {
    base: "",
    async fetch(input, init) {
      const u = new URL(input, "http://demo.local");
      const method = (init?.method ?? "GET").toUpperCase();
      let body: unknown;
      if (init?.body instanceof FormData) body = init.body;
      else if (typeof init?.body === "string") body = JSON.parse(init.body);
      if (latency) await new Promise((r) => setTimeout(r, latency * (0.5 + Math.random())));
      try {
        return await handle(method, u.pathname, body);
      } catch (err) {
        if (err instanceof HttpError) return json({ error: err.message }, err.status);
        console.error(err);
        return json({ error: err instanceof Error ? err.message : "The demo engine tripped" }, 500);
      }
    },
    subscribe(on, state) {
      listeners.add(on);
      state?.("connecting");
      const t = setTimeout(() => state?.("open"), 150);
      return () => {
        clearTimeout(t);
        listeners.delete(on);
      };
    },
  };
}

/** for tests: how much memory the demo machine has */
export const DEMO_RAM = RAM;
