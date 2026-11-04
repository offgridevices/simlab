// Strength test setup: material, which way it was printed, how it is held,
// what pushes on it, and how detailed. Faces are picked by clicking them in
// the 3D view while their step is open.
import { useMemo, useState } from "react";
import { MousePointerClickIcon, PlaneTakeoffIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { useApp } from "../lib/app";
import { dirLabel, G, LOAD_NAME, loadLine, n } from "../lib/format";
import type { Load, LoadType, MeshQuality, Motor, Patch, Region, StrengthConfig, Support, Vec3 } from "../lib/types";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { AxisPicker, facesCentre, NumberField, Segmented, Step, toggle, unit } from "./common";
import { AreaPicker, type PickMode } from "./area-picker";
import { isRegion, nextRegionId, regionCentre } from "../lib/regions";
import { MaterialPicker } from "./material-picker";
import { useScene } from "./scene-state";
import { Accordion } from "@/components/ui/accordion";
import { DEFAULT_MOTOR, modelCentre, motorLoad, motorTorque } from "./presets";
import type { StepsProps } from "./study-form";
import type { ToneArrow, ToneMark } from "../viewer/viewer";

export function strengthProblems(c: StrengthConfig): string[] {
  const out: string[] = [];
  if (!c.material_id) out.push("Pick a material (step 1).");
  if (c.support.type === "fixed" && !c.support.patches.length) out.push("Click the faces that are held still, or say it is flying (step 3).");
  if (!c.loads.length) out.push("Add at least one load: a motor, a push, a crash… (step 4).");
  c.loads.forEach((l, i) => {
    if ("patches" in l && !l.patches.length) out.push(`Load ${i + 1} (${LOAD_NAME[l.type].toLowerCase()}): click the faces it pushes on.`);
  });
  return out;
}

type Target = { kind: "bed" } | { kind: "support" } | { kind: "load"; i: number } | null;

export function StrengthSteps({ config: c, set, model, prepared, step, setStep, estimate }: StepsProps<StrengthConfig>) {
  const { materials, motors } = useApp();
  const patches = model.patches ?? [];
  const [bedPick, setBedPick] = useState(false);
  const [editing, setEditing] = useState<number | null>(null);
  const [pickMode, setPickMode] = useState<PickMode>("faces");
  const [brush, setBrush] = useState(() => roundBrush(model.size_mm ? Math.max(...model.size_mm) * 0.08 : 6));
  const [activeArea, setActiveArea] = useState<number | null>(null);
  const regions = c.regions ?? [];
  const pg = model.suggestions?.prop;
  const hub = pg ? { center: pg.center_mm, axis: pg.axis, radius: pg.hub_radius_mm ?? pg.radius_mm * 0.11 } : null;
  const mat = materials?.find((m) => m.id === c.material_id);
  const mounts = model.suggestions?.motor_mounts ?? [];
  const R = model.size_mm ? Math.max(...model.size_mm) / 2 : 50;

  const target: Target = step === 2 && bedPick ? { kind: "bed" } : step === 3 && c.support.type === "fixed" ? { kind: "support" } : step === 4 && editing !== null && c.loads[editing] && "patches" in c.loads[editing]! ? { kind: "load", i: editing } : null;

  const setLoad = (i: number, l: Load) => set({ ...c, loads: c.loads.map((x, j) => (j === i ? l : x)) });
  /** new ids for the support or a load, with the config's regions; areas nothing uses any more are dropped */
  const setArea = (where: "support" | number, ids: number[], next: Region[]) => {
    const support: Support = where === "support" ? { type: "fixed", patches: ids } : c.support;
    const loads = typeof where === "number" ? c.loads.map((l, j) => (j === where && "patches" in l ? ({ ...l, patches: ids } as Load) : l)) : c.loads;
    const used = new Set([...(support.type === "fixed" ? support.patches : []), ...loads.flatMap((l) => ("patches" in l ? l.patches : []))]);
    set({ ...c, support, loads, regions: next.filter((r) => used.has(r.id)) });
  };
  const brushAt = (p: Vec3) => {
    if (!target || target.kind === "bed") return;
    const id = nextRegionId(regions);
    const r: Region = { id, label: "", shape: { type: "sphere", center_mm: p.map((v) => Number(v.toFixed(3))) as Vec3, radius_mm: brush / 2 } };
    const ids = target.kind === "support" ? (c.support.type === "fixed" ? c.support.patches : []) : (c.loads[target.i] as { patches: number[] }).patches;
    setArea(target.kind === "support" ? "support" : target.i, [...ids, id], [...regions, r]);
    setActiveArea(id);
  };
  const pick = (id: number) => {
    if (!target) return;
    if (target.kind === "bed") {
      const nrm = faceNormal(id, patches, prepared);
      if (nrm) set({ ...c, print_up: [-nrm[0], -nrm[1], -nrm[2]].map((v) => Number(v.toFixed(4))) as Vec3 });
      setBedPick(false);
    } else if (target.kind === "support" && c.support.type === "fixed") set({ ...c, support: { type: "fixed", patches: toggle(c.support.patches, id) } });
    else if (target.kind === "load") {
      const l = c.loads[target.i]!;
      if ("patches" in l) setLoad(target.i, { ...l, patches: toggle(l.patches, id) } as Load);
    }
  };

  // what the 3D view shows: held faces in sky, loaded faces in ember, arrows for the loads
  const marks: ToneMark[] = [];
  if (c.support.type === "fixed") marks.push({ ids: c.support.patches, tone: "sky", alpha: target?.kind === "support" ? 0.9 : 0.7 });
  c.loads.forEach((l, i) => {
    if ("patches" in l) marks.push({ ids: l.patches, tone: "ember", alpha: target?.kind === "load" && target.i === i ? 0.95 : 0.55 });
  });
  const arrows = useMemo(() => loadArrows(c.loads, patches, regions, model, R), [c.loads, patches, regions, model, R]);
  const heldIds = new Set(c.support.type === "fixed" ? c.support.patches : []);
  const sceneRegions = regions.map((r) => ({ shape: r.shape, tone: heldIds.has(r.id) ? ("sky" as const) : ("ember" as const), alpha: r.id === activeArea ? 0.8 : 0.55 }));
  const brushing = pickMode === "brush" && (target?.kind === "support" || target?.kind === "load");
  const spin = c.loads.find((l): l is Extract<Load, { type: "spin" | "prop_aero" }> => l.type === "spin" || l.type === "prop_aero");
  useScene({
    source: { kind: "model", modelId: model.id },
    marks,
    onPick: target && !brushing ? pick : undefined,
    onPoint: brushing ? brushAt : undefined,
    regions: sceneRegions,
    arrows,
    plate: step === 2 ? { up: c.print_up } : null,
    spin: spin ? { center: spin.center_mm, axis: spin.axis, radius: model.suggestions?.prop?.radius_mm ?? R * 0.9 } : null,
    hint:
      brushing
        ? "Click the part to cover the area around that point"
        : target?.kind === "bed"
        ? "Click the face that sits on the printer's bed"
        : target?.kind === "support"
          ? "Click faces to hold them still · click again to let go"
          : target?.kind === "load"
            ? `Click the faces load ${target.i + 1} pushes on`
            : null,
  });

  const add = (t: LoadType) => {
    const centre = model.suggestions?.prop?.center_mm ?? modelCentre(model);
    const axis = model.suggestions?.prop?.axis ?? [0, 0, 1];
    const l: Load =
      t === "motor"
        ? motorLoad(motors?.find((m) => m.id === DEFAULT_MOTOR.id) ?? DEFAULT_MOTOR, [], [0, 0, 1], `Motor ${c.loads.filter((x) => x.type === "motor").length + 1}`)
        : t === "force"
          ? { type: "force", patches: [], force_n: [0, 0, -20], label: "" }
          : t === "pressure"
            ? { type: "pressure", patches: [], pressure_pa: 10_000, label: "" }
            : t === "acceleration"
              ? { type: "acceleration", g: [0, 0, -10], label: "Hard landing" }
              : t === "spin"
                ? { type: "spin", rpm: 25000, axis, center_mm: centre, label: "" }
                : { type: "prop_aero", rpm: 25000, airspeed_m_s: 0, axis, center_mm: centre, label: "" };
    set({ ...c, loads: [...c.loads, l] });
    setEditing(c.loads.length);
  };
  const motorsOnMounts = () => {
    const m = motors?.find((x) => x.id === DEFAULT_MOTOR.id) ?? DEFAULT_MOTOR;
    set({ ...c, loads: [...c.loads.filter((l) => l.type !== "motor"), ...mounts.map((g, i) => motorLoad(m, g.patches, g.axis, `Motor ${i + 1}`))] });
    setEditing(null);
  };

  const fdm = mat?.process === "fdm";
  return (
    <Accordion type="single" collapsible value={String(step)} onValueChange={(v) => setStep(Number(v) || 0)} className="flex flex-col gap-2">
      <Step n={1} title="What is it made of?" summary={mat ? mat.name : "Not picked"} done={!!mat} testId="step-material">
        <MaterialPicker materials={materials} value={c.material_id} onChange={(id) => set({ ...c, material_id: id })} />
        <Button size="sm" variant="outline" className="self-end" onClick={() => setStep(2)}>
          Next
        </Button>
      </Step>

      <Step n={2} title="Which way is up on the printer?" summary={`Layers stack along ${dirLabel(c.print_up)}`} done testId="step-print">
        <p className="text-sm text-soft">
          {fdm || !mat
            ? "Printed parts are weakest between their layers. Tell SimLab how it sat on the bed, so it knows which way the layers run."
            : "Resin parts are about as strong in every direction, so this matters less. It still sets how the part is shown."}
        </p>
        <Button variant={bedPick ? "default" : "outline"} size="sm" className="self-start" onClick={() => setBedPick((b) => !b)} aria-pressed={bedPick} data-testid="pick-bed">
          <MousePointerClickIcon />
          {bedPick ? "Now click the face on the bed…" : "Click the face that sits on the bed"}
        </Button>
        <span className="text-xs text-muted-foreground">Or pick which way is up:</span>
        <AxisPicker label="Up on the printer" value={c.print_up} onChange={(v) => set({ ...c, print_up: v })} />
      </Step>

      <Step
        n={3}
        title="How is it held?"
        summary={c.support.type === "free_flying" ? "Flying: nothing holds it" : `${c.support.patches.length} ${c.support.patches.length === 1 ? "face" : "faces"} held still`}
        done={c.support.type === "free_flying" || c.support.patches.length > 0}
        testId="step-support"
      >
        <RadioGroup
          aria-label="How it is held"
          className="gap-2"
          value={c.support.type}
          onValueChange={(v) => set({ ...c, support: v === "fixed" ? (c.support.type === "fixed" ? c.support : { type: "fixed", patches: [] }) : { type: "free_flying" } })}
        >
          {[
            { id: "fixed", title: "Held still at some faces", detail: "Bolted, clamped or glued there. Click those faces in the 3D view.", testId: "support-fixed" },
            { id: "free_flying", title: "It's flying: nothing holds it", detail: "The loads speed it up instead, like a frame in the air. Best for frames.", testId: "support-free" },
          ].map((o) => (
            <Label
              key={o.id}
              htmlFor={`support-${o.id}`}
              className="flex cursor-pointer items-start gap-3 rounded-lg border bg-card px-3.5 py-3 font-normal leading-normal has-[[data-state=checked]]:border-ember/60 has-[[data-state=checked]]:bg-ember/[0.06]"
              data-testid={o.testId}
            >
              <RadioGroupItem id={`support-${o.id}`} value={o.id} className="mt-0.5" />
              <span className="flex flex-col gap-0.5">
                <span className="font-medium">{o.title}</span>
                <span className="text-sm text-muted-foreground">{o.detail}</span>
              </span>
            </Label>
          ))}
        </RadioGroup>
        {c.support.type === "fixed" && (
          <AreaPicker
            ids={c.support.patches}
            regions={regions}
            onChange={(ids, next) => setArea("support", ids, next)}
            patches={patches}
            prepared={prepared}
            mode={pickMode}
            onMode={setPickMode}
            brush={brush}
            onBrush={setBrush}
            active={activeArea}
            hub={hub}
            empty="Nothing held yet. Click the faces that are bolted or clamped, or brush the area."
          />
        )}
      </Step>

      <Step
        n={4}
        title="What pushes on it?"
        summary={c.loads.length ? c.loads.map((l) => LOAD_NAME[l.type]).join(", ") : "No loads yet"}
        done={c.loads.length > 0}
        testId="step-loads"
      >
        {mounts.length > 0 && !c.loads.some((l) => l.type === "motor") && (
          <Button size="sm" variant="outline" className="self-start" onClick={motorsOnMounts} data-testid="motors-on-mounts">
            <PlaneTakeoffIcon />
            Put a motor on each of the {mounts.length} mounts found
          </Button>
        )}
        <ul className="flex flex-col gap-2">
          {c.loads.map((l, i) => (
            <LoadCard
              key={i}
              i={i}
              load={l}
              open={editing === i}
              onOpen={() => setEditing(editing === i ? null : i)}
              onChange={(x) => setLoad(i, x)}
              onRemove={() => {
                set({ ...c, loads: c.loads.filter((_, j) => j !== i) });
                setEditing(null);
              }}
              patches={patches}
              prepared={prepared}
              area={{ regions, onChange: (ids, next) => setArea(i, ids, next), mode: pickMode, onMode: setPickMode, brush, onBrush: setBrush, active: activeArea }}
              motors={motors}
              mounts={mounts}
              propCentre={model.suggestions?.prop?.center_mm ?? null}
            />
          ))}
        </ul>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button size="sm" variant={c.loads.length ? "outline" : "default"} className="self-start" data-testid="add-load">
              <PlusIcon />
              Add a load
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-64">
            {LOAD_MENU.map((x) => (
              <DropdownMenuItem key={x.type} onSelect={() => add(x.type)} className="flex-col items-start gap-0" data-testid={`add-${x.type}`}>
                <span className="font-medium">{LOAD_NAME[x.type]}</span>
                <span className="text-xs text-muted-foreground">{x.detail}</span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </Step>

      <Step n={5} title="How detailed?" summary={QUALITY_LABEL[c.quality.type]} done testId="step-quality">
        <Segmented
          label="Detail"
          value={c.quality.type === "custom" ? "normal" : c.quality.type}
          onChange={(v) => set({ ...c, quality: { type: v } as MeshQuality })}
          options={[
            { id: "draft", label: "Draft" },
            { id: "normal", label: "Normal" },
            { id: "fine", label: "Fine" },
          ]}
          testId="quality"
        />
        <p className="text-xs text-muted-foreground">{QUALITY_HELP[c.quality.type]}</p>
        {estimate}
      </Step>
    </Accordion>
  );
}

const QUALITY_LABEL = { draft: "Draft: quick look", normal: "Normal", fine: "Fine: most exact", custom: "Custom size" } as const;
const QUALITY_HELP = {
  draft: "Big pieces: a first look in seconds. Thin walls and small holes may come out too weak or too strong.",
  normal: "Good for most parts. The answer usually lands within a few percent of Fine.",
  fine: "Small pieces everywhere: slower, and it needs more memory. Use it to confirm a close call.",
  custom: "A piece size you chose.",
} as const;

const LOAD_MENU: { type: LoadType; detail: string }[] = [
  { type: "motor", detail: "Thrust, twist and weight of a motor bolted on" },
  { type: "force", detail: "A push or pull on some faces, in newtons" },
  { type: "pressure", detail: "Air or liquid pressing on faces" },
  { type: "acceleration", detail: "A crash landing, or plain gravity, in g" },
  { type: "spin", detail: "Spinning fast, like a propeller" },
  { type: "prop_aero", detail: "The air pushing on spinning blades" },
];

function LoadCard({
  i,
  load: l,
  open,
  onOpen,
  onChange,
  onRemove,
  patches,
  prepared,
  area,
  motors,
  mounts,
  propCentre,
}: {
  i: number;
  load: Load;
  open: boolean;
  onOpen(): void;
  onChange(l: Load): void;
  onRemove(): void;
  patches: Patch[];
  prepared: StepsProps<StrengthConfig>["prepared"];
  area: { regions: Region[]; onChange(ids: number[], regions: Region[]): void; mode: PickMode; onMode(m: PickMode): void; brush: number; onBrush(d: number): void; active: number | null };
  motors: Motor[] | null;
  mounts: NonNullable<NonNullable<import("../lib/types").Model["suggestions"]>["motor_mounts"]>;
  propCentre: Vec3 | null;
}) {
  const needsFaces = "patches" in l;
  const missing = needsFaces && !l.patches.length;
  return (
    <li data-testid="load-card">
      <Collapsible open={open} onOpenChange={onOpen} className={cn("rounded-lg border", open ? "border-ember/50 bg-ember/[0.03]" : "bg-sunken/40")}>
      <div className="flex items-center gap-2 px-3 py-2">
        <CollapsibleTrigger className="flex min-w-0 flex-1 flex-col rounded-sm text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50">
          <span className="flex items-center gap-2 text-sm font-medium">
            <span className="num text-xs text-muted-foreground">{i + 1}</span>
            {l.label || LOAD_NAME[l.type]}
            {missing && <Badge tone="gold">Pick faces</Badge>}
          </span>
          <span className="truncate text-xs text-muted-foreground">{loadLine(l)}</span>
        </CollapsibleTrigger>
        <Button variant="ghost" size="icon-sm" onClick={onRemove} aria-label={`Remove load ${i + 1}`}>
          <Trash2Icon />
        </Button>
      </div>
      <CollapsibleContent>
        <div className="flex flex-col gap-3 border-t px-3 py-3">
          {l.type === "motor" && <MotorFields load={l} onChange={onChange} motors={motors} mounts={mounts} />}
          {l.type === "force" && (
            <>
              <Field label="How hard">
                <NumberField label="Force" value={Math.hypot(...l.force_n)} unit="N" min={0} onChange={(v) => onChange({ ...l, force_n: scale(unit(l.force_n), v) })} />
                <span className="text-xs text-muted-foreground">≈ {n((Math.hypot(...l.force_n) / G) * 1000)} g of weight</span>
              </Field>
              <Field label="Which way">
                <AxisPicker label="Direction of the push" value={l.force_n} onChange={(v) => onChange({ ...l, force_n: scale(unit(v), Math.hypot(...l.force_n) || 1) })} />
              </Field>
            </>
          )}
          {l.type === "pressure" && (
            <Field label="How hard">
              <NumberField label="Pressure" value={l.pressure_pa / 1000} unit="kPa" onChange={(v) => onChange({ ...l, pressure_pa: v * 1000 })} />
              <span className="text-xs text-muted-foreground">Positive presses in; negative pulls out. 100 kPa is about one atmosphere.</span>
            </Field>
          )}
          {l.type === "acceleration" && (
            <>
              <div className="flex flex-wrap gap-1.5">
                {[
                  { label: "Gravity, 1 g", g: [0, 0, -1] as Vec3 },
                  { label: "Hard landing, 10 g", g: [0, 0, -10] as Vec3 },
                  { label: "Crash, 50 g", g: [0, 0, -50] as Vec3 },
                ].map((p) => (
                  <Button key={p.label} size="xs" variant="outline" onClick={() => onChange({ ...l, g: p.g, label: p.label.split(",")[0] })}>
                    {p.label}
                  </Button>
                ))}
              </div>
              <Field label="How hard">
                <NumberField label="Acceleration" value={Math.hypot(...l.g)} unit="g" min={0} onChange={(v) => onChange({ ...l, g: scale(unit(l.g), v) })} />
              </Field>
              <Field label="Which way it is thrown">
                <AxisPicker label="Direction of the acceleration" value={l.g} onChange={(v) => onChange({ ...l, g: scale(unit(v), Math.hypot(...l.g) || 1) })} />
              </Field>
            </>
          )}
          {(l.type === "spin" || l.type === "prop_aero") && (
            <>
              <Field label="Speed">
                <NumberField label="Speed" value={l.rpm} unit="rpm" min={0} step={500} onChange={(v) => onChange({ ...l, rpm: v })} testId="load-rpm" />
              </Field>
              {l.type === "prop_aero" && (
                <Field label="Flying forward at">
                  <NumberField label="Forward speed" value={l.airspeed_m_s ?? 0} unit="m/s" min={0} onChange={(v) => onChange({ ...l, airspeed_m_s: v })} />
                </Field>
              )}
              <Field label="Spins about">
                <AxisPicker label="Spin axis" value={l.axis} onChange={(v) => onChange({ ...l, axis: v })} />
              </Field>
              <Field label="Centre (mm)">
                <div className="flex flex-wrap items-center gap-2">
                  {(["x", "y", "z"] as const).map((k, j) => (
                    <span key={k} className="inline-flex items-center gap-1 text-xs text-muted-foreground">
                      {k.toUpperCase()}
                      <NumberField label={`Centre ${k}`} width="w-16" value={l.center_mm[j]!} onChange={(v) => onChange({ ...l, center_mm: l.center_mm.map((c, q) => (q === j ? v : c)) as Vec3 })} />
                    </span>
                  ))}
                  {propCentre && (
                    <Button size="xs" variant="ghost" onClick={() => onChange({ ...l, center_mm: propCentre })}>
                      Use the hub
                    </Button>
                  )}
                </div>
              </Field>
            </>
          )}
          {needsFaces && (
            <Field label="Pushes on these faces">
              <AreaPicker
                ids={l.patches}
                regions={area.regions}
                onChange={area.onChange}
                patches={patches}
                prepared={prepared}
                mode={area.mode}
                onMode={area.onMode}
                brush={area.brush}
                onBrush={area.onBrush}
                active={area.active}
                empty="Nothing yet. Click faces in the 3D view, or brush the area."
              />
            </Field>
          )}
        </div>
      </CollapsibleContent>
      </Collapsible>
    </li>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      <div className="flex flex-col gap-1.5">{children}</div>
    </div>
  );
}

function MotorFields({ load: l, onChange, motors, mounts }: { load: Extract<Load, { type: "motor" }>; onChange(l: Load): void; motors: Motor[] | null; mounts: { patches: number[]; axis: Vec3 }[] }) {
  const match = motors?.find((m) => Math.abs((m.max_thrust_g / 1000) * G - l.thrust_n) < 1e-3 && Math.abs(m.mass_g - (l.motor_mass_g ?? 0)) < 1e-6);
  return (
    <>
      <Field label="Motor">
        <Select
          value={match?.id ?? "own"}
          onValueChange={(v) => {
            const m = motors?.find((x) => x.id === v);
            if (m) onChange({ ...l, thrust_n: (m.max_thrust_g / 1000) * G, torque_nm: motorTorque(m), motor_mass_g: m.mass_g });
          }}
        >
          <SelectTrigger aria-label="Motor" data-testid="motor-preset">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="own">My own numbers</SelectItem>
            {motors?.map((m) => (
              <SelectItem key={m.id} value={m.id}>
                {m.name} · <span className="num">{n(m.max_thrust_g)} g</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <div className="grid grid-cols-1 gap-2 min-[380px]:grid-cols-2">
        <Field label="Thrust, full throttle">
          <NumberField label="Thrust" value={(l.thrust_n / G) * 1000} unit="g" min={0} onChange={(v) => onChange({ ...l, thrust_n: (v / 1000) * G })} />
        </Field>
        <Field label="Twist (torque)">
          <NumberField label="Torque" value={l.torque_nm} unit="N·m" min={0} onChange={(v) => onChange({ ...l, torque_nm: v })} />
        </Field>
        <Field label="Motor weight">
          <NumberField label="Motor weight" value={l.motor_mass_g ?? 0} unit="g" min={0} onChange={(v) => onChange({ ...l, motor_mass_g: v })} />
        </Field>
      </div>
      <Field label="Pushes toward">
        <AxisPicker label="Thrust direction" value={l.axis} onChange={(v) => onChange({ ...l, axis: v })} />
      </Field>
      {mounts.length > 0 && (
        <Field label="Bolted to">
          <div className="flex flex-wrap gap-1.5">
            {mounts.map((g, j) => (
              <Button key={j} size="xs" variant="outline" onClick={() => onChange({ ...l, patches: g.patches, axis: g.axis })}>
                Mount {j + 1}
              </Button>
            ))}
          </div>
        </Field>
      )}
    </>
  );
}

const scale = (v: Vec3, k: number): Vec3 => [v[0] * k, v[1] * k, v[2] * k];

/** a brush size that reads well: 0.5 mm steps */
const roundBrush = (d: number) => Math.max(0.5, Math.round(d * 2) / 2);

/** the middle of some faces and areas */
function areaCentre(ids: number[], patches: Patch[], regions: Region[]): Vec3 | null {
  const faces = facesCentre(ids.filter((i) => !isRegion(i)), patches);
  const pts = [...(faces ? [faces] : []), ...ids.filter(isRegion).flatMap((i) => {
    const r = regions.find((x) => x.id === i);
    return r ? [regionCentre(r.shape)] : [];
  })];
  if (!pts.length) return null;
  return [0, 1, 2].map((k) => pts.reduce((s, p) => s + p[k]!, 0) / pts.length) as Vec3;
}

/** a face's outward direction: from the engine when it is flat, else averaged from the surface */
export function faceNormal(id: number, patches: Patch[], prepared: StepsProps<StrengthConfig>["prepared"]): Vec3 | null {
  const p = patches.find((x) => x.id === id);
  if (p?.normal) return unit(p.normal);
  if (!prepared?.vertexPatch) return null;
  const s: Vec3 = [0, 0, 0];
  const vp = prepared.vertexPatch;
  for (let i = 0; i < vp.length; i++)
    if (vp[i] === id) {
      s[0] += prepared.normals[i * 3]!;
      s[1] += prepared.normals[i * 3 + 1]!;
      s[2] += prepared.normals[i * 3 + 2]!;
    }
  return Math.hypot(...s) > 0 ? unit(s) : null;
}

function loadArrows(loads: Load[], patches: Patch[], regions: Region[], model: import("../lib/types").Model, R: number): ToneArrow[] {
  const out: ToneArrow[] = [];
  for (const l of loads) {
    if (l.type === "motor" || l.type === "force" || l.type === "pressure") {
      const c = areaCentre(l.patches, patches, regions);
      if (!c) continue;
      const dir: Vec3 | null = l.type === "motor" ? l.axis : l.type === "force" ? l.force_n : (() => {
        const p = patches.find((x) => x.id === l.patches[0]);
        return p?.normal ? (p.normal.map((v) => (l.pressure_pa >= 0 ? -v : v)) as Vec3) : null;
      })();
      if (dir && Math.hypot(...dir) > 0) out.push({ origin: c, dir: unit(dir), tone: "ember", length: R * 0.3 });
    } else if (l.type === "acceleration" && Math.hypot(...l.g) > 0) out.push({ origin: modelCentre(model), dir: unit(l.g), tone: "gold", length: R * 0.6 });
  }
  return out;
}
