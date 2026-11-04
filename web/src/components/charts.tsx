// Small hand-drawn line charts (adapted from HardHub's): one or more lines,
// round-number axes, and a crosshair that follows the pointer or the arrow
// keys. No chart library. Lines are 2 px, grid and axes are hairlines, text
// is in the text colours, never the line's.
import { useCallback, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent, type RefObject } from "react";
import { decades, linear, linePath, log, nearest, niceDomain, niceTicks, type Scale } from "../lib/scales";
import type { Tone } from "../lib/format";
import { cn } from "@/lib/utils";

/** the element's width, kept current */
export function useWidth<T extends HTMLElement>(): [RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(el.getBoundingClientRect().width);
    const ro = new ResizeObserver(([e]) => setW(e!.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

export interface Series {
  name: string;
  xs: number[];
  ys: number[];
  tone?: Tone;
}

const M = { l: 54, r: 12, t: 10, b: 24 };
const STROKE: Record<Tone, string> = {
  neutral: "var(--soft)",
  ember: "var(--ember)",
  moss: "var(--moss)",
  sky: "var(--sky)",
  gold: "var(--gold)",
  plum: "var(--plum)",
  clay: "var(--clay)",
};
const DEFAULT_TONES: Tone[] = ["sky", "ember", "plum", "moss", "gold"];

export function LineChart({
  series,
  yLog,
  height = 180,
  fmtX,
  fmtY,
  label,
  xLabel,
  yLabel,
}: {
  series: Series[];
  yLog?: boolean;
  height?: number;
  fmtX(v: number): string;
  fmtY(v: number): string;
  label: string;
  xLabel?: string;
  yLabel?: string;
}) {
  const [box, w] = useWidth<HTMLDivElement>();
  const [at, setAt] = useState<number | null>(null);
  const svg = useRef<SVGSVGElement>(null);
  const W = Math.max(160, w);
  const live = series.filter((s) => s.xs.length > 0);
  const first = live[0];
  const allX = live.flatMap((s) => s.xs);
  const allY = live.flatMap((s) => s.ys).filter((v) => Number.isFinite(v) && (!yLog || v > 0));

  const { x, y, yt, xt } = useMemo(() => {
    const x0 = allX.length ? Math.min(...allX) : 0;
    const x1 = allX.length ? Math.max(...allX) : 1;
    let lo = allY.length ? Math.min(...allY) : 0;
    let hi = allY.length ? Math.max(...allY) : 1;
    let y: Scale;
    let yt: number[];
    if (yLog) {
      lo = 10 ** Math.floor(Math.log10(lo || 1e-9));
      hi = 10 ** Math.ceil(Math.log10(hi || 1));
      if (hi <= lo) hi = lo * 10;
      y = log([lo, hi], [height - M.b, M.t]);
      const d = decades(lo, hi);
      yt = d.length > 6 ? d.filter((_, i) => i % Math.ceil(d.length / 5) === 0) : d;
    } else {
      if (hi === lo) hi = lo + 1;
      const d = niceDomain(Math.min(0, lo), hi + (hi - lo) * 0.04, 4);
      y = linear(d, [height - M.b, M.t]);
      yt = niceTicks(d[0], d[1], 4);
    }
    const x = linear([x0, x1 > x0 ? x1 : x0 + 1], [M.l, W - M.r]);
    const xt = niceTicks(x0, x1 > x0 ? x1 : x0 + 1, Math.max(2, Math.floor(W / 90)));
    return { x, y, yt, xt };
    // eslint-style: recomputed when the data or width change
  }, [allX.join(), allY.join(), yLog, height, W]);

  const move = useCallback(
    (e: PointerEvent) => {
      if (!first) return;
      const r = svg.current!.getBoundingClientRect();
      setAt(nearest(first.xs, x.invert(((e.clientX - r.left) / r.width) * W)));
    },
    [first, x, W],
  );
  const key = (e: KeyboardEvent) => {
    if (!first || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
    e.preventDefault();
    const n = first.xs.length;
    setAt((i) => Math.min(n - 1, Math.max(0, (i ?? (e.key === "ArrowLeft" ? n : -1)) + (e.key === "ArrowLeft" ? -1 : 1))));
  };
  const cx = at !== null && first ? x(first.xs[at]!) : null;
  const tone = (s: Series, i: number) => STROKE[s.tone ?? DEFAULT_TONES[i % DEFAULT_TONES.length]!];

  return (
    <div className="flex flex-col gap-1.5">
      {(yLabel || live.length > 1) && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
          {yLabel && <span className="grow">{yLabel}</span>}
          {live.length > 1 &&
            live.map((s, i) => (
              <span key={s.name} className="inline-flex items-center gap-1.5">
                <span className="h-0.5 w-3 rounded-full" style={{ background: tone(s, i) }} />
                {s.name}
              </span>
            ))}
        </div>
      )}
      <div ref={box} className="relative w-full select-none">
        <svg
          ref={svg}
          width="100%"
          height={height}
          viewBox={`0 0 ${W} ${height}`}
          className="block max-w-full touch-pan-y overflow-visible rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
          role="img"
          aria-label={label}
          tabIndex={0}
          onPointerMove={move}
          onPointerDown={move}
          onPointerLeave={() => setAt(null)}
          onKeyDown={key}
          onBlur={() => setAt(null)}
        >
          {yt.map((t) => (
            <g key={t}>
              <line x1={M.l} x2={W - M.r} y1={y(t)} y2={y(t)} stroke="var(--border)" />
              <text x={M.l - 6} y={y(t)} dy="0.32em" textAnchor="end" className="fill-muted-foreground text-[10px] tabular-nums">
                {fmtY(t)}
              </text>
            </g>
          ))}
          <line x1={M.l} x2={W - M.r} y1={height - M.b} y2={height - M.b} stroke="var(--input)" />
          {xt.map((t) => (
            <text key={t} x={x(t)} y={height - M.b + 15} textAnchor="middle" className="fill-muted-foreground text-[10px] tabular-nums">
              {fmtX(t)}
            </text>
          ))}
          {live.map((s, i) => (
            <path key={s.name} d={linePath(s.xs, s.ys.map((v) => (yLog ? Math.max(v, y.domain[0]) : v)), x, y)} fill="none" stroke={tone(s, i)} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
          ))}
          {cx !== null && (
            <g pointerEvents="none">
              <line x1={cx} x2={cx} y1={M.t} y2={height - M.b} stroke="var(--muted-foreground)" strokeOpacity={0.6} />
              {live.map((s, i) => {
                const j = nearest(s.xs, first!.xs[at!]!);
                return j >= 0 ? <circle key={s.name} cx={x(s.xs[j]!)} cy={y(yLog ? Math.max(s.ys[j]!, y.domain[0]) : s.ys[j]!)} r={4} fill={tone(s, i)} stroke="var(--card)" strokeWidth={2} /> : null;
              })}
            </g>
          )}
          <rect x={M.l} y={M.t} width={Math.max(0, W - M.l - M.r)} height={height - M.t - M.b} fill="transparent" />
        </svg>
        {cx !== null && at !== null && first && (
          <div
            className={cn("pointer-events-none absolute top-1 z-10 rounded-md border bg-popover px-2.5 py-1.5 text-xs shadow-lg shadow-black/20", cx > W * 0.6 ? "-translate-x-[calc(100%+10px)]" : "translate-x-[10px]")}
            style={{ left: `${(cx / W) * 100}%` }}
            role="status"
          >
            {live.map((s, i) => {
              const j = nearest(s.xs, first.xs[at]!);
              return (
                <div key={s.name} className="flex items-center gap-1.5 font-semibold tabular-nums text-foreground">
                  {live.length > 1 && <span className="size-2 rounded-full" style={{ background: tone(s, i) }} />}
                  {fmtY(s.ys[j]!)}
                </div>
              );
            })}
            <div className="tabular-nums text-muted-foreground">
              {fmtX(first.xs[at]!)}
              {xLabel ? ` ${xLabel}` : ""}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** a bar: how much of something is used, the limit marked */
export function Meter({ value, tone = "sky", className, label }: { value: number; tone?: "sky" | "gold" | "clay" | "moss" | "ember"; className?: string; label?: string }) {
  const fill = { sky: "bg-sky", gold: "bg-gold", clay: "bg-clay", moss: "bg-moss", ember: "bg-ember" }[tone];
  const track = { sky: "bg-sky/15", gold: "bg-gold/15", clay: "bg-clay/15", moss: "bg-moss/15", ember: "bg-ember/15" }[tone];
  return (
    <span
      className={cn("relative block h-1.5 w-full overflow-hidden rounded-full", track, className)}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(Math.max(0, Math.min(1, value)) * 100)}
    >
      <span className={cn("absolute inset-y-0 left-0 rounded-full transition-[width] duration-300", fill)} style={{ width: `${Math.max(1.5, Math.min(1, value) * 100)}%` }} />
    </span>
  );
}
