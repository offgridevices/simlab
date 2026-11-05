// Strength results: the verdict in one line, the part coloured by a field
// (safety factor first) with a probe to read any point, a ring where it
// breaks first, the key numbers as readouts, "How sure", and the numbers
// behind it tucked into Details.
import { useEffect, useMemo, useState } from "react";
import { CrosshairIcon, ListChecksIcon, ScrollTextIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Alert } from "@/components/ui/alert";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { useApp } from "../lib/app";
import { useAsync, useField } from "../lib/hooks";
import { scaleFor } from "../lib/colormaps";
import { duration, engineLabel, n } from "../lib/format";
import type { Run, StrengthCheck, StrengthSummary } from "../lib/types";
import { Details, Row, Stat } from "../components/section";
import { HowSure, meshFact, Segmented } from "./common";
import { useScene, useSceneCtl } from "./scene-state";
import { Legend } from "./legend";
import { MetricChart, pickMetric } from "./running";
import type { Again } from "./start-run";

const DISP = ["ux", "uy", "uz"];
const coarse = () => typeof matchMedia !== "undefined" && matchMedia("(pointer: coarse)").matches;
const NICE: Record<string, string> = { safety_factor: "Safety", von_mises: "Stress", max_principal: "Pull", displacement: "Movement", failure_index: "Failure" };
const ORDER = ["safety_factor", "von_mises", "max_principal", "displacement", "failure_index"];

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
    <Card className={cn("gap-1 p-4", v.box)} data-testid="verdict" data-verdict={s.verdict}>
      <div className="flex items-baseline justify-between gap-3">
        <span className={cn("font-semibold tracking-tight", compact ? "text-xl" : "text-3xl", v.text)}>{word}</span>
        <span className={cn("num font-semibold", compact ? "text-base" : "text-2xl", v.text)} title="Safety factor: how many times this load it could take">
          {s.safety_factor >= 100 ? "100×+" : `${s.safety_factor.toFixed(1)}×`}
        </span>
      </div>
      <span className={cn("text-soft", compact ? "text-sm" : "text-base")}>{rest}</span>
    </Card>
  );
}

/** a CalculiX double-check next to SimLab's own run of the same test */
function CheckCard({ c }: { c: StrengthCheck }) {
  const pct = (v?: number | null) => (v === null || v === undefined ? "–" : `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(1)}%`);
  const rows: [string, number | null | undefined, number | null | undefined, number | null | undefined, string][] = [
    ["Biggest stress", c.native.max_von_mises_mpa, c.calculix.max_von_mises_mpa, c.diff_pct.max_von_mises, "MPa"],
    ["Safety factor", c.native.safety_factor, c.calculix.safety_factor, c.diff_pct.safety_factor, "×"],
    ["Biggest movement", c.native.max_displacement_mm, c.calculix.max_displacement_mm, c.diff_pct.max_displacement, "mm"],
  ];
  return (
    <Card className={cn("gap-2 p-3", c.agrees ? "border-moss/40 bg-moss/10" : "border-gold/40 bg-gold/10")} data-testid="calculix-check">
      <span className={cn("text-sm font-medium", c.agrees ? "text-moss-text" : "text-gold-text")}>{c.text}</span>
      <Table className="text-xs">
        <TableHeader>
          <TableRow>
            <TableHead />
            <TableHead className="text-right">SimLab</TableHead>
            <TableHead className="text-right">CalculiX</TableHead>
            <TableHead className="text-right">Difference</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map(([label, a, b, d, unit]) => (
            <TableRow key={label}>
              <TableCell>{label}</TableCell>
              <TableCell className="num whitespace-nowrap text-right">
                {n(a)} {unit}
              </TableCell>
              <TableCell className="num whitespace-nowrap text-right">
                {n(b)} {unit}
              </TableCell>
              <TableCell className="num text-right">{pct(d)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Card>
  );
}

export function StrengthResults({ run, summary: s, size = 100, again }: { run: Run; summary: StrengthSummary; /** the part's biggest size, mm */ size?: number; again?: Again }) {
  const { api } = useApp();
  const settings = useAsync(`settings:${run.id}`, () => api.settings());
  const isCheck = (s.engine ?? run.engine) === "reference";
  const { viewer } = useSceneCtl();
  const shown = run.fields.filter((f) => !DISP.includes(f.name)).sort((a, b) => (ORDER.indexOf(a.name) + 99) % 99 - ((ORDER.indexOf(b.name) + 99) % 99));
  const [fieldName, setFieldName] = useState(() => (shown.some((f) => f.name === "safety_factor") ? "safety_factor" : shown[0]?.name ?? ""));
  const info = shown.find((f) => f.name === fieldName) ?? null;
  const field = useField(run.id, fieldName || null);
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
    probe: info ? { label: info.label, unit: info.unit } : null,
    deform: disp ? { disp, k: autoK * exaggerate } : null,
    critical: s.critical_point_mm,
    hoverable: false,
    hint: bent && disp ? `Bend drawn ${n(autoK * exaggerate, 2)}× bigger than real` : coarse() ? "Tap the part to pin a value" : "Point at the part to read a value · click to pin it",
  });
  const [log, setLog] = useState<string | null>(null);
  useEffect(() => setLog(null), [run.id]);
  const metric = pickMetric(run.metrics);

  return (
    <div className="flex flex-col gap-4" data-testid="strength-results">
      <VerdictCard s={s} />
      {s.check && <CheckCard c={s.check} />}
      <Button variant="outline" size="sm" className="self-start" onClick={() => viewer.current?.focus(s.critical_point_mm)} data-testid="show-critical">
        <CrosshairIcon />
        {s.verdict === "breaks" ? "Show where it breaks" : "Show the weakest spot"}
      </Button>

      <div className="grid grid-cols-2 gap-2">
        <Stat label="Weight" value={n(s.mass_g)} unit="g" hint={run.material?.name} />
        <Stat label="Biggest stress" value={n(s.max_von_mises_mpa)} unit="MPa" hint={run.material ? `strength ${n(run.material.uts_xy_mpa)} MPa` : undefined} />
        <Stat label="Biggest movement" value={n(s.max_displacement_mm)} unit="mm" />
        <Stat label="Breaks at" value={s.breaks_at?.rpm ? n(Math.round(s.breaks_at.rpm / 100) * 100) : `${n(s.safety_factor * 100, 3)}`} unit={s.breaks_at?.rpm ? "rpm" : "% of load"} />
      </div>

      <HowSure
        accuracy={s.accuracy}
        facts={[meshFact(s.elements, s.element_mm), s.solver, `solved in ${duration(s.solve_seconds)}`, isCheck && "CalculiX check"]}
      />
      {again && !isCheck && settings.value?.reference_engine_enabled !== false && (
        <div className="flex flex-col gap-1.5" data-testid="double-check">
          <Button variant="outline" size="sm" className="self-start" onClick={() => again.rerun("reference")} disabled={again.starting} data-testid="run-calculix">
            <ListChecksIcon />
            {again.starting ? "Starting…" : "Double-check in CalculiX"}
          </Button>
          <span className="text-xs text-muted-foreground">Solves the same mesh and loads again in CalculiX, a widely used independent solver (in Docker), and compares the two.</span>
          {again.setting && <span className="text-xs text-muted-foreground">{again.setting}</span>}
          {again.startError && <span className="text-xs text-destructive">{again.startError}</span>}
          {again.dialog}
        </div>
      )}
      {metric && (
        <div className="flex flex-col gap-1">
          <span className="text-2xs font-medium uppercase tracking-wider text-muted-foreground">How the solver settled</span>
          <MetricChart run={run} name={metric} height={90} compact />
        </div>
      )}

      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium text-muted-foreground">Colour the part by</span>
        <Segmented label="Colour the part by" value={fieldName} onChange={setFieldName} options={shown.map((f) => ({ id: f.name, label: NICE[f.name] ?? f.label }))} testId="field-picker" />
        {info && <p className="text-xs text-muted-foreground">{FIELD_HELP[info.name] ?? info.label}</p>}
        {field.error && <p className="text-xs text-destructive">{field.error}</p>}
      </div>
      {info && scale && <Legend scale={scale} label={info.label} unit={info.unit} min={info.min} max={info.max} />}

      {hasDisp && (
        <Card className="gap-2 bg-sunken p-3">
          <Label className="justify-between font-normal">
            Show it bent
            <Switch checked={bent} onCheckedChange={setBent} data-testid="bend-toggle" />
          </Label>
          {bent && (
            <Label className="gap-3 text-xs font-normal text-muted-foreground">
              Exaggerate
              <Slider min={0} max={4} step={0.05} value={[exaggerate]} onValueChange={([v]) => setExaggerate(v!)} aria-label="How much to exaggerate the bend" />
            </Label>
          )}
        </Card>
      )}

      {s.warnings.map((w) => (
        <Alert key={w} tone="gold">
          {w}
        </Alert>
      ))}

      <Details summary="Details" testId="details">
        <Row label="Material">{run.material?.name ?? "–"}</Row>
        {run.material && run.material.process === "fdm" && (
          <Row label="Strength along / across layers">
            {n(run.material.uts_xy_mpa)} / {n(run.material.uts_z_mpa)} MPa
          </Row>
        )}
        <Row label="Biggest pull (max principal)">{n(s.max_principal_mpa)} MPa</Row>
        <Row label="Safety factor">{s.safety_factor.toFixed(2)}×</Row>
        {s.breaks_at && <Row label="Breaks at">{s.breaks_at.text}</Row>}
        <Row label="Pieces (elements)">{n(s.elements)}</Row>
        {s.element_mm ? <Row label="Piece size">{n(s.element_mm, 2)} mm</Row> : null}
        <Row label="Corners (nodes)">{n(s.nodes)}</Row>
        <Row label="Unknowns">{n(s.dofs)}</Row>
        <Row label="Solver">{s.solver}</Row>
        <Row label="Meshing / solving">
          {duration(s.mesh_seconds)} / {duration(s.solve_seconds)}
        </Row>
        <Row label="Solved by">{engineLabel("strength", s.engine ?? run.engine)}</Row>
        {s.reactions_n && <Row label="Held faces push back">{s.reactions_n.map((v) => n(v, 2)).join(", ")} N</Row>}
        <Button variant="ghost" size="xs" className="mt-1" onClick={async () => setLog(log === null ? await api.log(run.id).catch((e: Error) => e.message) : null)} data-testid="show-log">
          <ScrollTextIcon />
          {log === null ? "Show the log" : "Hide the log"}
        </Button>
        {log !== null && (
          <ScrollArea className="mt-2 h-48 rounded-md bg-sunken">
            <pre className="num whitespace-pre-wrap p-2 text-2xs text-soft">{log}</pre>
          </ScrollArea>
        )}
      </Details>
    </div>
  );
}

const FIELD_HELP: Record<string, string> = {
  safety_factor: "How many times this load each spot could take before it breaks. Below 1 (clay) breaks, 1 to 2 (gold) is cutting it close, 2 and up (moss) is safe.",
  von_mises: "How hard the material is working at each spot, in MPa. Compare it with the material's strength.",
  max_principal: "The biggest pull (red) or push (blue) at each spot, in MPa. Printed layers mostly fail by being pulled apart.",
  displacement: "How far each spot moves under the load, in millimetres.",
  failure_index: "Above 1 means that spot breaks. It counts the weaker direction between printed layers.",
};
