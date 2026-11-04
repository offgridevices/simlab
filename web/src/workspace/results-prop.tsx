// Propeller results: push and power against speed, efficiency in forward
// flight, the blade's shape as SimLab read it, and a way to check the blade
// will not break at a chosen speed.
import { useMemo, useState } from "react";
import { ShieldCheckIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { n } from "../lib/format";
import type { Model, PropSummary, Run } from "../lib/types";
import { LineChart, type Series } from "../components/charts";
import { Details, Stat } from "../components/section";
import { useScene } from "./scene-state";
import { HowSure } from "./common";
import { presetSpin, type Preset } from "./presets";
import { si } from "../lib/scales";


export function PropResults({ run, summary: s, model, onStrength }: { run: Run; summary: PropSummary; model: Model | undefined; onStrength(p: Preset): void }) {
  const cfg = run.config.kind === "prop" ? run.config : null;
  useScene({
    source: { kind: "model", modelId: model?.id },
    spin: cfg ? { center: cfg.center_mm, axis: cfg.axis, radius: s.radius_mm } : null,
    hoverable: false,
  });
  const speeds = useMemo(() => [...new Set(s.points.map((p) => p.airspeed_m_s))].sort((a, b) => a - b), [s.points]);
  const series = (y: (p: PropSummary["points"][number]) => number | null | undefined, only?: (v: number) => boolean): Series[] =>
    speeds
      .filter((v) => !only || only(v))
      .map((v) => {
        const i = speeds.indexOf(v);
        const pts = s.points.filter((p) => p.airspeed_m_s === v && y(p) !== null && y(p) !== undefined);
        return { name: v === 0 ? "Hover" : `${n(v)} m/s`, xs: pts.map((p) => p.rpm), ys: pts.map((p) => y(p)!), color: (i % 5) + 1 };
      });
  const hover = s.points.filter((p) => p.airspeed_m_s === 0);
  const top = (hover.length ? hover : s.points).at(-1);
  const rpms = useMemo(() => [...new Set(s.points.map((p) => p.rpm))].sort((a, b) => a - b), [s.points]);
  const [rpm, setRpm] = useState(() => rpms.at(-1) ?? 25000);
  const forward = speeds.some((v) => v > 0);

  return (
    <div className="flex flex-col gap-4" data-testid="prop-results">
      <Card className="gap-1 border-plum/40 bg-plum/10 p-4">
        <div className="flex items-baseline justify-between gap-3">
          <span className="text-xl font-semibold tracking-tight text-plum-text">Push and power</span>
          {top && <span className="num text-xl font-semibold text-plum-text">{n(top.thrust_g)} g</span>}
        </div>
        <span className="text-soft">{s.headline}</span>
      </Card>
      {top && (
        <div className="grid grid-cols-2 gap-2">
          <Stat label={`Push at ${si(top.rpm)}rpm`} value={n(top.thrust_g)} unit="g" />
          <Stat label="Power it takes" value={n(top.power_w)} unit="W" />
          <Stat label="Grams per watt" value={top.power_w > 0 ? n(top.thrust_g / top.power_w, 3) : "–"} hint="Higher is better" />
          <Stat label="Blades · across" value={`${s.blades} · ${n(s.radius_mm * 2)}`} unit="mm" />
        </div>
      )}
      <HowSure accuracy={s.accuracy} facts={[`${s.points.length} working points`, `${s.blades} blades`, `${s.geometry.length} blade sections read`]} />
      <LineChart series={series((p) => p.thrust_g)} height={170} fmtX={(v) => si(v)} fmtY={(v) => `${n(v)} g`} label="Push in grams against speed in rpm" yLabel="Push (g)" xLabel="rpm" />
      <LineChart series={series((p) => p.power_w)} height={150} fmtX={(v) => si(v)} fmtY={(v) => `${n(v)} W`} label="Power in watts against speed in rpm" yLabel="Power (W)" xLabel="rpm" />
      {forward ? (
        <LineChart
          series={series((p) => (p.efficiency ?? null) as number | null, (v) => v > 0).map((x) => ({ ...x, ys: x.ys.map((v) => v * 100) }))}
          height={150}
          fmtX={(v) => si(v)}
          fmtY={(v) => `${n(v, 2)}%`}
          label="Efficiency in forward flight against speed"
          yLabel="Efficiency in forward flight (%)"
          xLabel="rpm"
        />
      ) : (
        <LineChart
          series={series((p) => (p.power_w > 0 ? p.thrust_g / p.power_w : null), (v) => v === 0)}
          height={150}
          fmtX={(v) => si(v)}
          fmtY={(v) => `${n(v, 3)} g/W`}
          label="Grams of push per watt while hovering"
          yLabel="Hover efficiency (g per W)"
          xLabel="rpm"
        />
      )}

      <Card className="gap-2 p-3">
        <span className="text-sm font-medium">Will the blades hold at this speed?</span>
        <Select value={String(rpm)} onValueChange={(v) => setRpm(Number(v))}>
          <SelectTrigger className="w-auto" aria-label="Speed to test" data-testid="strength-rpm">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {rpms.map((r) => (
              <SelectItem key={r} value={String(r)}>
                <span className="num">{Math.round(r).toLocaleString("en-US")} rpm</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button onClick={() => model && onStrength(presetSpin(model, rpm, speeds[0] ?? 0))} disabled={!model} data-testid="test-strength-at-rpm">
          <ShieldCheckIcon />
          Test strength at this speed
        </Button>
        <span className="text-xs text-muted-foreground">Sets up a strength test with the spin and the air pushing on the blades, held at the hub.</span>
      </Card>

      <Details summary="Blade shape, as SimLab read it">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>From the centre</TableHead>
              <TableHead className="text-right">Width (chord)</TableHead>
              <TableHead className="text-right">Twist (pitch)</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {s.geometry.map((g) => (
              <TableRow key={g.r_mm}>
                <TableCell className="num">{n(g.r_mm)} mm</TableCell>
                <TableCell className="num text-right">{n(g.chord_mm)} mm</TableCell>
                <TableCell className="num text-right">{n(g.pitch_deg)}°</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        <p className="pt-2 text-xs text-muted-foreground">
          Hub {n(s.hub_radius_mm * 2)} mm across · turns {s.rotation === "ccw" ? "counter-clockwise" : "clockwise"} seen from the side it pushes toward
        </p>
      </Details>
    </div>
  );
}
