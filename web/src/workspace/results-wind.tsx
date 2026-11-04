// Wind tunnel results: the forces in plain numbers, the part coloured by
// air pressure, flow slices you can switch on, and how the force settled.
import { useMemo, useState } from "react";
import { Switch } from "@/components/ui/switch";
import { useApp } from "../lib/app";
import { useAsync, useField } from "../lib/hooks";
import { colorize, scaleFor, type ColorScale } from "../lib/colormaps";
import { duration, G, n } from "../lib/format";
import type { Run, Slice, WindSummary } from "../lib/types";
import { Details, Row, Stat } from "../components/section";
import { Segmented } from "./common";
import { useScene } from "./scene-state";
import { Legend } from "./legend";
import { MetricChart, pickMetric } from "./running";

export function WindResults({ run, summary: s }: { run: Run; summary: WindSummary }) {
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
  const sliceScale: (ColorScale & { label: string; unit: string }) | null = useMemo(() => {
    const list = slices.value;
    if (!list?.length) return null;
    const infos = list.map((x) => x.slice.fields.find((f) => f.name === sliceField) ?? x.slice.fields[0]!);
    const min = Math.min(...infos.map((f) => f.min));
    const max = Math.max(...infos.map((f) => f.max));
    const sc = scaleFor({ name: sliceField, min, max, scale: "linear", diverging: sliceField === "pressure" });
    return { ...sc, label: infos[0]!.label, unit: infos[0]!.unit };
  }, [slices.value, sliceField]);
  const shows = useMemo(
    () => (slices.value && sliceScale ? slices.value.map((x) => ({ key: `${run.id}:${x.slice.name}:${sliceField}:${sliceScale.min}:${sliceScale.max}`, slice: x.slice, rgba: colorize(x.values, sliceScale) })) : []),
    [slices.value, sliceScale, run.id, sliceField],
  );

  useScene({
    source: { kind: "run", runId: run.id },
    field: field.value && surfScale && colour === "pressure" ? { values: field.value, scale: surfScale } : null,
    slices: shows,
    hoverable: false,
  });
  const metric = pickMetric(run.metrics);
  const spin = s.thrust_n !== null && s.thrust_n !== undefined;

  return (
    <div className="flex flex-col gap-4" data-testid="wind-results">
      <div className="flex flex-col gap-1 rounded-xl border border-sky/40 bg-sky/10 p-4">
        <span className="text-xl font-semibold tracking-tight text-sky-text">{spin ? "Thrust and drag" : "Drag"}</span>
        <span className="text-soft">{s.headline}</span>
      </div>
      <div className="grid grid-cols-2 gap-2">
        {spin && (
          <>
            <Stat label="Thrust (push)" value={n(s.thrust_n!)} unit="N" hint={`${n((s.thrust_n! / G) * 1000)} g`} />
            <Stat label="Power" value={n(s.power_w ?? 0)} unit="W" />
            <Stat label="Twist (torque)" value={n(s.torque_nm ?? 0, 3)} unit="N·m" />
          </>
        )}
        <Stat label="Drag" value={n(s.drag_n)} unit="N" hint={`${n((s.drag_n / G) * 1000)} g`} />
        <Stat label="Lift" value={n(s.lift_n)} unit="N" />
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
        <Legend scale={sliceScale} label={`${sliceScale.label} in the slice${on.length > 1 ? "s" : ""}`} unit={sliceScale.unit} />
      ) : colour === "pressure" && surfScale && surfaceField ? (
        <Legend scale={surfScale} label={surfaceField.label} unit={surfaceField.unit} />
      ) : null}

      {run.slices.length > 0 && (
        <div className="flex flex-col gap-2 rounded-lg bg-sunken p-3" data-testid="slices">
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
            <label key={sl.name} className="flex items-center justify-between gap-3 text-sm">
              {sl.label}
              <Switch checked={on.includes(sl.name)} onCheckedChange={(v) => setOn((o) => (v ? [...o, sl.name] : o.filter((x) => x !== sl.name)))} aria-label={sl.label} />
            </label>
          ))}
          {slices.error && <p className="text-xs text-destructive">{slices.error}</p>}
        </div>
      )}

      {metric && <MetricChart run={run} name={metric} />}

      {s.warnings.length > 0 && (
        <ul className="flex flex-col gap-1.5">
          {s.warnings.map((w) => (
            <li key={w} className="rounded-lg bg-gold/10 px-3 py-2 text-sm text-gold-text">
              {w}
            </li>
          ))}
        </ul>
      )}
      <Details summary="Details">
        <Row label="Force x, y, z">{s.force_n.map((v) => n(v, 3)).join(", ")} N</Row>
        <Row label="Moment x, y, z">{s.moment_nm.map((v) => n(v, 3)).join(", ")} N·m</Row>
        <Row label="Frontal area">{n(s.ref_area_mm2)} mm²</Row>
        <Row label="Reynolds number">{n(s.reynolds)}</Row>
        <Row label="Air cells">{n(s.cells)}</Row>
        <Row label="Time steps">{n(s.steps)}</Row>
        <Row label="Speed">{n(s.mlups)} million cell updates/s</Row>
        <Row label="Took">{duration(s.seconds)}</Row>
        <Row label="Settled">{s.converged ? "Yes" : "Not fully: treat the numbers as rough"}</Row>
        <Row label="Solved by">{run.engine === "native" ? "SimLab (GPU)" : run.engine}</Row>
      </Details>
    </div>
  );
}
