// Wind tunnel setup: which way the air blows, how fast, whether the part
// spins (a propeller), and how detailed.
import { Label } from "@/components/ui/label";
import { useMemo, useState } from "react";
import { ArrowDownIcon, ArrowLeftIcon, ArrowRightIcon, ArrowUpIcon, MoveDownLeftIcon, MoveUpRightIcon } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { dirLabel, n } from "../lib/format";
import type { GridQuality, Vec3, WindConfig } from "../lib/types";
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

export function WindSteps({ config: c, set, model, step, setStep, estimate }: StepsProps<WindConfig>) {
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
        <p className="text-xs text-muted-foreground">More detail means smaller air cells: sharper flow pictures and forces, but slower, and more memory on the graphics chip.</p>
        {estimate}
      </Step>
    </Accordion>
  );
}
