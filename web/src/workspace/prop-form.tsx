// Propeller test setup: which way it pushes and where its hub is (filled in
// from what SimLab found), the speeds to sweep, and forward speeds.
import { Label } from "@/components/ui/label";
import { useState } from "react";
import { PlusIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { dirLabel, n } from "../lib/format";
import type { PropConfig, Vec3 } from "../lib/types";
import { AxisPicker, NumberField, Step } from "./common";
import { useScene } from "./scene-state";
import { Accordion } from "@/components/ui/accordion";
import type { StepsProps } from "./study-form";

export function propProblems(c: PropConfig): string[] {
  const out: string[] = [];
  if (!(c.rpm_max > c.rpm_min) || c.rpm_min < 0) out.push("The top speed must be higher than the lowest (step 2).");
  if (!(c.airspeeds_m_s ?? [0]).length) out.push("Add at least one forward speed; 0 is hovering (step 3).");
  if (!(Math.hypot(...c.axis) > 0)) out.push("Pick which way it pushes (step 1).");
  return out;
}

export function PropSteps({ config: c, set, model, step, setStep }: StepsProps<PropConfig>) {
  const g = model.suggestions?.prop;
  const R = g?.radius_mm ?? (model.size_mm ? Math.max(...model.size_mm) / 2 : 50);
  const [extra, setExtra] = useState(10);
  useScene({
    source: { kind: "model", modelId: model.id },
    spin: { center: c.center_mm, axis: c.axis, radius: R },
    arrows: [{ origin: c.center_mm.map((v, k) => v + c.axis[k]! * R * 0.55) as Vec3, dir: c.axis, tone: "plum", length: R * 0.5 }],
    hint: `Pushes toward ${dirLabel(c.axis)}`,
  });
  const speeds = c.airspeeds_m_s ?? [0];
  return (
    <Accordion type="single" collapsible value={String(step)} onValueChange={(v) => setStep(Number(v) || 0)} className="flex flex-col gap-2">
      <Step n={1} title="Which way does it push?" summary={`Toward ${dirLabel(c.axis)}${g ? " (found from the shape)" : ""}`} done testId="step-prop-axis">
        {g && <p className="text-sm text-soft">SimLab found a {g.blades}-blade propeller {n(g.radius_mm * 2)} mm across and filled this in. Change it if it pushes the other way.</p>}
        <AxisPicker label="Thrust direction" value={c.axis} onChange={(v) => set({ ...c, axis: v })} />
        <span className="text-xs text-muted-foreground">Hub centre (mm)</span>
        <div className="flex flex-wrap items-center gap-2">
          {(["x", "y", "z"] as const).map((k, j) => (
            <span key={k} className="inline-flex items-center gap-1 text-xs text-muted-foreground">
              {k.toUpperCase()}
              <NumberField label={`Hub centre ${k}`} width="w-16" value={c.center_mm[j]!} onChange={(v) => set({ ...c, center_mm: c.center_mm.map((x, q) => (q === j ? v : x)) as Vec3 })} />
            </span>
          ))}
        </div>
        <Label className="flex flex-wrap items-center gap-2 text-sm">
          Blades
          <NumberField label="Blades" value={c.blades ?? g?.blades ?? 2} min={1} max={8} step={1} width="w-16" onChange={(v) => set({ ...c, blades: Math.round(v) })} />
        </Label>
      </Step>

      <Step n={2} title="How fast does it spin?" summary={`${n(c.rpm_min)} to ${n(c.rpm_max)} rpm, ${c.rpm_steps ?? 12} steps`} done testId="step-prop-rpm">
        <div className="flex flex-wrap items-center gap-2 text-sm">
          From
          <NumberField label="Lowest speed" value={c.rpm_min} min={0} step={1000} onChange={(v) => set({ ...c, rpm_min: v })} testId="rpm-min" />
          to
          <NumberField label="Top speed" value={c.rpm_max} min={0} step={1000} unit="rpm" onChange={(v) => set({ ...c, rpm_max: v })} testId="rpm-max" />
        </div>
        <Label className="flex flex-wrap items-center gap-2 text-sm">
          In
          <NumberField label="Steps" value={c.rpm_steps ?? 12} min={2} max={60} step={1} width="w-16" onChange={(v) => set({ ...c, rpm_steps: Math.round(v) })} />
          steps
        </Label>
        <p className="text-xs text-muted-foreground">A 5-inch prop on a racing quad tops out around 30,000 rpm; a 7-inch around 20,000.</p>
      </Step>

      <Step n={3} title="Flying forward too?" summary={speeds.map((v) => (v === 0 ? "hover" : `${n(v)} m/s`)).join(", ")} done testId="step-prop-speeds">
        <p className="text-sm text-soft">0 m/s is hovering. Faster forward flight lowers the push at the same speed.</p>
        <ul className="flex flex-wrap gap-1.5">
          {speeds.map((v) => (
            <li key={v}>
              <Badge className="pr-0.5">
                <span className="num">{v === 0 ? "Hover (0 m/s)" : `${n(v)} m/s`}</span>
                <Button variant="ghost" size="icon-xs" className="rounded-full" onClick={() => set({ ...c, airspeeds_m_s: speeds.filter((x) => x !== v) })} aria-label={`Remove ${v} m/s`}>
                  <XIcon />
                </Button>
              </Badge>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap items-center gap-2">
          <NumberField label="Forward speed to add" value={extra} unit="m/s" min={0} onChange={setExtra} width="w-20" />
          <Button size="sm" variant="outline" onClick={() => !speeds.includes(extra) && set({ ...c, airspeeds_m_s: [...speeds, extra].sort((a, b) => a - b) })}>
            <PlusIcon />
            Add
          </Button>
        </div>
      </Step>
    </Accordion>
  );
}
