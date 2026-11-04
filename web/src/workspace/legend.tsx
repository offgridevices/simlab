// The colour legend drawn over the 3D view: the scale, round ticks in mono,
// the unit, and markers where this result's own lowest and highest values
// fall. The safety scale also names its three bands. It redraws when the
// theme changes (the safety colours come from the tokens).
import { createPortal } from "react-dom";
import { cssGradient, legendTicks, normalize, SAFETY_STEPS, type ColorScale } from "../lib/colormaps";
import { n } from "../lib/format";
import { useResolvedTheme } from "../lib/theme";
import { useSceneCtl } from "./scene-state";

/** a label at x (0..1) that stays inside the bar: left-aligned near the left end, right-aligned near the right */
const place = (x: number): React.CSSProperties =>
  x < 0.2 ? { left: `${x * 100}%` } : x > 0.8 ? { right: `${(1 - x) * 100}%` } : { left: `${x * 100}%`, transform: "translateX(-50%)" };

const fmt = (v: number, unit: string) => (unit === "×" ? `${n(v, 2)}×` : n(v, 3));

export function Legend({ scale, label, unit, min, max }: { scale: ColorScale; label: string; unit: string; min?: number; max?: number }) {
  const { overlay } = useSceneCtl();
  useResolvedTheme(); // re-render on theme change: the gradient reads the tokens
  if (!overlay) return null;
  const ticks = legendTicks(scale);
  const safety = scale.map === "safety";
  const marks = [
    { v: min, word: "min" },
    { v: max, word: "max" },
  ].filter((m): m is { v: number; word: string } => m.v !== undefined && Number.isFinite(m.v));
  return createPortal(
    <div className="pointer-events-auto w-full max-w-sm rounded-lg border bg-hud px-3 py-2 text-hud-foreground shadow-float backdrop-blur" data-testid="legend">
      <div className="mb-1 flex items-baseline justify-between gap-2 text-xs">
        <span className="font-medium">{label}</span>
        {unit && unit !== "×" && <span className="num text-hud-muted">{unit}</span>}
      </div>
      {/* where this result's lowest and highest values sit on the scale; one label when they sit close */}
      <div className="relative mb-0.5 h-3.5 text-2xs text-hud-muted">
        {marks.map((m) => (
          <span key={m.word} className="absolute bottom-0 h-0 w-0 -translate-x-1/2 border-x-4 border-t-4 border-x-transparent border-t-hud-foreground" style={{ left: `${normalize(m.v, scale) * 100}%` }} aria-hidden="true" />
        ))}
        {marks.length === 2 && Math.abs(normalize(marks[1]!.v, scale) - normalize(marks[0]!.v, scale)) < 0.4 ? (
          <span className="num absolute -top-0.5 whitespace-nowrap leading-none" style={place((normalize(marks[0]!.v, scale) + normalize(marks[1]!.v, scale)) / 2)}>
            min {fmt(marks[0]!.v, unit)} · max {fmt(marks[1]!.v, unit)}
          </span>
        ) : (
          marks.map((m) => (
            <span key={m.word} className="num absolute -top-0.5 whitespace-nowrap leading-none" style={place(normalize(m.v, scale))}>
              {m.word} {fmt(m.v, unit)}
            </span>
          ))
        )}
      </div>
      <div className="h-2.5 rounded-xs" style={{ background: cssGradient(scale.map) }} aria-hidden="true" />
      <div className="relative mt-0.5 h-4 text-2xs text-hud-muted">
        {ticks.map((t, i) => {
          const x = normalize(t, scale) * 100;
          return (
            <span key={i} className="num absolute top-0 flex -translate-x-1/2 flex-col items-center leading-none" style={{ left: `${x}%` }}>
              <span className="h-1 w-px bg-hud-muted" aria-hidden="true" />
              {safety && t === scale.max ? `${t}×+` : safety ? `${t}×` : n(t, 3)}
            </span>
          );
        })}
      </div>
      {safety && (
        <div className="relative mt-0.5 h-3.5 text-2xs font-medium">
          <span className="absolute left-0 text-clay-text">Breaks</span>
          <span className="absolute -translate-x-1/2 text-gold-text" style={{ left: `${(((SAFETY_STEPS[0] + SAFETY_STEPS[1]) / 2) / scale.max) * 100}%` }}>
            Marginal
          </span>
          <span className="absolute right-0 text-moss-text">Safe</span>
        </div>
      )}
    </div>,
    overlay,
  );
}
