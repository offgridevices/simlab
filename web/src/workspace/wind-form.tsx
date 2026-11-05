// Wind tunnel setup: which way the air blows, how fast, whether the part
// spins (a propeller), and how detailed.
import { Label } from "@/components/ui/label";
import { useMemo, useState } from "react";
import { ArrowDownIcon, ArrowLeftIcon, ArrowRightIcon, ArrowUpIcon, GaugeIcon, MoveDownLeftIcon, MoveUpRightIcon, ZapIcon } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { dirLabel, duration, n } from "../lib/format";
import type { Engine, Estimate, GridQuality, Vec3, WindConfig } from "../lib/types";
import { AxisPicker, NumberField, Segmented, Step, unit } from "./common";
import { useScene } from "./scene-state";
import { Accordion } from "@/components/ui/accordion";
import { modelCentre } from "./presets";
import type { StepsProps } from "./study-form";
import type { ToneArrow } from "../viewer/viewer";

export function windProblems(c: WindConfig): string[] {
  const out: string[] = [];
  if (!(Math.hypot(...c.wind_dir) > 0)) out.push("Pick which way the air blows (step 1).");
  if (c.speed_m_s <= 0 && !(c.spin && c.spin.rpm > 0)) out.push("Set a wind speed, or make it spin (step 2 or 3).");
  if (c.speed_m_s > 120) out.push("SimLab's wind tunnel goes up to 120 m/s.");
  return out;
}

// directions in plain words, as seen in the default three-quarter view (front is −Y)
const FROM: { label: string; dir: Vec3; icon: React.ReactNode }[] = [
  { label: "From the front", dir: [0, 1, 0], icon: <MoveUpRightIcon /> },
  { label: "From behind", dir: [0, -1, 0], icon: <MoveDownLeftIcon /> },
  { label: "From the left", dir: [1, 0, 0], icon: <ArrowRightIcon /> },
  { label: "From the right", dir: [-1, 0, 0], icon: <ArrowLeftIcon /> },
  { label: "From above", dir: [0, 0, -1], icon: <ArrowDownIcon /> },
  { label: "From below", dir: [0, 0, 1], icon: <ArrowUpIcon /> },
];
const same = (a: Vec3, b: Vec3) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) < 1e-6;

/** the two ways to run an air test, in the words the app uses */
export const WIND_ENGINES: { id: Engine; label: string; icon: React.ReactNode; detail: string }[] = [
  {
    id: "reference",
    label: "Accurate",
    icon: <GaugeIcon />,
    detail: "OpenFOAM in Docker, on the processor. The numbers to trust: propeller thrust within about 3%, drag on simple shapes within about 1%.",
  },
  {
    id: "native",
    label: "Quick preview",
    icon: <ZapIcon />,
    detail: "On the graphics chip, for a first look at the flow. Drag within about 3% on simple shapes; a spinning part's thrust can be 30–60% off, far more at Draft.",
  },
];

function timeFor(est: Estimate | null, e: Engine): string {
  const x = est?.engines?.[e];
  if (!x) return est ? "" : "…";
  if (x.available === false) return "can't run";
  return `about ${duration(x.seconds)}`;
}

export function WindSteps({ config: c, set, model, step, setStep, estimate, engine, setEngine, estimates }: StepsProps<WindConfig>) {
  const [kmh, setKmh] = useState(false);
  const prop = model.suggestions?.prop;
  const R = model.size_mm ? Math.max(...model.size_mm) / 2 : 50;
  const centre = modelCentre(model);
  const arrows = useMemo<ToneArrow[]>(() => {
    if (c.speed_m_s <= 0) return [];
    const d = unit(c.wind_dir);
    // a few arrows upstream of the part, pointing the way the air moves
    const a: Vec3 = Math.abs(d[2]) < 0.9 ? [0, 0, 1] : [1, 0, 0];
    const b: Vec3 = unit([d[1] * a[2] - d[2] * a[1], d[2] * a[0] - d[0] * a[2], d[0] * a[1] - d[1] * a[0]]);
    const a2: Vec3 = unit([b[1] * d[2] - b[2] * d[1], b[2] * d[0] - b[0] * d[2], b[0] * d[1] - b[1] * d[0]]);
    const out: ToneArrow[] = [];
    for (const i of [-1, 0, 1])
      for (const j of [-1, 0, 1]) {
        const o: Vec3 = [0, 1, 2].map((k) => centre[k]! - d[k]! * R * 1.05 + (b[k]! * i + a2[k]! * j) * R * 0.35) as Vec3;
        out.push({ origin: o, dir: d, tone: "sky", length: R * 0.35 });
      }
    return out;
  }, [c.wind_dir, c.speed_m_s, R, centre[0], centre[1], centre[2]]);
  useScene({
    source: { kind: "model", modelId: model.id },
    arrows,
    spin: c.spin && c.spin.rpm > 0 ? { center: c.spin.center_mm, axis: c.spin.axis, radius: prop?.radius_mm ?? R * 0.9 } : null,
    hint: c.speed_m_s > 0 ? `Air moves toward ${dirLabel(c.wind_dir)} at ${n(c.speed_m_s)} m/s` : null,
  });
  const preset = FROM.find((f) => same(f.dir, unit(c.wind_dir)));
  const speed = kmh ? c.speed_m_s * 3.6 : c.speed_m_s;

  return (
    <Accordion type="single" collapsible value={String(step)} onValueChange={(v) => setStep(Number(v) || 0)} className="flex flex-col gap-2">
      <Step n={1} title="Which way does the air blow?" summary={preset?.label ?? `Toward ${dirLabel(c.wind_dir)}`} done testId="step-wind-dir">
        <ToggleGroup
          type="single"
          aria-label="Where the wind comes from"
          className="grid h-auto w-full grid-cols-2 gap-1 min-[420px]:grid-cols-3"
          value={preset?.label ?? ""}
          onValueChange={(v) => {
            const f = FROM.find((x) => x.label === v);
            if (f) set({ ...c, wind_dir: f.dir });
          }}
        >
          {FROM.map((f) => (
            <ToggleGroupItem key={f.label} value={f.label} className="h-9 justify-start px-2.5 text-xs">
              {f.icon}
              {f.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <span className="text-xs text-muted-foreground">Or the exact way the air moves:</span>
        <AxisPicker label="Air moves toward" value={c.wind_dir} onChange={(v) => set({ ...c, wind_dir: v })} />
      </Step>

      <Step n={2} title="How fast?" summary={`${n(c.speed_m_s)} m/s (${n(c.speed_m_s * 3.6)} km/h)`} done testId="step-wind-speed">
        <div className="flex flex-wrap items-center gap-3">
          <NumberField label="Wind speed" value={Number(speed.toFixed(2))} min={0} onChange={(v) => set({ ...c, speed_m_s: kmh ? v / 3.6 : v })} testId="wind-speed" />
          <Segmented
            label="Units"
            value={kmh ? "kmh" : "ms"}
            onChange={(v) => setKmh(v === "kmh")}
            options={[
              { id: "ms", label: "m/s" },
              { id: "kmh", label: "km/h" },
            ]}
          />
        </div>
        <div className="flex flex-wrap gap-1.5">
          {[
            { label: "Breezy hover", v: 5 },
            { label: "Cruising", v: 15 },
            { label: "Racing", v: 30 },
          ].map((p) => (
            <Button key={p.label} variant="outline" size="xs" className="rounded-full" onClick={() => set({ ...c, speed_m_s: p.v })}>
              {p.label} · <span className="num">{p.v} m/s</span>
            </Button>
          ))}
        </div>
      </Step>

      <Step n={3} title="Does it spin?" summary={c.spin && c.spin.rpm > 0 ? `${n(c.spin.rpm)} rpm about ${dirLabel(c.spin.axis)}` : "No"} done testId="step-wind-spin">
        <Label className="flex items-center justify-between gap-3 text-sm">
          <span>
            It spins, like a propeller
            <span className="block text-xs text-muted-foreground">Then SimLab also measures its push (thrust), twist and power.</span>
          </span>
          <Switch
            checked={!!c.spin}
            onCheckedChange={(on) => set({ ...c, spin: on ? { rpm: 20000, axis: prop?.axis ?? [0, 0, 1], center_mm: prop?.center_mm ?? centre } : null })}
            aria-label="It spins"
            data-testid="wind-spin"
          />
        </Label>
        {c.spin && (
          <>
            <NumberField label="Spin speed" value={c.spin.rpm} unit="rpm" min={0} step={500} onChange={(v) => set({ ...c, spin: { ...c.spin!, rpm: v } })} />
            <span className="text-xs text-muted-foreground">Spins about (right-hand rule: positive turns counter-clockwise looking down the arrow)</span>
            <AxisPicker label="Spin axis" value={c.spin.axis} onChange={(v) => set({ ...c, spin: { ...c.spin!, axis: v } })} />
          </>
        )}
      </Step>

      <Step n={4} title="How detailed?" summary={c.quality.type[0]!.toUpperCase() + c.quality.type.slice(1)} done testId="step-wind-quality">
        <Segmented
          label="Detail"
          value={c.quality.type === "custom" ? "normal" : c.quality.type}
          onChange={(v) => set({ ...c, quality: { type: v } as GridQuality })}
          options={[
            { id: "draft", label: "Draft" },
            { id: "normal", label: "Normal" },
            { id: "fine", label: "Fine" },
          ]}
        />
        <p className="text-xs text-muted-foreground">More detail means smaller air cells: sharper flow pictures and forces, but slower and more memory.</p>
      </Step>

      <Step n={5} title="How sure do you need to be?" summary={engine === "native" ? "Quick preview (GPU)" : "Accurate (OpenFOAM)"} done testId="step-wind-engine">
        <ToggleGroup
          type="single"
          aria-label="Accurate or quick preview"
          className="grid h-auto w-full grid-cols-1 gap-1 min-[420px]:grid-cols-2"
          value={engine}
          onValueChange={(v) => v && setEngine(v as Engine)}
          data-testid="wind-engine"
        >
          {WIND_ENGINES.map((e) => (
            <ToggleGroupItem key={e.id} value={e.id} className="h-auto min-h-11 flex-col items-start gap-0.5 px-3 py-2 text-left" data-testid={`engine-${e.id}`}>
              <span className="flex items-center gap-1.5 text-sm font-medium [&_svg]:size-4">
                {e.icon}
                {e.label}
                {e.id === "reference" && <span className="text-2xs font-normal text-muted-foreground">recommended</span>}
              </span>
              <span className="num text-xs font-normal text-muted-foreground">{timeFor(estimates, e.id)}</span>
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <p className="text-xs text-muted-foreground">{WIND_ENGINES.find((e) => e.id === engine)?.detail}</p>
        {engine === "reference" && estimates?.engines?.reference?.docker_ready === false && (
          <p className="text-xs text-gold-text">Needs Docker (Colima) running. SimLab asks before starting it.</p>
        )}
        {estimate}
      </Step>
    </Accordion>
  );
}
