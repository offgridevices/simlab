// Picking a material: a search box over the library, grouped into printer
// filaments, resins and the rest, each with its strength along and across
// the printed layers.
import { useMemo, useState } from "react";
import { SearchIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { n } from "../lib/format";
import type { Material, Process } from "../lib/types";

const GROUPS: { process: Process; label: string }[] = [
  { process: "fdm", label: "Filament (FDM printers)" },
  { process: "resin", label: "Resin (SLA / MSLA printers)" },
  { process: "other", label: "Not printed, for comparison" },
];

export function MaterialPicker({ materials, value, onChange }: { materials: Material[] | null; value: string; onChange(id: string): void }) {
  const [q, setQ] = useState("");
  const list = useMemo(() => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    return (materials ?? []).filter((m) => words.every((w) => `${m.name} ${m.process} ${m.notes ?? ""}`.toLowerCase().includes(w)));
  }, [materials, q]);
  if (!materials) return <p className="text-sm text-muted-foreground">Loading materials…</p>;
  return (
    <div className="flex flex-col gap-2">
      <div className="relative">
        <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
        <Input className="pl-8" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search: PETG, nylon, tough resin…" aria-label="Search materials" data-testid="material-search" />
      </div>
      <div className="max-h-80 overflow-y-auto rounded-lg border" role="radiogroup" aria-label="Material">
        {GROUPS.map((g) => {
          const items = list.filter((m) => m.process === g.process);
          if (!items.length) return null;
          return (
            <div key={g.process}>
              <div className="sticky top-0 z-[1] border-b bg-card/95 px-3 py-1.5 text-xs font-medium text-muted-foreground backdrop-blur">{g.label}</div>
              {items.map((m) => {
                const on = m.id === value;
                return (
                  <button
                    key={m.id}
                    type="button"
                    role="radio"
                    aria-checked={on}
                    onClick={() => onChange(m.id)}
                    className={cn(
                      "flex w-full items-start gap-3 border-b px-3 py-2 text-left outline-none last:border-b-0 hover:bg-foreground/[0.03] focus-visible:bg-accent",
                      on && "bg-ember/[0.08] hover:bg-ember/[0.1]",
                    )}
                    data-testid={`material-${m.id}`}
                  >
                    <span aria-hidden="true" className={cn("mt-1 grid size-3.5 shrink-0 place-items-center rounded-full border border-input", on && "border-primary bg-primary")}>
                      {on && <span className="size-1.5 rounded-full bg-primary-foreground" />}
                    </span>
                    <span className="flex min-w-0 flex-1 flex-col">
                      <span className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
                        {m.name}
                        {m.custom && <Badge tone="sky">Yours</Badge>}
                      </span>
                      <span className="num text-xs text-muted-foreground">
                        {m.process === "fdm" ? (
                          <>
                            {n(m.uts_xy_mpa)} MPa along layers · {n(m.uts_z_mpa)} across
                          </>
                        ) : (
                          <>{n(m.uts_xy_mpa)} MPa strong</>
                        )}
                        {" · "}
                        {n(m.density / 1000, 3)} g/cm³
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          );
        })}
        {!list.length && <p className="px-3 py-4 text-sm text-muted-foreground">Nothing matches "{q}". Add your own material in Settings.</p>}
      </div>
    </div>
  );
}
