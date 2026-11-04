// Ready-made setups the app offers: motors on every mount it found, a
// propeller spinning at a speed, and the defaults for each kind of test.
import { REGION_ID_BASE, type Load, type Model, type Motor, type PropConfig, type Region, type StrengthConfig, type StudyConfig, type StudyKind, type Vec3, type WindConfig } from "../lib/types";
import { G } from "../lib/format";

/** shaft torque at full throttle (as crates/core Motor::est_torque_nm): 80% of electrical power over the loaded shaft speed */
export function motorTorque(m: Pick<Motor, "max_current_a" | "voltage_v" | "kv">): number {
  const shaft = 0.8 * m.max_current_a * m.voltage_v;
  const rpm = m.kv * m.voltage_v * 0.75;
  return rpm > 0 ? shaft / ((rpm * 2 * Math.PI) / 60) : 0;
}

export const DEFAULT_MOTOR: Motor = { id: "2207-1950", name: "2207 1950 KV (5-inch, 6S)", kv: 1950, mass_g: 33, max_thrust_g: 1450, max_current_a: 42, voltage_v: 22.2, prop: "5.1 × 4.3 inch", mount_mm: 16 };

export function motorLoad(m: Motor, patches: number[], axis: Vec3, label: string): Extract<Load, { type: "motor" }> {
  return { type: "motor", patches, thrust_n: (m.max_thrust_g / 1000) * G, torque_nm: motorTorque(m), axis, motor_mass_g: m.mass_g, label };
}

export interface Preset {
  name?: string;
  config: Partial<StudyConfig> & { kind: StudyKind };
}

export function presetFromMounts(model: Model, motor: Motor = DEFAULT_MOTOR): Preset {
  const mounts = model.suggestions?.motor_mounts ?? [];
  return {
    name: "Full throttle",
    config: {
      kind: "strength",
      support: { type: "free_flying" },
      loads: mounts.map((g, i) => motorLoad(motor, g.patches, g.axis, `Motor ${i + 1}`)),
    },
  };
}

/** the shaft hole: round faces close around the spin axis */
export function hubFaces(model: Model, center: Vec3, axis: Vec3, radius: number): number[] {
  const out: number[] = [];
  for (const p of model.patches ?? []) {
    if (p.kind !== "cylinder" || !p.radius_mm || p.radius_mm > radius * 0.12) continue;
    const d: Vec3 = [p.center_mm[0] - center[0], p.center_mm[1] - center[1], p.center_mm[2] - center[2]];
    const along = d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2];
    const off = Math.hypot(d[0] - along * axis[0], d[1] - along * axis[1], d[2] - along * axis[2]);
    if (off < Math.max(0.5, p.radius_mm * 0.3)) out.push(p.id);
  }
  return out;
}

export function presetSpin(model: Model, rpm: number, airspeed = 0): Preset {
  const g = model.suggestions?.prop;
  const axis = g?.axis ?? [0, 0, 1];
  const center = g?.center_mm ?? [0, 0, 0];
  const held = g ? hubFaces(model, center, axis, g.radius_mm) : [];
  // a smooth part with no hub faces of its own: hold a round area around the axis instead
  const hubArea: Region | null = !held.length && g ? { id: REGION_ID_BASE, label: "Hub", shape: { type: "cylinder", axis, center_mm: center, radius_mm: g.hub_radius_mm ?? g.radius_mm * 0.11 } } : null;
  return {
    name: `Spinning at ${Math.round(rpm).toLocaleString("en-US")} rpm`,
    config: {
      kind: "strength",
      ...(hubArea ? { regions: [hubArea] } : {}),
      support: held.length ? { type: "fixed", patches: held } : hubArea ? { type: "fixed", patches: [hubArea.id] } : { type: "free_flying" },
      loads: [
        { type: "spin", rpm, axis, center_mm: center, label: "Spin" },
        { type: "prop_aero", rpm, airspeed_m_s: airspeed, axis, center_mm: center, label: "Air load" },
      ],
    },
  };
}

function centre(model: Model | undefined): Vec3 {
  const b = model?.bbox_mm;
  return b ? ([0, 1, 2].map((k) => (b.min[k]! + b.max[k]!) / 2) as Vec3) : [0, 0, 0];
}

export function defaults(kind: StudyKind, model: Model | undefined, materialId = "pla"): StudyConfig {
  const prop = model?.suggestions?.prop;
  if (kind === "strength") {
    const c: StrengthConfig = { kind: "strength", material_id: materialId, print_up: [0, 0, 1], support: { type: "fixed", patches: [] }, loads: [], quality: { type: "normal" } };
    return c;
  }
  if (kind === "wind") {
    const c: WindConfig = { kind: "wind", wind_dir: [0, 1, 0], speed_m_s: 15, spin: null, quality: { type: "normal" } };
    if (prop) c.spin = null;
    return c;
  }
  const c: PropConfig = {
    kind: "prop",
    axis: prop?.axis ?? [0, 0, 1],
    center_mm: prop?.center_mm ?? centre(model),
    rpm_min: 5000,
    rpm_max: 30000,
    rpm_steps: 12,
    airspeeds_m_s: [0],
    blades: prop?.blades ?? null,
  };
  return c;
}

export { centre as modelCentre };
