// Wind tunnel results: the forces in plain numbers, the part coloured by
// air pressure, flow slices you can switch on, and how the force settled.
import { useMemo, useState } from "react";
import { GaugeIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Card } from "@/components/ui/card";
import { Alert } from "@/components/ui/alert";
import { Label } from "@/components/ui/label";
import { useApp } from "../lib/app";
import { useAsync, useField } from "../lib/hooks";
import { colorize, scaleFor, type ColorScale } from "../lib/colormaps";
import { bytes, duration, engineLabel, G, n } from "../lib/format";
import type { Run, Slice, WindSummary } from "../lib/types";
import { Details, Row, Stat } from "../components/section";
import { HowSure, Segmented } from "./common";
import { useScene } from "./scene-state";
import { Legend } from "./legend";
import { MetricChart, pickMetric } from "./running";
import type { Again } from "./start-run";

export function WindResults({ run, summary: s, again }: { run: Run; summary: WindSummary; again?: Again }) {
  const { api } = useApp();
  const surfaceField = run.fields.find((f) => f.name === "pressure") ?? run.fields[0] ?? null;
  const [colour, setColour] = useState<"pressure" | "plain">(surfaceField ? "pressure" : "plain");
  const field = useField(run.id, colour === "pressure" ? surfaceField?.name : null);
  const surfScale = surfaceField ? scaleFor(surfaceField) : null;
  const [on, setOn] = useState<string[]>(() => (run.slices[0] ? [run.slices[0].name] : []));
  const [sliceField, setSliceField] = useState<"speed" | "pressure">("speed");

  // the slices that are switched on, with the chosen field, coloured on one shared scale
  const key = on.length ? `slices:${run.id}:${on.join(",")}:${sliceField}` : null;
  const slices = useAsync<{ slice: Slice; values: Float32Array }[]>(key, () =>
    Promise.all(
      on.map(async (name) => {
        const slice = await api.slice(run.id, name);
        const f = slice.fields.find((x) => x.name === sliceField) ?? slice.fields[0]!;
        return { slice, values: await api.sliceField(run.id, name, f.name) };
      }),
    ),
  );
  const sliceScale: (ColorScale & { label: string; unit: string; dataMin: number; dataMax: number }) | null = useMemo(() => {
    const list = slices.value;
    if (!list?.length) return null;
    const infos = list.map((x) => x.slice.fields.find((f) => f.name === sliceField) ?? x.slice.fields[0]!);
    const min = Math.min(...infos.map((f) => f.min));
    const max = Math.max(...infos.map((f) => f.max));
    const sc = scaleFor({ name: sliceField, min, max, scale: "linear", diverging: sliceField === "pressure" });
    return { ...sc, label: infos[0]!.label, unit: infos[0]!.unit, dataMin: min, dataMax: max };
  }, [slices.value, sliceField]);
  const shows = useMemo(
    () => (slices.value && sliceScale ? slices.value.map((x) => ({ key: `${run.id}:${x.slice.name}:${sliceField}:${sliceScale.min}:${sliceScale.max}`, slice: x.slice, rgba: colorize(x.values, sliceScale) })) : []),
    [slices.value, sliceScale, run.id, sliceField],
  );

  useScene({
    source: { kind: "run", runId: run.id },
    field: field.value && surfScale && colour === "pressure" ? { values: field.value, scale: surfScale } : null,
    probe: surfaceField && colour === "pressure" ? { label: surfaceField.label, unit: surfaceField.unit } : null,
    slices: shows,
    hoverable: false,
  });
  const metric = pickMetric(run.metrics);
  const spin = s.thrust_n !== null && s.thrust_n !== undefined;
  const preview = (s.engine ?? run.engine) === "native";
  // a part spinning in still air: "drag" and "lift" along the wake mean nothing
  const hover = spin && run.config.kind === "wind" && run.config.speed_m_s === 0;
  const iterations = s.iterations ?? (preview ? null : s.steps);

  return (
    <div className="flex flex-col gap-4" data-testid="wind-results">
      {preview && (
        <Alert tone="gold" data-testid="preview-note">
          <span className="flex flex-col gap-2">
            <span>
              <span className="font-semibold">Quick preview, not the answer. </span>
              {s.preview_note ?? (spin ? "Rough picture only: thrust can be 30–90% off (quick previews of a 3-inch prop read 32–50 g where OpenFOAM gets 345 g); use Accurate for numbers." : "Quick preview: drag on rounded parts came within about 7% (a sphere from Re 20 to 100,000), but sharp-edged parts read low on a preview's coarse grid: about 12% on a flat plate, 20–30% on a cube or a thin bar. Lift, side force and anything that depends on thin boundary layers (airfoils, wings, small fins) are less certain, and can be off by 20% or more. Use the OpenFOAM test for numbers you will rely on.")}
            </span>
            {again && (
              <Button size="sm" variant="outline" className="self-start" onClick={() => again.rerun("reference")} disabled={again.starting} data-testid="run-accurate">
                <GaugeIcon />
                {again.starting ? "Starting…" : "Run it accurately (OpenFOAM)"}
              </Button>
            )}
            {again?.setting && <span className="text-xs">{again.setting}</span>}
            {again?.startError && <span className="text-xs text-destructive">{again.startError}</span>}
          </span>
        </Alert>
      )}
      {again?.dialog}
      <Card className="gap-1 border-sky/40 bg-sky/10 p-4">
        <div className="flex items-baseline justify-between gap-3">
          <span className="text-xl font-semibold tracking-tight text-sky-text">{spin ? (preview ? "Thrust (rough)" : "Thrust") : preview ? "Drag (preview)" : "Drag"}</span>
          <span className="num text-xl font-semibold text-sky-text">{spin ? `${n(s.thrust_g ?? (s.thrust_n! / G) * 1000)} g` : `${n(s.drag_n, 3)} N`}</span>
        </div>
        <span className="text-soft">{s.headline}</span>
      </Card>
      <div className="grid grid-cols-2 gap-2">
        {spin && (
          <>
            <Stat label="Thrust (push)" value={n(s.thrust_n!)} unit="N" hint={`${n((s.thrust_n! / G) * 1000)} g`} />
            <Stat label="Power" value={n(s.power_w ?? 0)} unit="W" hint="at the shaft" />
            <Stat label="Twist (torque)" value={n((s.torque_nm ?? 0) * 1000, 3)} unit="mN·m" />
          </>
        )}
        {!hover && (
          <>
            <Stat label="Drag" value={n(s.drag_n)} unit="N" hint={`${n((s.drag_n / G) * 1000)} g`} />
            <Stat label="Lift" value={n(s.lift_n)} unit="N" />
          </>
        )}
        {!spin && <Stat label="Side force" value={n(s.side_n)} unit="N" />}
        {s.cd !== null && s.cd !== undefined && <Stat label="Drag coefficient" value={n(s.cd, 3)} hint="Cd, by the frontal area" />}
      </div>

      {surfaceField && (
        <div className="flex flex-col gap-2">
          <span className="text-xs font-medium text-muted-foreground">Colour the part by</span>
          <Segmented
            label="Colour the part by"
            value={colour}
            onChange={setColour}
            options={[
              { id: "pressure", label: "Air pressure" },
              { id: "plain", label: "Plain" },
            ]}
            testId="wind-colour"
          />
          {colour === "pressure" && <p className="text-xs text-muted-foreground">Red is where the air presses on the part; blue is where it pulls (suction).</p>}
        </div>
      )}
      {sliceScale ? (
        <Legend scale={sliceScale} label={`${sliceScale.label} in the slice${on.length > 1 ? "s" : ""}`} unit={sliceScale.unit} min={sliceScale.dataMin} max={sliceScale.dataMax} />
      ) : colour === "pressure" && surfScale && surfaceField ? (
        <Legend scale={surfScale} label={surfaceField.label} unit={surfaceField.unit} min={surfaceField.min} max={surfaceField.max} />
      ) : null}

      {run.slices.length > 0 && (
        <Card className="gap-2 bg-sunken p-3" data-testid="slices">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-sm font-medium">Flow slices</span>
            <Segmented
              label="Slices show"
              value={sliceField}
              onChange={setSliceField}
              options={[
                { id: "speed", label: "Speed" },
                { id: "pressure", label: "Pressure" },
              ]}
            />
          </div>
          {run.slices.map((sl) => (
            <Label key={sl.name} className="justify-between font-normal">
              {sl.label}
              <Switch checked={on.includes(sl.name)} onCheckedChange={(v) => setOn((o) => (v ? [...o, sl.name] : o.filter((x) => x !== sl.name)))} />
            </Label>
          ))}
          {slices.error && <p className="text-xs text-destructive">{slices.error}</p>}
        </Card>
      )}

      <HowSure
        accuracy={s.accuracy}
        facts={[engineLabel("wind", s.engine ?? run.engine), `${n(s.cells)} cells`, iterations ? `${n(iterations)} iterations` : `${n(s.steps)} steps`, s.converged ? "settled" : "not fully settled", `took ${duration(s.seconds)}`]}
      />
      {metric && <MetricChart run={run} name={metric} />}

      {s.warnings.filter((w) => w !== s.preview_note).map((w) => (
        <Alert key={w} tone="gold">
          {w}
        </Alert>
      ))}
      <Details summary="Details">
        <Row label="Force x, y, z">{s.force_n.map((v) => n(v, 3)).join(", ")} N</Row>
        <Row label="Moment x, y, z">{s.moment_nm.map((v) => n(v, 3)).join(", ")} N·m</Row>
        <Row label="Frontal area">{n(s.ref_area_mm2)} mm²</Row>
        <Row label="Reynolds number">{n(s.reynolds)}</Row>
        {s.rpm ? <Row label="Spin speed">{n(Math.abs(s.rpm))} rpm</Row> : null}
        <Row label="Air cells">{n(s.cells)}</Row>
        {iterations ? <Row label="Solver iterations">{n(iterations)}</Row> : <Row label="Time steps">{n(s.steps)}</Row>}
        {s.mlups ? <Row label="Speed">{n(s.mlups)} million cell updates/s</Row> : null}
        {s.turbulence ? <Row label="Turbulence model">{s.turbulence}</Row> : null}
        {s.y_plus ? <Row label="y+ on the part (min, average, max)">{s.y_plus.map((v) => n(v, 2)).join(", ")}</Row> : null}
        <Row label="Took">{duration(s.seconds)}</Row>
        <Row label="Settled">{s.converged ? "Yes" : "Not fully: treat the numbers as rough"}</Row>
        <Row label="Solved by">{engineLabel("wind", s.engine ?? run.engine)}</Row>
        {s.preview_cells_across ? <Row label="Grid cells across the part">{n(s.preview_cells_across)}</Row> : null}
        {s.case_dir ? (
          <Row label="OpenFOAM case (open case.foam in ParaView)">
            <span className="flex flex-col items-end gap-0.5">
              <span className="num">{s.case_bytes ? bytes(s.case_bytes) : "–"}</span>
              <span className="num break-all text-2xs text-muted-foreground" data-testid="case-dir">
                {s.case_dir}
              </span>
            </span>
          </Row>
        ) : null}
      </Details>
    </div>
  );
}
