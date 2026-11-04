// A run on its way: how far along, in plain words, a live chart of whatever
// the solver reports, and a Stop button.
import { useState } from "react";
import { SquareIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useApp } from "../lib/app";
import { ago, n } from "../lib/format";
import type { Run } from "../lib/types";
import { LineChart, Meter } from "../components/charts";
import { si } from "../lib/scales";

export const METRIC: Record<string, { label: string; log?: boolean; unit?: string }> = {
  residual: { label: "How far from settled (smaller is better)", log: true },
  drag_n: { label: "Drag", unit: "N" },
  thrust_n: { label: "Thrust", unit: "N" },
  thrust_g: { label: "Thrust", unit: "g" },
  lift_n: { label: "Lift", unit: "N" },
};
const ORDER = ["thrust_n", "drag_n", "thrust_g", "residual"];

export function pickMetric(metrics: Run["metrics"]): string | null {
  const names = Object.keys(metrics).filter((k) => (metrics[k]?.length ?? 0) > 1);
  return ORDER.find((k) => names.includes(k)) ?? names[0] ?? null;
}

export function MetricChart({ run, name, height = 150 }: { run: Run; name: string; height?: number }) {
  const pts = run.metrics[name] ?? [];
  const m = METRIC[name] ?? { label: name.replace(/_/g, " ") };
  return (
    <LineChart
      series={[{ name: m.label, xs: pts.map((p) => p[0]), ys: pts.map((p) => p[1]), tone: name === "residual" ? "plum" : "sky" }]}
      yLog={m.log}
      height={height}
      fmtX={(v) => si(v)}
      fmtY={(v) => (m.log ? (v >= 0.001 ? n(v) : v.toExponential(0)) : `${n(v, 3)}${m.unit ? ` ${m.unit}` : ""}`)}
      label={`${m.label} as the run goes on`}
      yLabel={`${m.label}${m.unit ? ` (${m.unit})` : ""}`}
      xLabel="step"
    />
  );
}

export function RunningView({ run }: { run: Run }) {
  const { api } = useApp();
  const [stopping, setStopping] = useState(false);
  const metric = pickMetric(run.metrics);
  const waiting = run.status === "queued";
  return (
    <div className="flex flex-col gap-4" data-testid="running" aria-live="polite">
      <div className="flex flex-col gap-3 rounded-xl border border-ember/30 bg-ember/[0.05] p-4">
        <div className="flex items-center justify-between gap-2">
          <Badge tone={waiting ? "neutral" : "ember"}>{waiting ? "Waiting for its turn" : "Running"}</Badge>
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
