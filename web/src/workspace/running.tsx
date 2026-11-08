// A run on its way: how far along, in plain words, a live chart of whatever
// the solver reports, and a Stop button. The 3D view meanwhile shows the test
// itself: the propeller spinning, air streaming past or through it, the
// loads pushing, with how far along over the view.
import { useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { SquareIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useApp } from "../lib/app";
import { ago, engineLabel, n } from "../lib/format";
import type { Model, Run, Vec3 } from "../lib/types";
import type { LiveProps, ToneArrow } from "../viewer/viewer";
import { useScene, useSceneCtl } from "./scene-state";
import { modelCentre } from "./presets";
import { loadArrows } from "./strength-form";
import { unit } from "./common";
import { LineChart, Meter } from "../components/charts";
import { si } from "../lib/scales";

export const METRIC: Record<string, { label: string; log?: boolean; unit?: string }> = {
  residual: { label: "How far from settled (smaller is better)", log: true },
  drag_n: { label: "Drag", unit: "N" },
  thrust_n: { label: "Thrust", unit: "N" },
  thrust_g: { label: "Thrust", unit: "g" },
  lift_n: { label: "Lift", unit: "N" },
  side_n: { label: "Side force", unit: "N" },
  torque_nm: { label: "Twist (torque)", unit: "N·m" },
  residual_U: { label: "Air speed: how far from settled (smaller is better)", log: true },
  residual_p: { label: "Pressure: how far from settled (smaller is better)", log: true },
};
const ORDER = ["thrust_n", "drag_n", "thrust_g", "residual"];

export function pickMetric(metrics: Run["metrics"]): string | null {
  const names = Object.keys(metrics).filter((k) => (metrics[k]?.length ?? 0) > 1);
  return ORDER.find((k) => names.includes(k)) ?? names[0] ?? null;
}

export function MetricChart({ run, name, height = 150, compact }: { run: Run; name: string; height?: number; compact?: boolean }) {
  const pts = run.metrics[name] ?? [];
  const m = METRIC[name] ?? { label: name.replace(/_/g, " ") };
  return (
    <LineChart
      series={[{ name: m.label, xs: pts.map((p) => p[0]), ys: pts.map((p) => p[1]), color: name.startsWith("residual") ? 3 : 1 }]}
      yLog={m.log}
      height={height}
      fmtX={(v) => si(v)}
      fmtY={(v) => (m.log ? (v >= 0.001 ? n(v) : v.toExponential(0)) : `${n(v, 3)}${m.unit ? ` ${m.unit}` : ""}`)}
      label={`${m.label} as the run goes on`}
      yLabel={compact ? undefined : `${m.label}${m.unit ? ` (${m.unit})` : ""}`}
      xLabel="step"
    />
  );
}

const neg = (v: Vec3): Vec3 => [-v[0], -v[1], -v[2]];

/** what the 3D view shows while this run goes: the test as it happens (drawn, not the solver's numbers) */
export function liveScene(run: Run, model: Model | undefined): { live: LiveProps; arrows: ToneArrow[]; spin: { center: Vec3; axis: Vec3; radius: number } | null } {
  const c = run.config;
  const R = model?.size_mm ? Math.max(...model.size_mm) / 2 : 50;
  const g = model?.suggestions?.prop;
  const centre = model ? modelCentre(model) : ([0, 0, 0] as Vec3);
  if (c.kind === "prop") {
    const radius = g?.radius_mm ?? R;
    // the prop pushes toward its axis, so the air it moves goes the other way
    return { live: { spin: { center: c.center_mm, axis: c.axis }, flow: { kind: "through", dir: neg(unit(c.axis)), center: c.center_mm, radius } }, arrows: [], spin: null };
  }
  if (c.kind === "wind") {
    const spin = c.spin && c.spin.rpm > 0 ? { center: c.spin.center_mm, axis: c.spin.axis } : null;
    const flow = c.speed_m_s > 0 ? { kind: "around" as const, dir: unit(c.wind_dir), center: centre, radius: R } : spin ? { kind: "through" as const, dir: neg(unit(spin.axis)), center: spin.center, radius: g?.radius_mm ?? R } : null;
    return { live: { spin, flow }, arrows: [], spin: null };
  }
  const spinning = c.loads.find((l): l is Extract<typeof l, { type: "spin" | "prop_aero" }> => l.type === "spin" || l.type === "prop_aero");
  const air = c.loads.find((l): l is Extract<typeof l, { type: "prop_aero" }> => l.type === "prop_aero");
  const radius = g?.radius_mm ?? R * 0.9;
  return {
    live: {
      spin: spinning ? { center: spinning.center_mm, axis: spinning.axis } : null,
      flow: air ? { kind: "through", dir: neg(unit(air.axis)), center: air.center_mm, radius } : null,
      pulse: true,
    },
    arrows: model ? loadArrows(c.loads, model.patches ?? [], c.regions ?? [], model, R) : [],
    spin: spinning ? { center: spinning.center_mm, axis: spinning.axis, radius } : null,
  };
}

export function RunningView({ run, model }: { run: Run; model?: Model }) {
  const { api } = useApp();
  const { overlay } = useSceneCtl();
  const shown = useMemo(() => liveScene(run, model), [JSON.stringify(run.config), model?.id, model?.patches?.length]);
  const going = run.status === "running";
  const heldIds = new Set(run.config.kind === "strength" && run.config.support.type === "fixed" ? run.config.support.patches : []);
  useScene({
    source: { kind: "model", modelId: model?.id },
    live: going ? shown.live : null,
    arrows: shown.arrows,
    spin: shown.spin,
    marks: run.config.kind === "strength" ? [{ ids: [...heldIds], tone: "sky", alpha: 0.7 }] : [],
    regions: run.config.kind === "strength" ? (run.config.regions ?? []).map((r) => ({ shape: r.shape, tone: heldIds.has(r.id) ? ("sky" as const) : ("ember" as const), alpha: 0.55 })) : [],
    hint: going ? "The test as it runs: drawn to show what is being tested, the numbers come at the end" : "Waiting for its turn",
  });
  const [stopping, setStopping] = useState(false);
  const metric = pickMetric(run.metrics);
  const waiting = run.status === "queued";
  return (
    <div className="flex flex-col gap-4" data-testid="running" aria-live="polite">
      {overlay &&
        createPortal(
          <div className="pointer-events-auto flex w-full max-w-sm flex-col gap-1.5 rounded-lg bg-hud px-3 py-2 text-hud-foreground shadow-float" data-testid="live-hud">
            <span className="flex items-center justify-between gap-3 text-xs">
              <span className="truncate text-hud-muted">{waiting ? "Waiting for its turn" : run.message || "Working…"}</span>
              <span className="num font-semibold">{Math.round(run.progress * 100)}%</span>
            </span>
            <Meter value={run.progress} tone="ember" label="How far along" />
          </div>,
          overlay,
        )}
      <div className="flex flex-col gap-3 rounded-xl border border-ember/30 bg-ember/[0.05] p-4">
        <div className="flex items-center justify-between gap-2">
          <span className="flex min-w-0 flex-wrap items-center gap-2">
            <Badge tone={waiting ? "neutral" : "ember"}>{waiting ? "Waiting for its turn" : "Running"}</Badge>
            {run.config.kind !== "prop" && <span className="text-xs text-muted-foreground">{engineLabel(run.config.kind, run.engine)}</span>}
          </span>
          <span className="num text-sm font-semibold">{Math.round(run.progress * 100)}%</span>
        </div>
        <Meter value={run.progress} tone="ember" label="How far along" />
        <p className="text-sm text-soft" data-testid="run-message">
          {waiting ? "Another test is using the solver. This one starts as soon as it finishes." : run.message || "Working…"}
        </p>
        <span className="text-xs text-muted-foreground">Started {ago(run.started_at ?? run.created_at)}. You can leave this page; it keeps going.</span>
      </div>
      {metric ? <MetricChart run={run} name={metric} /> : !waiting && <p className="text-sm text-muted-foreground">A live chart appears here once the solver reports progress.</p>}
      <Button
        variant="outline"
        onClick={async () => {
          setStopping(true);
          try {
            await api.stopRun(run.id);
          } finally {
            setStopping(false);
          }
        }}
        disabled={stopping}
        data-testid="stop"
      >
        <SquareIcon />
        {stopping ? "Stopping…" : "Stop"}
      </Button>
    </div>
  );
}
