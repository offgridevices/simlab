// Every colour scale in one place. Magnitudes use perceptual maps (turbo
// for stress and air speed, viridis for how far the part moves); signed
// values (pressure) use a diverging map centred on zero; the safety factor
// has its own: clay below 1 (breaks), gold from 1 to 2 (marginal), moss
// above 2 (safe), blended smoothly. The viewer's shader and the legends both
// read from here, so they always agree.
import type { FieldInfo } from "./types";

export type ColormapName = "turbo" | "viridis" | "diverging" | "safety";
export type RGB = [number, number, number];

/** the safety factor's scale: 0 to this; anything higher is as safe as it gets */
export const SAFETY_MAX = 4;

const clamp01 = (t: number) => (t < 0 ? 0 : t > 1 ? 1 : t);

// Google's polynomial fit of Turbo (Mikhailov 2019, Apache-2.0)
function turbo(t: number): RGB {
  const x = clamp01(t);
  const x2 = x * x;
  const x3 = x2 * x;
  const x4 = x2 * x2;
  const x5 = x4 * x;
  return [
    clamp01(0.13572138 + 4.6153926 * x - 42.66032258 * x2 + 132.13108234 * x3 - 152.94239396 * x4 + 59.28637943 * x5),
    clamp01(0.09140261 + 2.19418839 * x + 4.84296658 * x2 - 14.18503333 * x3 + 4.27729857 * x4 + 2.82956604 * x5),
    clamp01(0.1066733 + 12.64194608 * x - 60.58204836 * x2 + 110.36276771 * x3 - 89.90310912 * x4 + 27.34824973 * x5),
  ];
}

const hex = (h: string): RGB => [parseInt(h.slice(1, 3), 16) / 255, parseInt(h.slice(3, 5), 16) / 255, parseInt(h.slice(5, 7), 16) / 255];

type Stops = [number, RGB][];
const VIRIDIS: Stops = ["#440154", "#482878", "#3e4989", "#31688e", "#26828e", "#1f9e89", "#35b779", "#6ece58", "#fde725"].map((c, i, a) => [i / (a.length - 1), hex(c)]);
// Moreland's cool-warm: blue for below zero, near-grey at zero, red above
const DIVERGING: Stops = [
  [0, hex("#3b4cc0")],
  [0.25, hex("#8db0fe")],
  [0.5, hex("#d9d6cf")],
  [0.75, hex("#f49a7b")],
  [1, hex("#b40426")],
];
// in safety-factor units, then scaled to 0..1 over 0..SAFETY_MAX
const SAFETY: Stops = (
  [
    [0, "#b8401f"],
    [0.9, "#f07a5a"],
    [1.15, "#f0b84a"],
    [1.8, "#f0b84a"],
    [2.2, "#8cc084"],
    [SAFETY_MAX, "#5e9a57"],
  ] as const
).map(([v, c]) => [v / SAFETY_MAX, hex(c)]);

function fromStops(stops: Stops, t: number): RGB {
  const x = clamp01(t);
  for (let i = 1; i < stops.length; i++) {
    const [t1, c1] = stops[i]!;
    if (x <= t1) {
      const [t0, c0] = stops[i - 1]!;
      const k = t1 > t0 ? (x - t0) / (t1 - t0) : 0;
      return [c0[0] + (c1[0] - c0[0]) * k, c0[1] + (c1[1] - c0[1]) * k, c0[2] + (c1[2] - c0[2]) * k];
    }
  }
  return stops.at(-1)![1];
}

/** the colour at t (0..1) on a scale, as 0..1 RGB */
export function sample(name: ColormapName, t: number): RGB {
  switch (name) {
    case "turbo":
      return turbo(t);
    case "viridis":
      return fromStops(VIRIDIS, t);
    case "diverging":
      return fromStops(DIVERGING, t);
    case "safety":
      return fromStops(SAFETY, t);
  }
}

/** the scale as `n` RGBA texels, for the viewer's shader */
export function lut(name: ColormapName, n = 256): Uint8Array {
  const out = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const [r, g, b] = sample(name, i / (n - 1));
    out[i * 4] = Math.round(r * 255);
    out[i * 4 + 1] = Math.round(g * 255);
    out[i * 4 + 2] = Math.round(b * 255);
    out[i * 4 + 3] = 255;
  }
  return out;
}

const css = ([r, g, b]: RGB) => `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;

/** a CSS gradient of the scale, left to right, for legends */
export function cssGradient(name: ColormapName, steps = 16): string {
  const parts = Array.from({ length: steps + 1 }, (_, i) => `${css(sample(name, i / steps))} ${((i / steps) * 100).toFixed(1)}%`);
  return `linear-gradient(to right, ${parts.join(", ")})`;
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

/** the colour scale a field is shown with */
export function scaleFor(f: Pick<FieldInfo, "name" | "min" | "max" | "scale" | "diverging">): ColorScale {
  if (isSafety(f.name)) return { map: "safety", min: 0, max: SAFETY_MAX, log: false, diverging: false };
  if (f.diverging) {
    const m = Math.max(Math.abs(f.min), Math.abs(f.max)) || 1;
    return { map: "diverging", min: -m, max: m, log: false, diverging: true };
  }
  const log = f.scale === "log" && f.min > 0 && f.max > f.min;
  const map: ColormapName = /disp|deform|bend/i.test(f.name) ? "viridis" : "turbo";
  return { map, min: f.min, max: f.max > f.min ? f.max : f.min + 1, log, diverging: false };
}

export const isSafety = (name: string) => /safety/i.test(name);

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
  if (s.map === "safety") return [0, 1, 2, 3, SAFETY_MAX];
  if (s.log) {
    const out: number[] = [];
    for (let e = Math.ceil(Math.log10(s.min)); e <= Math.floor(Math.log10(s.max)); e++) out.push(10 ** e);
    return out.length >= 2 ? out : [s.min, s.max];
  }
  const out: number[] = [];
  for (let i = 0; i <= count; i++) out.push(s.min + ((s.max - s.min) * i) / count);
  return out;
}
