// Numbers and words the way people read them.
import type { Load, Material, Patch, RunBrief, RunStatus, StudyKind, Summary, Vec3 } from "./types";

export type Tone = "neutral" | "ember" | "moss" | "sky" | "gold" | "plum" | "clay";

/** a number with sensible digits: 3.14, 31.4, 314, 3,140 */
export function n(v: number | null | undefined, digits = 3): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "–";
  if (v === 0) return "0";
  const a = Math.abs(v);
  if (a >= 1000) return Math.round(v).toLocaleString("en-US");
  const s = Number(v.toPrecision(digits));
  return s.toLocaleString("en-US", { maximumFractionDigits: 6 });
}

export const fixed = (v: number | null | undefined, d = 1) => (v === null || v === undefined || !Number.isFinite(v) ? "–" : v.toFixed(d));

export function bytes(b: number): string {
  if (!Number.isFinite(b) || b <= 0) return "0 MB";
  const gb = b / 1024 ** 3;
  if (gb >= 1) return `${n(gb, 2)} GB`;
  return `${n(b / 1024 ** 2, 2)} MB`;
}

export function duration(s: number): string {
  if (!Number.isFinite(s)) return "–";
  if (s < 1) return "under a second";
  if (s < 60) return `${Math.round(s)} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  const h = Math.floor(s / 3600);
  const m = Math.round((s % 3600) / 60);
  return m ? `${h} h ${m} min` : `${h} h`;
}

export function ago(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  const s = Math.max(0, (now - t) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  if (s < 86400 * 7) return `${Math.round(s / 86400)} d ago`;
  return new Date(t).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

export const KIND_LABEL: Record<StudyKind, string> = { strength: "Strength", wind: "Wind tunnel", prop: "Propeller" };
export const KIND_QUESTION: Record<StudyKind, string> = {
  strength: "Will it break?",
  wind: "How does air flow around it?",
  prop: "How much does it push?",
};
export const KIND_TONE: Record<StudyKind, Tone> = { strength: "ember", wind: "sky", prop: "plum" };

export const STATUS_LABEL: Record<RunStatus, string> = { queued: "Waiting", running: "Running", done: "Done", failed: "Failed", stopped: "Stopped" };
export const STATUS_TONE: Record<RunStatus, Tone> = { queued: "neutral", running: "ember", done: "moss", failed: "clay", stopped: "neutral" };

/**
 * A short pill for a run: "Safe 3.1×", "Breaks at 28,500 rpm", "Drag 0.42 N".
 * Built from the run's one-sentence headline, since a run brief carries
 * nothing else about the result.
 */
export function pillFor(r: (Pick<RunBrief, "status" | "headline" | "progress"> & { verdict?: RunBrief["verdict"]; short?: string | null }) | null | undefined): { tone: Tone; text: string } | null {
  if (!r) return null;
  if (r.status === "queued") return { tone: "neutral", text: "Waiting" };
  if (r.status === "running") return { tone: "ember", text: `Running ${Math.round((r.progress || 0) * 100)}%` };
  if (r.status === "failed") return { tone: "clay", text: "Failed" };
  if (r.status === "stopped") return { tone: "neutral", text: "Stopped" };
  const h = (r.headline ?? "").trim();
  // the engine's own short label wins (e.g. "392 g at 45,800 rpm")
  if (r.short && !r.verdict) return { tone: "sky", text: r.short };
  if (!h) return { tone: "moss", text: "Done" };
  const times = /(\d+(?:\.\d+)?)\s*×/.exec(h)?.[1];
  // the engine's own verdict wins over reading the sentence
  const v = r.verdict ?? (/^safe/i.test(h) ? "safe" : /^marginal/i.test(h) ? "marginal" : /^breaks?/i.test(h) ? "breaks" : null);
  if (v === "safe") return { tone: "moss", text: times ? `Safe ${times}×` : "Safe" };
  if (v === "marginal") return { tone: "gold", text: times ? `Marginal ${times}×` : "Marginal" };
  if (v === "breaks") return { tone: "clay", text: /^breaks?/i.test(h) ? firstClause(h).replace(/\babout\s+/i, "") : "Breaks" };
  if (/^safe/i.test(h)) return { tone: "moss", text: times ? `Safe ${times}×` : "Safe" };
  if (/^marginal/i.test(h)) return { tone: "gold", text: times ? `Marginal ${times}×` : "Marginal" };
  if (/^breaks?/i.test(h)) return { tone: "clay", text: firstClause(h).replace(/\babout\s+/i, "") };
  return { tone: "sky", text: firstClause(h) };
}

function firstClause(h: string): string {
  // split at ", " or ": " or " — " but not inside a number like 28,500
  const cut = h.split(/,\s(?!\d)|:\s|\s[—–-]\s|\.\s/)[0] ?? h;
  const s = cut.replace(/\.$/, "");
  return s.length > 34 ? `${s.slice(0, 32).trimEnd()}…` : s;
}

export function verdictTone(s: Summary | null | undefined): Tone {
  if (!s) return "neutral";
  if (s.kind === "strength") return s.verdict === "safe" ? "moss" : s.verdict === "marginal" ? "gold" : "clay";
  return s.kind === "wind" ? "sky" : "plum";
}

/** "Hole, 3.2 mm across", "Flat face, 120 mm²" */
export function patchLabel(p: Patch, hole?: boolean): string {
  if (p.label) return p.label;
  switch (p.kind) {
    case "cylinder":
      return p.radius_mm ? `${hole === false ? "Round face" : "Hole"}, ${n(p.radius_mm * 2, 2)} mm across` : "Round face";
    case "plane":
      return `Flat face, ${n(p.area_mm2, 2)} mm²`;
    case "cone":
      return "Sloped round face";
    case "sphere":
      return "Ball-shaped face";
    case "torus":
      return "Rounded edge";
    default:
      return `Curved face, ${n(p.area_mm2, 2)} mm²`;
  }
}

export const KIND_NAME: Record<Patch["kind"], string> = { plane: "Flat", cylinder: "Round", cone: "Cone", sphere: "Ball", torus: "Rounded edge", other: "Curved" };

const AXES: [Vec3, string][] = [
  [[1, 0, 0], "+X"],
  [[-1, 0, 0], "−X"],
  [[0, 1, 0], "+Y"],
  [[0, -1, 0], "−Y"],
  [[0, 0, 1], "+Z"],
  [[0, 0, -1], "−Z"],
];
/** "+Z" for an axis-aligned direction, else "x, y, z" */
export function dirLabel(v: Vec3): string {
  const len = Math.hypot(...v) || 1;
  const u = v.map((c) => c / len) as Vec3;
  const hit = AXES.find(([a]) => a[0] * u[0] + a[1] * u[1] + a[2] * u[2] > 0.999);
  return hit ? hit[1] : u.map((c) => fixed(c, 2)).join(", ");
}

export function materialLine(m: Material): string {
  if (m.process === "fdm") return `${n(m.uts_xy_mpa)} MPa along layers · ${n(m.uts_z_mpa)} across`;
  return `${n(m.uts_xy_mpa)} MPa strong`;
}

export const G = 9.80665;

/** one line about a load, for lists */
export function loadLine(l: Load): string {
  switch (l.type) {
    case "motor":
      return `${n(l.thrust_n / G * 1000)} g thrust, ${n(l.torque_nm, 2)} N·m twist${l.motor_mass_g ? `, ${n(l.motor_mass_g)} g motor` : ""}`;
    case "force":
      return `${n(Math.hypot(...l.force_n))} N toward ${dirLabel(l.force_n)}`;
    case "pressure":
      return `${n(l.pressure_pa / 1000)} kPa`;
    case "acceleration":
      return `${n(Math.hypot(...l.g))} g toward ${dirLabel(l.g)}`;
    case "spin":
      return `${n(l.rpm)} rpm about ${dirLabel(l.axis)}`;
    case "prop_aero":
      return `${n(l.rpm)} rpm${l.airspeed_m_s ? ` at ${n(l.airspeed_m_s)} m/s` : ", hovering"}`;
  }
}

export const LOAD_NAME: Record<Load["type"], string> = {
  motor: "Motor",
  force: "Push or pull",
  pressure: "Pressure",
  acceleration: "Crash or acceleration",
  spin: "Spin",
  prop_aero: "Propeller air load",
};
