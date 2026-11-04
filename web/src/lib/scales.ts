// The few scales the hand-drawn charts need (no chart library: the Tests
// view stays small): linear and log, with ticks at round numbers.

export interface Scale {
  (v: number): number;
  domain: [number, number];
  range: [number, number];
  invert(px: number): number;
}

export function linear(domain: [number, number], range: [number, number]): Scale {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  const k = d1 === d0 ? 0 : (r1 - r0) / (d1 - d0);
  const f = ((v: number) => r0 + (v - d0) * k) as Scale;
  f.domain = domain;
  f.range = range;
  f.invert = (px) => (k ? d0 + (px - r0) / k : d0);
  return f;
}

export function log(domain: [number, number], range: [number, number]): Scale {
  const l = linear([Math.log10(domain[0]), Math.log10(domain[1])], range);
  const f = ((v: number) => l(Math.log10(Math.max(v, Number.MIN_VALUE)))) as Scale;
  f.domain = domain;
  f.range = range;
  f.invert = (px) => 10 ** l.invert(px);
  return f;
}

/** about `count` ticks at 1, 2 or 5 times a power of ten, covering the domain */
export function niceTicks(lo: number, hi: number, count = 5): number[] {
  if (!(hi > lo)) return [lo];
  const raw = (hi - lo) / Math.max(1, count);
  const p = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 5, 10].map((m) => m * p).find((s) => s >= raw) ?? 10 * p;
  const out: number[] = [];
  for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + step * 1e-9; v += step) out.push(Math.round(v / step) * step);
  // floating point: 0.30000000000000004 -> 0.3
  return out.map((v) => Number(v.toPrecision(12)));
}

/** a domain widened to the round ticks around it */
export function niceDomain(lo: number, hi: number, count = 5): [number, number] {
  const t = niceTicks(lo, hi, count);
  const step = t.length > 1 ? t[1]! - t[0]! : 1;
  const round = (v: number) => Number(v.toPrecision(12));
  return [round(t[0]! > lo ? t[0]! - step : t[0]!), round(t.at(-1)! < hi ? t.at(-1)! + step : t.at(-1)!)];
}

/** decades (1, 10, 100…) inside a log domain */
export function decades(lo: number, hi: number): number[] {
  const out: number[] = [];
  for (let e = Math.ceil(Math.log10(lo) - 1e-9); e <= Math.floor(Math.log10(hi) + 1e-9); e++) out.push(10 ** e);
  return out;
}

/** a line through points, as an SVG path */
export function linePath(xs: number[], ys: number[], x: Scale, y: Scale): string {
  let d = "";
  for (let i = 0; i < xs.length; i++) d += `${i ? "L" : "M"}${x(xs[i]!).toFixed(1)} ${y(ys[i]!).toFixed(1)}`;
  return d;
}

/** the index of the sorted value nearest v */
export function nearest(sorted: number[], v: number): number {
  let lo = 0;
  let hi = sorted.length - 1;
  if (hi < 0) return -1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid]! < v) lo = mid;
    else hi = mid;
  }
  return Math.abs(sorted[lo]! - v) <= Math.abs(sorted[hi]! - v) ? lo : hi;
}

const SI = [
  [1e9, "G"],
  [1e6, "M"],
  [1e3, "k"],
  [1, ""],
  [1e-3, "m"],
  [1e-6, "µ"],
] as const;
/** 1000 -> "1 k", 0.01 -> "10 m": axis labels with SI prefixes */
export function si(v: number, unit = ""): string {
  if (v === 0) return `0${unit ? " " + unit : ""}`;
  const [k, p] = SI.find(([k]) => Math.abs(v) >= k * 0.9999) ?? SI.at(-1)!;
  const n = v / k;
  const s = Number(n.toPrecision(3)).toString();
  return `${s} ${p}${unit}`.trimEnd();
}
