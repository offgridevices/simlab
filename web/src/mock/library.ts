// Demo mode's material and motor library: typical published values for
// printed parts, rounded. The engine ships its own researched library.
import type { Check, Health, Material, Motor, Settings } from "../lib/types";

const m = (id: string, name: string, process: Material["process"], density: number, e_xy: number, e_z: number, nu: number, uts_xy: number, uts_z: number, notes = ""): Material => ({
  id,
  name,
  process,
  density,
  e_xy_mpa: e_xy,
  e_z_mpa: e_z,
  nu,
  uts_xy_mpa: uts_xy,
  uts_z_mpa: uts_z,
  notes,
  sources: ["Demo values"],
  estimated_fields: [],
});

export const MATERIALS: Material[] = [
  m("pla", "PLA", "fdm", 1240, 3000, 2600, 0.35, 50, 30, "Stiff and easy to print; softens in a hot car (about 55 °C)."),
  m("pla-plus", "PLA+ (tough PLA)", "fdm", 1230, 2700, 2300, 0.35, 47, 32),
  m("petg", "PETG", "fdm", 1270, 2000, 1800, 0.38, 45, 30, "Tougher than PLA, bends before it snaps."),
  m("petg-cf", "PETG-CF (carbon filled)", "fdm", 1300, 3800, 2200, 0.36, 55, 25),
  m("abs", "ABS", "fdm", 1040, 2100, 1800, 0.35, 38, 22),
  m("asa", "ASA", "fdm", 1070, 2200, 1900, 0.35, 42, 24, "Like ABS, but stands up to sunlight."),
  m("pa12-cf", "Nylon PA12-CF (carbon filled)", "fdm", 1150, 6000, 2500, 0.35, 85, 35, "Strong and light; dry it before printing."),
  m("pc", "Polycarbonate (PC)", "fdm", 1200, 2300, 2000, 0.37, 60, 35),
  m("tpu-95a", "TPU 95A (flexible)", "fdm", 1210, 26, 22, 0.45, 35, 20, "Rubbery: for bumpers and mounts that soak up crashes."),
  m("resin-standard", "Standard resin", "resin", 1150, 2000, 2000, 0.38, 28, 28, "Crisp detail, but brittle."),
  m("resin-tough", "Tough resin (ABS-like)", "resin", 1180, 1800, 1800, 0.38, 45, 45),
  m("resin-rigid", "Rigid resin (glass filled)", "resin", 1450, 4000, 4000, 0.35, 75, 75),
  m("resin-flex", "Flexible resin", "resin", 1100, 15, 15, 0.45, 8, 8),
  m("cf-plate", "Carbon fibre plate", "other", 1550, 60000, 60000, 0.3, 600, 600, "Cut sheet, not printed: for comparison."),
  m("al-6061", "Aluminium 6061-T6", "other", 2700, 68900, 68900, 0.33, 276, 276, "Machined, not printed: strength is where it starts to bend for good."),
];

export const MOTORS: Motor[] = [
  { id: "1404-3800", name: "1404 3800 KV (3-inch)", kv: 3800, mass_g: 9, max_thrust_g: 380, max_current_a: 12, voltage_v: 14.8, prop: "3 inch, 3 blades", mount_mm: 9 },
  { id: "2207-1950", name: "2207 1950 KV (5-inch, 6S)", kv: 1950, mass_g: 33, max_thrust_g: 1450, max_current_a: 42, voltage_v: 22.2, prop: "5.1 × 4.3 inch, 3 blades", mount_mm: 16 },
  { id: "2306-2450", name: "2306 2450 KV (5-inch, 4S)", kv: 2450, mass_g: 31, max_thrust_g: 1250, max_current_a: 38, voltage_v: 14.8, prop: "5 × 4.3 inch, 3 blades", mount_mm: 16 },
  { id: "2806-1300", name: "2806.5 1300 KV (7-inch)", kv: 1300, mass_g: 50, max_thrust_g: 2000, max_current_a: 45, voltage_v: 22.2, prop: "7 × 3.5 inch, 2 blades", mount_mm: 16 },
  { id: "2812-900", name: "2812 900 KV (9-inch)", kv: 900, mass_g: 75, max_thrust_g: 2600, max_current_a: 40, voltage_v: 22.2, prop: "9 × 4.5 inch, 2 blades", mount_mm: 19 },
];

export const RAM = 36 * 1024 ** 3;

export function health(checks: Check[]): Health {
  return {
    version: "0.1.0-demo",
    data_dir: "~/Library/Application Support/SimLab",
    machine: { cpu: "Apple M3 Pro", cores: 12, ram_bytes: RAM, gpu: "Apple M3 Pro (18-core GPU)", gpu_memory_bytes: RAM },
    checks,
  };
}

export const CHECKS: Check[] = [
  { id: "engine", label: "SimLab engine", status: "ok", detail: "Running, version 0.1.0" },
  { id: "mesher", label: "Part mesher (Gmsh)", status: "ok", detail: "Installed in SimLab's own Python, version 4.13" },
  { id: "gpu", label: "Graphics chip for the wind tunnel", status: "ok", detail: "Metal, 18 GPU cores, shares the Mac's 36 GB" },
  { id: "disk", label: "Free disk space", status: "ok", detail: "212 GB free" },
  {
    id: "reference",
    label: "Reference solvers (CalculiX, OpenFOAM)",
    status: "optional",
    detail: "Not set up. Optional: they double-check SimLab's answers, more slowly.",
    fix: { label: "Set up", asks: "Download Colima and the reference solvers? It is about 1.2 GB, and Colima uses 8 GB of memory while it runs." },
  },
];

export const SETTINGS: Settings = { memory_budget_gb: 18, cpu_threads: 12, default_units_mm: 1, reference_engine_enabled: false };
