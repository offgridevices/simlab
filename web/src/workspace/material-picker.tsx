// Picking a material: a shadcn Combobox (Popover + Command) over the library,
// grouped into printer filaments, resins and the rest, each with its
// strength along and across the printed layers. The chosen one is summed up
// under the button.
import { useState } from "react";
import { CheckIcon, ChevronRightIcon, ChevronsUpDownIcon } from "lucide-react";
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
  const picked = materials?.find((m) => m.id === value);
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
      {picked && (
        <div className="grid grid-cols-2 gap-2">
          <Stat label={picked.process === "fdm" ? "Along layers" : "Strength"} value={n(picked.uts_xy_mpa)} unit="MPa" />
          {picked.process === "fdm" ? <Stat label="Across layers" value={n(picked.uts_z_mpa)} unit="MPa" /> : <Stat label="Stiffness" value={n(picked.e_xy_mpa / 1000, 3)} unit="GPa" />}
          <p className="col-span-2 text-xs text-muted-foreground">
            {picked.process === "fdm"
              ? "Lab values for solid test bars. Real prints are weaker, most of all across layers, so SimLab checks safety against the across-layer number."
              : "Lab values for solid test bars. Real parts can be a little weaker."}
          </p>
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
