// Strength results: the verdict in one line, the part coloured by a field
// (safety factor first), a ring where it breaks first, and the numbers
// behind it tucked into Details.
import { useEffect, useMemo, useState } from "react";
import { CrosshairIcon, ScrollTextIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { useApp } from "../lib/app";
import { useField } from "../lib/hooks";
import { scaleFor } from "../lib/colormaps";
import { duration, n } from "../lib/format";
import type { Run, StrengthSummary } from "../lib/types";
import { Details, Row, Stat } from "../components/section";
import { Segmented } from "./common";
import { useScene, useSceneCtl } from "./scene-state";
import { Legend } from "./legend";
import { MetricChart, pickMetric } from "./running";

const DISP = ["ux", "uy", "uz"];
const NICE: Record<string, string> = { safety_factor: "Safety", von_mises: "Stress", displacement: "Bend", failure_index: "Failure" };

export const VERDICT = {
  safe: { word: "Safe", box: "border-moss/40 bg-moss/10", text: "text-moss-text" },
  marginal: { word: "Marginal", box: "border-gold/40 bg-gold/10", text: "text-gold-text" },
  breaks: { word: "Breaks", box: "border-clay/40 bg-clay/10", text: "text-clay-text" },
} as const;

/** "Safe: 3.1× stronger…" -> ["Safe", "3.1× stronger…"] */
export function splitHeadline(s: Pick<StrengthSummary, "headline" | "verdict">): [string, string] {
  const h = s.headline.trim();
  const i = h.indexOf(":");
  if (i > 0 && i < 20) return [h.slice(0, i), h.slice(i + 1).trim()];
  return [VERDICT[s.verdict].word, h];
}

export function VerdictCard({ s, compact }: { s: StrengthSummary; compact?: boolean }) {
  const v = VERDICT[s.verdict];
  const [word, rest] = splitHeadline(s);
  return (
    <div className={cn("flex flex-col gap-1 rounded-xl border p-4", v.box)} data-testid="verdict" data-verdict={s.verdict}>
      <span className={cn("font-semibold tracking-tight", compact ? "text-xl" : "text-3xl", v.text)}>{word}</span>
      <span className={cn("text-soft", compact ? "text-sm" : "text-base")}>{rest}</span>
      {s.verdict !== "breaks" && !compact && (
        <span className="mt-1 text-xs text-muted-foreground">
          {s.verdict === "safe" ? "It would take about " : "Only about "}
          <span className="num">{s.safety_factor.toFixed(1)}</span> times this load to break it.
        </span>
      )}
    </div>
  );
}

export function StrengthResults({ run, summary: s, size = 100 }: { run: Run; summary: StrengthSummary; /** the part's biggest size, mm */ size?: number }) {
  const { api } = useApp();
  const { viewer } = useSceneCtl();
  const shown = run.fields.filter((f) => !DISP.includes(f.name));
  const [fieldName, setFieldName] = useState(() => (shown.some((f) => f.name === "safety_factor") ? "safety_factor" : shown[0]?.name ?? null));
  const info = shown.find((f) => f.name === fieldName) ?? null;
  const field = useField(run.id, fieldName);
  const hasDisp = DISP.every((d) => run.fields.some((f) => f.name === d));
  const [bent, setBent] = useState(false);
  const ux = useField(run.id, hasDisp && bent ? "ux" : null);
  const uy = useField(run.id, hasDisp && bent ? "uy" : null);
  const uz = useField(run.id, hasDisp && bent ? "uz" : null);
  const disp = useMemo(() => {
    if (!ux.value || !uy.value || !uz.value) return null;
    const out = new Float32Array(ux.value.length * 3);
    for (let i = 0; i < ux.value.length; i++) {
      out[i * 3] = ux.value[i]!;
      out[i * 3 + 1] = uy.value[i]!;
      out[i * 3 + 2] = uz.value[i]!;
    }
    return out;
  }, [ux.value, uy.value, uz.value]);
  // the bend you can see: the biggest movement drawn as 6% of the part's size, times the slider
  const [exaggerate, setExaggerate] = useState(1);
  const scale = info ? scaleFor(info) : null;
  const autoK = s.max_displacement_mm > 0 ? (0.06 * size) / s.max_displacement_mm : 1;
  useScene({
    source: { kind: "run", runId: run.id },
    field: field.value && scale ? { values: field.value, scale } : null,
    deform: disp ? { disp, k: autoK * exaggerate } : null,
    critical: s.critical_point_mm,
    hoverable: false,
    hint: bent && disp ? `Bend shown ${n(autoK * exaggerate, 2)}× bigger than it really is` : null,
  });
  const [log, setLog] = useState<string | null>(null);
  useEffect(() => setLog(null), [run.id]);
  const metric = pickMetric(run.metrics);

  return (
    <div className="flex flex-col gap-4" data-testid="strength-results">
      <VerdictCard s={s} />
      <Button variant="outline" size="sm" className="self-start" onClick={() => viewer.current?.focus(s.critical_point_mm)} data-testid="show-critical">
        <CrosshairIcon />
        {s.verdict === "breaks" ? "Show where it breaks" : "Show the weakest spot"}
      </Button>

      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium text-muted-foreground">Colour the part by</span>
        <Segmented label="Colour the part by" value={fieldName ?? ""} onChange={setFieldName} options={shown.map((f) => ({ id: f.name, label: NICE[f.name] ?? f.label }))} testId="field-picker" />
        {info && <p className="text-xs text-muted-foreground">{FIELD_HELP[info.name] ?? info.label}</p>}
        {field.error && <p className="text-xs text-destructive">{field.error}</p>}
      </div>
      {info && scale && <Legend scale={scale} label={info.label} unit={info.unit} />}

      {hasDisp && (
        <div className="flex flex-col gap-2 rounded-lg bg-sunken p-3">
          <label className="flex items-center justify-between gap-3 text-sm">
            Show it bent
            <Switch checked={bent} onCheckedChange={setBent} aria-label="Show it bent" data-testid="bend-toggle" />
          </label>
          {bent && (
            <label className="flex items-center gap-3 text-xs text-muted-foreground">
              Exaggerate
              <Slider min={0} max={4} step={0.05} value={[exaggerate]} onValueChange={([v]) => setExaggerate(v!)} aria-label="How much to exaggerate the bend" />
            </label>
          )}
        </div>
      )}

      <div className="grid grid-cols-2 gap-2">
        <Stat label="Weight" value={n(s.mass_g)} unit="g" hint={run.material?.name} />
        <Stat label="Biggest stress" value={n(s.max_von_mises_mpa)} unit="MPa" />
        <Stat label="Biggest bend" value={n(s.max_displacement_mm)} unit="mm" />
        <Stat label="Safety factor" value={`${s.safety_factor.toFixed(1)}×`} />
      </div>

      {s.warnings.length > 0 && (
        <ul className="flex flex-col gap-1.5">
          {s.warnings.map((w) => (
            <li key={w} className="rounded-lg bg-gold/10 px-3 py-2 text-sm text-gold-text">
              {w}
            </li>
          ))}
        </ul>
      )}

      <Details summary="Details" testId="details">
        <Row label="Material">{run.material?.name ?? "–"}</Row>
        {run.material && run.material.process === "fdm" && (
          <Row label="Strength along / across layers">
            {n(run.material.uts_xy_mpa)} / {n(run.material.uts_z_mpa)} MPa
          </Row>
        )}
        <Row label="Biggest pull (max principal)">{n(s.max_principal_mpa)} MPa</Row>
        {s.breaks_at && <Row label="Breaks at">{s.breaks_at.text}</Row>}
        <Row label="Pieces (elements)">{n(s.elements)}</Row>
        <Row label="Corners (nodes)">{n(s.nodes)}</Row>
        <Row label="Unknowns">{n(s.dofs)}</Row>
        <Row label="Solver">{s.solver}</Row>
        <Row label="Meshing / solving">
          {duration(s.mesh_seconds)} / {duration(s.solve_seconds)}
        </Row>
        <Row label="Solved by">{run.engine === "native" ? "SimLab" : run.engine}</Row>
        {s.reactions_n && <Row label="Held faces push back">{s.reactions_n.map((v) => n(v, 2)).join(", ")} N</Row>}
        {metric && (
          <div className="py-2">
            <MetricChart run={run} name={metric} height={120} />
          </div>
        )}
        <Button
          variant="ghost"
          size="xs"
          className="mt-1"
          onClick={async () => setLog(log === null ? await api.log(run.id).catch((e: Error) => e.message) : null)}
          data-testid="show-log"
        >
          <ScrollTextIcon />
          {log === null ? "Show the log" : "Hide the log"}
        </Button>
        {log !== null && <pre className="num mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-md bg-sunken p-2 text-[11px] text-soft">{log}</pre>}
      </Details>
    </div>
  );
}

const FIELD_HELP: Record<string, string> = {
  safety_factor: "How many times this load each spot could take before it breaks. Below 1 (clay) breaks, 1 to 2 (gold) is cutting it close, over 2 (moss) is safe.",
  von_mises: "How hard the material is working at each spot, in MPa. Compare it with the material's strength.",
  displacement: "How far each spot moves under the load, in millimetres.",
  failure_index: "Above 1 means that spot breaks. It counts the weaker direction between printed layers.",
};
