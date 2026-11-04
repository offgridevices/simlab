// The colour legend drawn over the bottom of the 3D view: the scale, a few
// values, and for the safety factor the words that matter.
import { createPortal } from "react-dom";
import { cssGradient, legendTicks, normalize, type ColorScale } from "../lib/colormaps";
import { n } from "../lib/format";
import { useSceneCtl } from "./scene-state";

export function Legend({ scale, label, unit }: { scale: ColorScale; label: string; unit: string }) {
  const { overlay } = useSceneCtl();
  if (!overlay) return null;
  const ticks = legendTicks(scale);
  const safety = scale.map === "safety";
  return createPortal(
    <div className="pointer-events-auto w-full max-w-sm rounded-lg border bg-card/95 px-3 py-2 shadow-lg shadow-black/20 backdrop-blur" data-testid="legend">
      <div className="mb-1.5 flex items-baseline justify-between gap-2 text-xs">
        <span className="font-medium">{label}</span>
        {unit && unit !== "×" && <span className="text-muted-foreground">{unit}</span>}
      </div>
      <div className="h-2.5 rounded-full" style={{ background: cssGradient(scale.map) }} aria-hidden="true" />
      <div className="relative mt-1 h-4 text-[10px] text-muted-foreground">
        {ticks.map((t, i) => {
          const x = normalize(t, scale) * 100;
          return (
            <span key={i} className="num absolute -translate-x-1/2 whitespace-nowrap first:translate-x-0 last:-translate-x-full" style={{ left: `${x}%` }}>
              {safety && t === scale.max ? `${t}×+` : safety ? `${t}×` : n(t, 2)}
            </span>
          );
        })}
      </div>
      {safety && (
        <div className="mt-0.5 grid grid-cols-[1fr_1fr_2fr] text-[10px] font-medium">
          <span className="text-clay-text">Breaks</span>
          <span className="text-gold-text">Marginal</span>
          <span className="text-moss-text">Safe</span>
        </div>
      )}
    </div>,
    overlay,
  );
}
