// The engine's API types, as docs/API.md and crates/core/src/config.rs
// describe them. Units: model millimetres where a name ends in _mm, SI
// everywhere else unless the name says otherwise (_mpa, _rpm, _g).

export type Vec3 = [number, number, number];

// ---------------------------------------------------------------- system

export type CheckStatus = "ok" | "missing" | "error" | "working" | "optional";
export interface Check {
  id: string;
  label: string;
  status: CheckStatus;
  detail: string;
  fix?: { label: string; asks: string };
}
export interface Health {
  version: string;
  data_dir: string;
  machine: { cpu: string; cores: number; performance_cores?: number; ram_bytes: number; gpu?: string | null; gpu_memory_bytes?: number };
  checks: Check[];
}
export interface Settings {
  memory_budget_gb: number;
  cpu_threads: number;
  default_units_mm: number;
  reference_engine_enabled: boolean;
}

// ---------------------------------------------------------------- library

export type Process = "fdm" | "resin" | "other";
export interface Material {
  id: string;
  name: string;
  process: Process;
  /** kg/m³ */
  density: number;
  /** stiffness along the layers, MPa */
  e_xy_mpa: number;
  /** stiffness across the layers, MPa */
  e_z_mpa: number;
  nu: number;
  /** strength along the layers, MPa */
  uts_xy_mpa: number;
  /** strength across the layers (layer adhesion), MPa */
  uts_z_mpa: number;
  elongation_pct?: number | null;
  hdt_c?: number | null;
  notes?: string;
  sources?: string[];
  estimated_fields?: string[];
  custom?: boolean;
}
export interface Motor {
  id: string;
  name: string;
  kv: number;
  mass_g: number;
  max_thrust_g: number;
  max_current_a: number;
  voltage_v: number;
  prop: string;
  mount_mm: number;
  notes?: string;
  custom?: boolean;
}

// ---------------------------------------------------------------- projects and models

export type RunStatus = "queued" | "running" | "done" | "failed" | "stopped";
export type Engine = "native" | "calculix" | "openfoam" | "reference";
export interface RunBrief {
  id: string;
  status: RunStatus;
  engine: Engine;
  /** 0..1 */
  progress: number;
  created_at: string;
  finished_at?: string | null;
  headline?: string | null;
}
export interface Project {
  id: string;
  name: string;
  notes: string;
  created_at: string;
  updated_at: string;
}
export interface ProjectSummary extends Project {
  models: number;
  studies: number;
  runs: number;
  last_run?: RunBrief | null;
  thumbnail_model_id?: string | null;
}

export type ModelFormat = "stl" | "step" | "obj" | "3mf";
export type PatchKind = "plane" | "cylinder" | "cone" | "sphere" | "torus" | "other";
export interface Patch {
  id: number;
  kind: PatchKind;
  area_mm2: number;
  center_mm: Vec3;
  normal?: Vec3;
  axis?: Vec3;
  radius_mm?: number;
  label?: string;
}
export interface MountGuess {
  center_mm: Vec3;
  axis: Vec3;
  bolt_circle_mm: number;
  patches: number[];
}
export interface PropGuess {
  axis: Vec3;
  center_mm: Vec3;
  radius_mm: number;
  blades: number;
}
export interface Model {
  id: string;
  project_id: string;
  name: string;
  file_name: string;
  format: ModelFormat;
  unit_mm: number;
  status: "processing" | "ready" | "failed";
  message: string;
  created_at: string;
  bbox_mm?: { min: Vec3; max: Vec3 };
  size_mm?: Vec3;
  volume_mm3?: number;
  area_mm2?: number;
  triangles?: number;
  watertight?: boolean;
  patches?: Patch[];
  suggestions?: {
    looks_like: "propeller" | "frame" | "part";
    prop?: PropGuess;
    motor_mounts?: MountGuess[];
  };
}

// ---------------------------------------------------------------- study configs (crates/core/src/config.rs)

export type Support = { type: "fixed"; patches: number[] } | { type: "free_flying" };

export type Load =
  | { type: "force"; patches: number[]; force_n: Vec3; label?: string }
  | { type: "pressure"; patches: number[]; pressure_pa: number; label?: string }
  | { type: "motor"; patches: number[]; thrust_n: number; torque_nm: number; axis: Vec3; motor_mass_g?: number; label?: string }
  | { type: "acceleration"; g: Vec3; label?: string }
  | { type: "spin"; rpm: number; axis: Vec3; center_mm: Vec3; label?: string }
  | { type: "prop_aero"; rpm: number; airspeed_m_s?: number; axis: Vec3; center_mm: Vec3; label?: string };
export type LoadType = Load["type"];

export type MeshQuality = { type: "draft" } | { type: "normal" } | { type: "fine" } | { type: "custom"; size_mm: number };
export type GridQuality = { type: "draft" } | { type: "normal" } | { type: "fine" } | { type: "custom"; cells_across: number };

export interface Air {
  /** kg/m³ */
  density: number;
  /** m²/s */
  kinematic_viscosity: number;
}
export const SEA_LEVEL_AIR: Air = { density: 1.204, kinematic_viscosity: 1.516e-5 };

export interface SpinConfig {
  rpm: number;
  axis: Vec3;
  center_mm: Vec3;
}

export interface StrengthConfig {
  kind: "strength";
  material_id: string;
  print_up: Vec3;
  support: Support;
  loads: Load[];
  quality: MeshQuality;
}
export interface WindConfig {
  kind: "wind";
  wind_dir: Vec3;
  speed_m_s: number;
  air?: Air;
  spin?: SpinConfig | null;
  quality: GridQuality;
  duration?: number | null;
}
export interface PropConfig {
  kind: "prop";
  axis: Vec3;
  center_mm: Vec3;
  rpm_min: number;
  rpm_max: number;
  rpm_steps?: number;
  airspeeds_m_s?: number[];
  air?: Air;
  blades?: number | null;
}
export type StudyConfig = StrengthConfig | WindConfig | PropConfig;
export type StudyKind = StudyConfig["kind"];

export interface Study {
  id: string;
  project_id: string;
  model_id: string;
  name: string;
  kind: StudyKind;
  config: StudyConfig;
  created_at: string;
  updated_at: string;
  runs: RunBrief[];
}

export interface Estimate {
  fits: boolean;
  memory_bytes: number;
  budget_bytes: number;
  size: { elements?: number; nodes?: number; cells?: number };
  seconds: number;
  note: string;
}

// ---------------------------------------------------------------- runs and results

export interface FieldInfo {
  name: string;
  label: string;
  unit: string;
  min: number;
  max: number;
  scale: "linear" | "log";
  diverging?: boolean;
}

export interface StrengthSummary {
  kind: "strength";
  headline: string;
  verdict: "safe" | "marginal" | "breaks";
  safety_factor: number;
  max_von_mises_mpa: number;
  max_principal_mpa: number;
  max_displacement_mm: number;
  critical_point_mm: Vec3;
  critical_patch?: number | null;
  breaks_at?: { load_factor: number; rpm?: number | null; text: string } | null;
  mass_g: number;
  elements: number;
  nodes: number;
  dofs: number;
  solver: string;
  solve_seconds: number;
  mesh_seconds: number;
  reactions_n?: Vec3 | null;
  warnings: string[];
}
export interface WindSummary {
  kind: "wind";
  headline: string;
  force_n: Vec3;
  moment_nm: Vec3;
  drag_n: number;
  lift_n: number;
  side_n: number;
  cd?: number | null;
  ref_area_mm2: number;
  thrust_n?: number | null;
  torque_nm?: number | null;
  power_w?: number | null;
  reynolds: number;
  cells: number;
  steps: number;
  mlups: number;
  seconds: number;
  converged: boolean;
  warnings: string[];
}
export interface PropPoint {
  rpm: number;
  airspeed_m_s: number;
  thrust_n: number;
  thrust_g: number;
  torque_nm: number;
  power_w: number;
  ct: number;
  cp: number;
  efficiency?: number | null;
}
export interface PropSummary {
  kind: "prop";
  headline: string;
  blades: number;
  radius_mm: number;
  hub_radius_mm: number;
  rotation: "cw" | "ccw";
  geometry: { r_mm: number; chord_mm: number; pitch_deg: number }[];
  points: PropPoint[];
  warnings: string[];
}
export type Summary = StrengthSummary | WindSummary | PropSummary;

export interface Run extends RunBrief {
  study_id: string;
  message: string;
  started_at?: string | null;
  config: StudyConfig;
  material?: Material | null;
  summary?: Summary | null;
  fields: FieldInfo[];
  slices: { name: string; label: string }[];
  metrics: Record<string, [number, number][]>;
  error?: string | null;
}

export interface Slice {
  name: string;
  label: string;
  origin_mm: Vec3;
  /** axis vectors whose length is the slice size */
  u_mm: Vec3;
  v_mm: Vec3;
  nu: number;
  nv: number;
  fields: { name: string; label: string; unit: string; min: number; max: number }[];
}

/** a surface mesh: positions (mm) and normals xyz per vertex, three indices per triangle */
export interface SurfaceBuffers {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
  /** the surface patch of each triangle (model surfaces only) */
  patch?: Uint32Array;
}

// ---------------------------------------------------------------- live events

/** a run's live update: the engine's progress ticks carry only id, status, progress, message and project */
export type RunEventBrief = Pick<RunBrief, "id" | "status" | "progress"> & Partial<Omit<RunBrief, "id" | "status" | "progress">> & { study_id?: string; project_id: string; message: string };
export type EngineEvent =
  | { type: "run"; run: RunEventBrief }
  | { type: "metric"; run_id: string; name: string; step: number; value: number }
  | { type: "model"; model: Model }
  | { type: "project"; project_id: string }
  | { type: "check"; check: Check };
