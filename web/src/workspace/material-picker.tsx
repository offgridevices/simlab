// Picking a material: a shadcn Combobox (Popover + Command) over the library,
// grouped into printer filaments, resins and the rest, each with its
// strength along and across the printed layers. The chosen one is summed up
// under the button, and its numbers can be changed right there (from a data
// sheet): a built-in material is saved as your own copy, your own is updated.
import { useState, type FormEvent } from "react";
import { CheckIcon, ChevronRightIcon, ChevronsUpDownIcon, PencilIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useApp } from "../lib/app";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { cn } from "@/lib/utils";
import { n } from "../lib/format";
import type { Material, Process } from "../lib/types";
import { Stat } from "../components/section";

const GROUPS: { process: Process; label: string }[] = [
  { process: "fdm", label: "Filament (FDM printers)" },
  { process: "resin", label: "Resin (SLA / MSLA printers)" },
  { process: "other", label: "Not printed, for comparison" },
];

const strengthLine = (m: Material) => (m.process === "fdm" ? `${n(m.uts_xy_mpa)} MPa along layers · ${n(m.uts_z_mpa)} across` : `${n(m.uts_xy_mpa)} MPa strong`);

export function MaterialPicker({ materials, value, onChange }: { materials: Material[] | null; value: string; onChange(id: string): void }) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  /** a copy just saved, until the library has reloaded with it */
  const [saving, setSaving] = useState<string | null>(null);
  const picked = materials?.find((m) => m.id === value);
  const waiting = !!saving && !materials?.some((m) => m.id === saving);
  if (!materials) return <p className="text-sm text-muted-foreground">Loading materials…</p>;
  return (
    <div className="flex flex-col gap-2">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button variant="outline" role="combobox" aria-expanded={open} aria-label="Material" className="w-full justify-between font-normal" data-testid="material-picker">
            <span className="truncate">{picked?.name ?? "Pick a material…"}</span>
            <ChevronsUpDownIcon className="opacity-60" />
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-(--radix-popover-trigger-width) min-w-72 p-0" align="start">
          <Command>
            <CommandInput placeholder="Search: PETG, nylon, tough resin…" data-testid="material-search" />
            <CommandList className="max-h-80">
              <CommandEmpty>Nothing matches. Add your own material in Settings.</CommandEmpty>
              {GROUPS.map((g) => {
                const items = materials.filter((m) => m.process === g.process);
                if (!items.length) return null;
                return (
                  <CommandGroup key={g.process} heading={g.label}>
                    {items.map((m) => (
                      <CommandItem
                        key={m.id}
                        value={`${m.name} ${m.process} ${m.id}`}
                        onSelect={() => {
                          onChange(m.id);
                          setOpen(false);
                        }}
                        data-testid={`material-${m.id}`}
                      >
                        <CheckIcon className={cn("mt-0.5 self-start", m.id === value ? "opacity-100" : "opacity-0")} />
                        <span className="flex min-w-0 flex-col">
                          <span className="flex flex-wrap items-center gap-1.5">
                            {m.name}
                            {m.custom && <Badge tone="sky">Yours</Badge>}
                          </span>
                          <span className="num text-xs text-muted-foreground">
                            {strengthLine(m)} · {n(m.density / 1000, 3)} g/cm³
                          </span>
                        </span>
                      </CommandItem>
                    ))}
                  </CommandGroup>
                );
              })}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {picked && editing && !waiting && (
        <MaterialNumbers
          key={picked.id}
          material={picked}
          onDone={(id) => {
            if (id) {
              setSaving(id);
              onChange(id);
            }
            setEditing(false);
          }}
        />
      )}
      {waiting && <p className="text-sm text-muted-foreground">Saving your numbers…</p>}
      {picked && !editing && !waiting && (
        <div className="grid grid-cols-2 gap-2">
          <Stat label={picked.process === "fdm" ? "Along layers" : "Strength"} value={n(picked.uts_xy_mpa)} unit="MPa" />
          {picked.process === "fdm" ? <Stat label="Across layers" value={n(picked.uts_z_mpa)} unit="MPa" /> : <Stat label="Stiffness" value={n(picked.e_xy_mpa / 1000, 3)} unit="GPa" />}
          <p className="col-span-2 text-xs text-muted-foreground">
            {picked.process === "fdm"
              ? "Lab values for solid test bars. Real prints are weaker, most of all across layers, so SimLab checks safety against the across-layer number."
              : "Lab values for solid test bars. Real parts can be a little weaker."}
          </p>
          <Button size="xs" variant="outline" className="col-span-2 justify-self-start" onClick={() => setEditing(true)} data-testid="edit-material">
            <PencilIcon />
            {picked.custom ? "Change these numbers" : "Use my own numbers"}
          </Button>
          {picked.notes && (
            <Collapsible className="col-span-2">
              <CollapsibleTrigger className="group inline-flex items-center gap-1 text-xs text-muted-foreground underline-offset-2 hover:underline">
                <ChevronRightIcon className="size-3 transition-transform group-data-[state=open]:rotate-90" />
                Where these numbers come from
              </CollapsibleTrigger>
              <CollapsibleContent>
                <p className="pt-1.5 text-xs text-muted-foreground">{picked.notes}</p>
              </CollapsibleContent>
            </Collapsible>
          )}
        </div>
      )}
    </div>
  );
}

/** a material's numbers, editable: your own is updated, a built-in one is saved as your own copy */
function MaterialNumbers({ material: m, onDone }: { material: Material; onDone(id: string | null): void }) {
  const { api, reloadLibrary } = useApp();
  const [d, setD] = useState<Material>(() => ({ ...m, name: m.custom ? m.name : `${m.name} (mine)` }));
  /** what is typed in each box, kept while it is not a number yet (an emptied box stays empty) */
  const [text, setText] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const fdm = d.process === "fdm";
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!(d.uts_xy_mpa > 0 && d.e_xy_mpa > 0 && d.density > 0 && (!fdm || d.uts_z_mpa > 0))) {
      setError("Every number must be above zero.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const saved = await api.saveMaterial({
        ...d,
        id: m.custom ? m.id : "",
        name: d.name.trim() || `${m.name} (mine)`,
        custom: true,
        // one number for parts that are as strong every way
        // filament: across-layer stiffness keeps its share of the along-layer one
        e_z_mpa: fdm ? (m.e_z_mpa * d.e_xy_mpa) / (m.e_xy_mpa || d.e_xy_mpa) : d.e_xy_mpa,
        uts_z_mpa: fdm ? d.uts_z_mpa : d.uts_xy_mpa,
      });
      reloadLibrary();
      onDone(saved.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save");
      setBusy(false);
    }
  };
  const num = (label: string, key: "uts_xy_mpa" | "uts_z_mpa" | "e_xy_mpa" | "density", unit: string, k = 1) => (
    <Label className="flex flex-col items-stretch gap-1 text-xs font-medium text-muted-foreground">
      {label}
      <span className="flex items-center gap-1.5">
        <Input
          type="number"
          inputMode="decimal"
          step="any"
          min={0}
          className="num h-8"
          value={text[key] ?? (Number.isFinite(d[key]) ? String(Number((d[key] / k).toPrecision(6))) : "")}
          onChange={(e) => {
            const t = e.target.value;
            setText({ ...text, [key]: t });
            const v = Number(t);
            if (t.trim() !== "" && Number.isFinite(v)) setD({ ...d, [key]: v * k });
          }}
          required
          data-testid={`material-${key}`}
        />
        <span className="shrink-0 text-xs font-normal">{unit}</span>
      </span>
    </Label>
  );
  return (
    <form className="flex flex-col gap-3 rounded-lg border bg-sunken/50 p-3" onSubmit={submit} data-testid="material-numbers">
      <p className="text-xs text-soft">
        {m.custom ? "Your own material: changes apply to every test that uses it." : `${m.name} is built in, so your numbers are saved as your own copy. You find it later under the same search.`}
      </p>
      <Label className="flex flex-col items-stretch gap-1 text-xs font-medium text-muted-foreground">
        Name
        <Input className="h-8" value={d.name} onChange={(e) => setD({ ...d, name: e.target.value })} data-testid="material-name" />
      </Label>
      <div className="grid grid-cols-2 gap-2">
        {num(fdm ? "Strength along layers" : "Strength", "uts_xy_mpa", "MPa")}
        {fdm && num("Strength across layers", "uts_z_mpa", "MPa")}
        {num("Stiffness", "e_xy_mpa", "GPa", 1000)}
        {num("Density", "density", "g/cm³", 1000)}
      </div>
      <p className="text-2xs text-muted-foreground">On a data sheet, strength is "tensile strength" and stiffness is "tensile modulus" (also called Young's modulus).</p>
      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={busy} data-testid="save-material-numbers">
          {busy ? "Saving…" : m.custom ? "Save" : "Save as my own"}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => onDone(null)}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
