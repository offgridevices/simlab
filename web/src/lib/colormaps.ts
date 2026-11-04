// Every colour scale in one module. Which scale shows which kind of result
// is a design choice in tokens.css (--map-magnitude, --map-movement,
// --map-signed, --map-safety); the stops of the perceptual scales are data in
// colormap-data.ts; the safety scale is built from the status colours in
// tokens.css (clay, gold, moss), so it follows the theme. Clear steps at a
// safety factor of 1 (breaks) and 2 (safe), with a little shading inside each
// band so you can still read how deep into it a spot is.
//
// The viewer's shader, the legends and the flow slices all sample from here,
// so they always agree.
import type { FieldInfo } from "./types";
import { CIVIDIS, COOLWARM, STATUS_FALLBACK, VIRIDIS } from "./colormap-data";

export type ColormapName = "viridis" | "cividis" | "coolwarm" | "safety";
export type RGB = [number, number, number];

/** the safety factor's scale runs 0 to this; anything higher is as safe as it gets */
export const SAFETY_MAX = 4;
/** where the safety colours change */
export const SAFETY_STEPS = [1, 2] as const;

const clamp01 = (t: number) => (t < 0 ? 0 : t > 1 ? 1 : t);

function parseColor(c: string): RGB | null {
  const s = c.trim();
  const h = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(s)?.[1];
  if (h) {
    const f = h.length === 3 ? h.split("").map((x) => x + x).join("") : h;
    return [parseInt(f.slice(0, 2), 16) / 255, parseInt(f.slice(2, 4), 16) / 255, parseInt(f.slice(4, 6), 16) / 255];
  }
  const m = /^rgba?\(([^)]+)\)$/i.exec(s);
  if (m) {
    const [r, g, b] = m[1]!.split(/[\s,/]+/).map(Number);
    if ([r, g, b].every((v) => Number.isFinite(v))) return [r! / 255, g! / 255, b! / 255];
  }
  return null;
}

/** a token's colour now, or the fallback */
function token(name: string, fallback: string): RGB {
  let v = "";
  try {
    v = typeof document !== "undefined" ? getComputedStyle(document.documentElement).getPropertyValue(name) : "";
  } catch {
    v = "";
  }
  return parseColor(v) ?? parseColor(fallback)!;
}
function tokenWord(name: string, fallback: string): string {
  try {
    const v = typeof document !== "undefined" ? getComputedStyle(document.documentElement).getPropertyValue(name).trim() : "";
    return v || fallback;
  } catch {
    return fallback;
  }
}

const mix = (a: RGB, b: RGB, k: number): RGB => [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
const darken = (c: RGB, k: number): RGB => mix(c, [0, 0, 0], k);
const lighten = (c: RGB, k: number): RGB => mix(c, [1, 1, 1], k);

type Stops = [number, RGB][];
const even = (hexes: string[]): Stops => hexes.map((h, i, a) => [i / (a.length - 1), parseColor(h)!]);
const FIXED: Record<Exclude<ColormapName, "safety">, Stops> = { viridis: even(VIRIDIS), cividis: even(CIVIDIS), coolwarm: even(COOLWARM) };

/** the status colours the safety scale is made of, read from the tokens now */
export function statusColors() {
  return {
    clay: token("--clay", STATUS_FALLBACK.clay),
    gold: token("--gold", STATUS_FALLBACK.gold),
    moss: token("--moss", STATUS_FALLBACK.moss),
    nan: token("--nan", STATUS_FALLBACK.nan),
  };
}

/** a stamp that changes when the colours a scale depends on change (the theme) */
export function paletteStamp(): string {
  const s = statusColors();
  return [s.clay, s.gold, s.moss, s.nan].map((c) => c.map((v) => v.toFixed(3)).join(",")).join("|");
}

function safetyStops(): Stops {
  const { clay, gold, moss } = statusColors();
  const e = 1e-4;
  const at = (sf: number) => sf / SAFETY_MAX;
  return [
    [at(0), darken(clay, 0.35)],
    [at(1 - e), clay],
    [at(1), darken(gold, 0.12)],
    [at(2 - e), lighten(gold, 0.08)],
    [at(2), moss],
    [at(SAFETY_MAX), darken(moss, 0.3)],
  ];
}

function fromStops(stops: Stops, t: number): RGB {
  const x = clamp01(t);
  for (let i = 1; i < stops.length; i++) {
    const [t1, c1] = stops[i]!;
    if (x <= t1) {
      const [t0, c0] = stops[i - 1]!;
      return mix(c0, c1, t1 > t0 ? (x - t0) / (t1 - t0) : 0);
    }
  }
  return stops.at(-1)![1];
}

/** the colour at t (0..1) on a scale, as 0..1 RGB */
export function sample(name: ColormapName, t: number): RGB {
  return fromStops(name === "safety" ? safetyStops() : FIXED[name], t);
}

/** the scale as `n` RGBA texels, for the viewer's shader */
export function lut(name: ColormapName, n = 256): Uint8Array {
  const stops = name === "safety" ? safetyStops() : FIXED[name];
  const out = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const [r, g, b] = fromStops(stops, i / (n - 1));
    out[i * 4] = Math.round(r * 255);
    out[i * 4 + 1] = Math.round(g * 255);
    out[i * 4 + 2] = Math.round(b * 255);
    out[i * 4 + 3] = 255;
  }
  return out;
}

const hex2 = (v: number) => Math.round(clamp01(v) * 255).toString(16).padStart(2, "0");
/** an RGB colour as CSS */
export const css = ([r, g, b]: RGB) => `#${hex2(r)}${hex2(g)}${hex2(b)}`;

/** a CSS gradient of the scale, left to right, for legends (hard steps where the scale steps) */
export function cssGradient(name: ColormapName, steps = 24): string {
  const stops = name === "safety" ? safetyStops() : FIXED[name];
  const ts = [...new Set([...Array.from({ length: steps + 1 }, (_, i) => i / steps), ...stops.map(([t]) => t)])].sort((a, b) => a - b);
  return `linear-gradient(to right, ${ts.map((t) => `${css(fromStops(stops, t))} ${(t * 100).toFixed(2)}%`).join(", ")})`;
}

export function cssColor(name: ColormapName, t: number): string {
  return css(sample(name, t));
}

/** how a field's values become colours: the scale, the range, log or not, centred on zero or not */
export interface ColorScale {
  map: ColormapName;
  min: number;
  max: number;
  log: boolean;
  diverging: boolean;
}

const asMap = (v: string, fallback: ColormapName): ColormapName => (["viridis", "cividis", "coolwarm", "safety"].includes(v) ? (v as ColormapName) : fallback);

/** the colour scale a field is shown with */
export function scaleFor(f: Pick<FieldInfo, "name" | "min" | "max" | "scale" | "diverging">): ColorScale {
  if (isSafety(f.name)) return { map: asMap(tokenWord("--map-safety", "safety"), "safety"), min: 0, max: SAFETY_MAX, log: false, diverging: false };
  if (f.diverging && !isMovementComponent(f.name)) {
    const m = Math.max(Math.abs(f.min), Math.abs(f.max)) || 1;
    return { map: asMap(tokenWord("--map-signed", "coolwarm"), "coolwarm"), min: -m, max: m, log: false, diverging: true };
  }
  const log = f.scale === "log" && f.min > 0 && f.max > f.min;
  const movement = /disp|deform|bend|movement/i.test(f.name);
  const map = movement ? asMap(tokenWord("--map-movement", "cividis"), "cividis") : asMap(tokenWord("--map-magnitude", "viridis"), "viridis");
  return { map, min: f.min, max: f.max > f.min ? f.max : f.min + 1, log, diverging: false };
}

export const isSafety = (name: string) => /safety/i.test(name);
const isMovementComponent = (name: string) => /^u[xyz]$/.test(name);

/** where a value falls on a scale, 0..1 (NaN stays NaN) */
export function normalize(v: number, s: Pick<ColorScale, "min" | "max" | "log">): number {
  if (Number.isNaN(v)) return NaN;
  if (s.log) {
    const lo = Math.log10(s.min);
    const hi = Math.log10(s.max);
    return clamp01((Math.log10(Math.max(v, s.min)) - lo) / (hi - lo || 1));
  }
  return clamp01((v - s.min) / (s.max - s.min || 1));
}

/** values to RGBA texels (NaN, inside the part, becomes see-through), for flow slices */
export function colorize(values: Float32Array, s: ColorScale): Uint8Array {
  const out = new Uint8Array(values.length * 4);
  const table = lut(s.map, 256);
  for (let i = 0; i < values.length; i++) {
    const t = normalize(values[i]!, s);
    if (Number.isNaN(t)) continue; // alpha stays 0
    const k = Math.round(t * 255) * 4;
    out[i * 4] = table[k]!;
    out[i * 4 + 1] = table[k + 1]!;
    out[i * 4 + 2] = table[k + 2]!;
    out[i * 4 + 3] = 235;
  }
  return out;
}

/** a few round tick values for a legend */
export function legendTicks(s: ColorScale, count = 4): number[] {
  if (s.map === "safety") return [0, ...SAFETY_STEPS, 3, SAFETY_MAX];
  if (s.log) {
    const out: number[] = [];
    for (let e = Math.ceil(Math.log10(s.min)); e <= Math.floor(Math.log10(s.max)); e++) out.push(10 ** e);
    return out.length >= 2 ? out : [s.min, s.max];
  }
  // round numbers inside the range
  const span = s.max - s.min;
  const raw = span / count;
  const p = 10 ** Math.floor(Math.log10(raw || 1));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * p).find((x) => x >= raw) ?? raw;
  const out: number[] = [];
  for (let v = Math.ceil(s.min / step) * step; v <= s.max + step * 1e-9; v += step) out.push(Number(v.toPrecision(10)));
  return out.length >= 2 ? out : [s.min, s.max];
}

/** the colour for "no value here" (inside the part, outside the data) */
export function nanColor(): RGB {
  return statusColors().nan;
}
