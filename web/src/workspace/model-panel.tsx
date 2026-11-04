// The Model panel: what the part is, a units check, what SimLab thinks it
// is (a propeller, a frame with motor mounts), and its faces: point at one
// to see it, click to pick it and name it.
import { useEffect, useMemo, useRef, useState } from "react";
import { FanIcon, PlusIcon, RulerIcon, SparklesIcon, TriangleAlertIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Loader } from "@/components/ui/skeleton";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useApp } from "../lib/app";
import { dirLabel, KIND_NAME, n, patchLabel } from "../lib/format";
import type { Model, Patch } from "../lib/types";
import type { ProjectData } from "../lib/hooks";
import type { Prepared } from "../viewer/prepare";
import type { Nav } from "../screens/workspace";
import { Section, Stat } from "../components/section";
import { Panel } from "./common";
import { useScene, useSceneCtl } from "./scene-state";
import { presetFromMounts, presetSpin } from "./presets";

const UNITS = [
  { k: 25.4, word: "inches" },
  { k: 10, word: "centimetres" },
  { k: 1000, word: "metres" },
  { k: 0.001, word: "micrometres" },
];

/** sizes outside 5 mm .. 1.5 m are probably in another unit; the units that would make it sensible */
export function unitsQuestion(m: Pick<Model, "size_mm" | "unit_mm">): { across: number; options: { k: number; word: string; across: number }[] } | null {
  if (!m.size_mm) return null;
  const across = Math.max(...m.size_mm);
  if (across >= 5 && across <= 1500) return null;
  const raw = across / m.unit_mm; // in file units
  const options = UNITS.map((u) => ({ ...u, across: raw * u.k })).filter((o) => o.across >= 5 && o.across <= 1500 && o.k !== m.unit_mm);
  return { across, options };
}

const dismissedKey = (id: string) => `sl_units_ok_${id}`;

export function ModelPanel({ data, model, nav, prepared, reload }: { data: ProjectData; model: Model | undefined; nav: Nav; prepared: Prepared | null; reload(): void }) {
  const { api } = useApp();
  const { hovered } = useSceneCtl();
  const [picked, setPicked] = useState<number | null>(null);
  const [showMounts, setShowMounts] = useState(false);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [unitsOk, setUnitsOk] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const patches = model?.patches ?? [];
  const mounts = model?.suggestions?.motor_mounts ?? [];
  useEffect(() => {
    setPicked(null);
    try {
      setUnitsOk(!!model && localStorage.getItem(dismissedKey(model.id)) === "1");
    } catch {
      setUnitsOk(false);
    }
  }, [model?.id]);

  const hoverPatch = hovered !== null ? patches.find((p) => p.id === hovered) : undefined;
  useScene({
    source: { kind: "model", modelId: model?.id },
    marks: [
      ...(showMounts ? [{ ids: mounts.flatMap((m) => m.patches), tone: "plum" as const }] : []),
      ...(picked !== null ? [{ ids: [picked], tone: "ember" as const }] : []),
    ],
    onPick: (id) => setPicked((p) => (p === id ? null : id)),
    hint: hoverPatch ? patchLabel(hoverPatch, prepared?.holes.get(hoverPatch.id)) : patches.length ? "Point at a face to see it · click to pick it" : null,
  });

  const counts = useMemo(() => {
    const c = { plane: 0, cylinder: 0, holes: 0, other: 0 };
    for (const p of patches) {
      if (p.kind === "plane") c.plane++;
      else if (p.kind === "cylinder" && prepared?.holes.get(p.id) !== false) c.holes++;
      else if (p.kind === "cylinder") c.cylinder++;
      else c.other++;
    }
    return c;
  }, [patches, prepared]);

  const setUnits = async (k: number) => {
    if (!model) return;
    setBusy("units");
    setError("");
    try {
      await api.updateModel(model.id, { unit_mm: k });
      reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not change the units");
    } finally {
      setBusy("");
    }
  };
  const keepMm = () => {
    if (!model) return;
    try {
      localStorage.setItem(dismissedKey(model.id), "1");
    } catch {
      /* asked again next time */
    }
    setUnitsOk(true);
  };
  const addPart = async (f: File | undefined) => {
    if (!f) return;
    setBusy("upload");
    setError("");
    try {
      await api.uploadModel(data.project.id, f);
      reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not add the part");
    } finally {
      setBusy("");
    }
  };

  const q = model && model.status === "ready" && !unitsOk ? unitsQuestion(model) : null;
  const prop = model?.suggestions?.looks_like === "propeller" ? model.suggestions.prop : undefined;

  return (
    <Panel
      title="Model"
      testId="model-panel"
      actions={
        <>
          <Button variant="ghost" size="sm" onClick={() => file.current?.click()} disabled={busy === "upload"} data-testid="add-part">
            <PlusIcon />
            {busy === "upload" ? "Adding…" : "Add a part"}
          </Button>
          <Input ref={file} type="file" accept=".stl,.step,.stp,.obj,.3mf" className="hidden" onChange={(e) => addPart(e.target.files?.[0])} />
        </>
      }
    >
      {data.models.length > 1 && model && (
        <Select value={model.id} onValueChange={(v) => nav.go({ model: v }, true)}>
          <SelectTrigger aria-label="Which part">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {data.models.map((m) => (
              <SelectItem key={m.id} value={m.id}>
                {m.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {!model ? (
        <p className="text-sm text-muted-foreground">No part yet. Add an STL, STEP, OBJ or 3MF file to start.</p>
      ) : model.status === "processing" ? (
        <Card className="gap-3 bg-sunken p-4 text-sm text-soft">
          <Loader />
          {model.message || "Reading the part and finding its faces"}
        </Card>
      ) : model.status === "failed" ? (
        <Card className="gap-2 border-clay/40 bg-clay/10 p-4 text-sm text-clay-text" role="alert">
          <span className="font-medium">This file could not be read.</span>
          <span>{model.message}</span>
        </Card>
      ) : (
        <>
          <div className="flex flex-col gap-1">
            <h2 className="text-lg font-semibold leading-tight">{model.name}</h2>
            <p className="truncate text-sm text-muted-foreground">
              {model.file_name}
              {model.unit_mm !== 1 && ` · drawn in ${UNITS.find((u) => u.k === model.unit_mm)?.word ?? `units of ${model.unit_mm} mm`}`}
            </p>
            {model.message && <p className="text-sm text-gold-text">{model.message}</p>}
          </div>

          {q && (
            <Card className="gap-3 border-gold/40 bg-gold/10 p-4" role="alert" data-testid="units-check">
              <div className="flex items-start gap-2.5 text-sm text-gold-text">
                <RulerIcon className="mt-0.5 size-4 shrink-0" />
                <span>
                  <span className="font-semibold">
                    Is this part {n(q.across)} mm across{q.options[0] ? `, or ${n(q.across / model.unit_mm)} ${q.options[0].word}` : ""}?
                  </span>{" "}
                  STL files have no units, and {n(q.across)} mm is {q.across < 5 ? "very small" : "very big"} for a drone part.
                </span>
              </div>
              <div className="flex flex-wrap gap-2">
                {q.options.map((o, i) => (
                  <Button key={o.k} size="sm" variant={i === 0 ? "default" : "outline"} onClick={() => setUnits(o.k)} disabled={!!busy} data-testid={`units-${o.word}`}>
                    It's {o.word} ({n(o.across)} mm)
                  </Button>
                ))}
                <Button size="sm" variant="outline" onClick={keepMm} disabled={!!busy} data-testid="units-keep">
                  It really is {n(q.across)} mm
                </Button>
              </div>
            </Card>
          )}

          <div className="grid grid-cols-2 gap-2">
            <Stat label="Size" value={model.size_mm ? model.size_mm.map((v) => n(v)).join(" × ") : "–"} unit="mm" className="col-span-2" />
            <Stat label="Volume" value={model.volume_mm3 ? n(model.volume_mm3 / 1000) : "–"} unit="cm³" />
            <Stat label="Triangles" value={model.triangles ? n(model.triangles) : "–"} />
          </div>
          <div className="flex flex-wrap gap-2">
            {model.watertight === false ? (
              <Badge tone="gold">
                <TriangleAlertIcon />
                Has gaps in its surface
              </Badge>
            ) : (
              <Badge tone="moss">Closed surface, ready to test</Badge>
            )}
            <Badge>{patches.length} faces</Badge>
          </div>

          {prop && (
            <Suggestion
              icon={<FanIcon />}
              title={`This looks like a ${prop.blades}-blade propeller, ${n(prop.radius_mm * 2)} mm across`}
              detail={`It spins about ${dirLabel(prop.axis)}.`}
            >
              <Button size="sm" onClick={() => nav.go({ view: "tests", new: "prop", study: null, run: null, preset: null })} data-testid="suggest-prop">
                How much does it push?
              </Button>
              <Button size="sm" variant="outline" onClick={() => nav.go({ view: "tests", new: "strength", study: null, run: null, preset: JSON.stringify(presetSpin(model, 25000)) })}>
                Will it break when spinning?
              </Button>
            </Suggestion>
          )}
          {mounts.length > 0 && (
            <div onMouseEnter={() => setShowMounts(true)} onMouseLeave={() => setShowMounts(false)} onFocus={() => setShowMounts(true)} onBlur={() => setShowMounts(false)}>
              <Suggestion icon={<SparklesIcon />} title={`Found ${mounts.length} motor ${mounts.length === 1 ? "mount" : "mounts"}`} detail={`Bolt holes on a ${n(mounts[0]!.bolt_circle_mm)} mm circle. Point here to see them.`}>
                <Button size="sm" onClick={() => nav.go({ view: "tests", new: "strength", study: null, run: null, preset: JSON.stringify(presetFromMounts(model)) })} data-testid="suggest-motors">
                  Test it with motors at full throttle
                </Button>
              </Suggestion>
            </div>
          )}

          <Section title="Faces" aside={`${counts.plane} flat · ${counts.holes} ${counts.holes === 1 ? "hole" : "holes"}${counts.cylinder ? ` · ${counts.cylinder} round` : ""} · ${counts.other} curved`}>
            {picked !== null && patches.find((p) => p.id === picked) ? (
              <PickedFace key={picked} model={model} patch={patches.find((p) => p.id === picked)!} hole={prepared?.holes.get(picked)} onSaved={reload} onClose={() => setPicked(null)} />
            ) : (
              <p className="rounded-lg bg-sunken px-3 py-2.5 text-sm text-muted-foreground">Click a face in the 3D view to see its size and give it a name you will recognise later, like "Battery strap slot".</p>
            )}
          </Section>
        </>
      )}
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </Panel>
  );
}

function Suggestion({ icon, title, detail, children }: { icon: React.ReactNode; title: string; detail: string; children: React.ReactNode }) {
  return (
    <Card className="gap-3 border-plum/30 bg-plum/[0.08] p-4" data-testid="suggestion">
      <div className="flex items-start gap-2.5">
        <span className="mt-0.5 text-plum-text [&_svg]:size-4">{icon}</span>
        <span className="flex flex-col gap-0.5 text-sm">
          <span className="font-semibold">{title}</span>
          <span className="text-soft">{detail}</span>
        </span>
      </div>
      <div className="flex flex-wrap gap-2">{children}</div>
    </Card>
  );
}

function PickedFace({ model, patch, hole, onSaved, onClose }: { model: Model; patch: Patch; hole?: boolean; onSaved(): void; onClose(): void }) {
  const { api } = useApp();
  const [name, setName] = useState(patch.label ?? "");
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    try {
      await api.updateModel(model.id, { patch_labels: { [String(patch.id)]: name.trim() } });
      onSaved();
    } finally {
      setSaving(false);
    }
  };
  return (
    <Card className="gap-3 border-ember/40 bg-ember/[0.05] p-4" data-testid="picked-face">
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="font-semibold">{patchLabel(patch, hole)}</span>
          <span className="text-sm text-muted-foreground">
            {KIND_NAME[patch.kind]} face
            {patch.kind === "cylinder" && hole !== undefined ? (hole ? " (a hole)" : " (a round edge or boss)") : ""}
          </span>
        </div>
        <Button variant="ghost" size="xs" onClick={onClose}>
          Clear
        </Button>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <Stat label="Area" value={n(patch.area_mm2)} unit="mm²" />
        {patch.radius_mm ? <Stat label="Across" value={n(patch.radius_mm * 2)} unit="mm" /> : patch.normal ? <Stat label="Faces" value={dirLabel(patch.normal)} /> : <Stat label="Face number" value={patch.id} />}
      </div>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Name this face" aria-label="Name this face" />
        <Button type="submit" variant="outline" disabled={saving || name.trim() === (patch.label ?? "")}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </form>
    </Card>
  );
}
