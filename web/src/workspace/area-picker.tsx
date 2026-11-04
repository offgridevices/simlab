// Picking where something holds or pushes: whole faces (click them), or an
// area by shape for smooth parts that are one big face: the brush (click a
// point, set its size) and, for a propeller, "Hold the hub" (a round area
// around the spin axis). Areas are regions in the config (docs/API.md);
// their ids sit in the same list as face ids.
import { useMemo } from "react";
import { CircleDotIcon, MousePointerClickIcon, PaintbrushIcon, XIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { n, patchLabel } from "../lib/format";
import { coverage, isRegion, nextRegionId, regionLabel } from "../lib/regions";
import type { Model, Patch, Region, Vec3 } from "../lib/types";
import type { Prepared } from "../viewer/prepare";
import { NumberField } from "./common";
import { useSceneCtl } from "./scene-state";

export type PickMode = "faces" | "brush";

export function AreaPicker({
  ids,
  regions,
  onChange,
  patches,
  prepared,
  mode,
  onMode,
  brush,
  onBrush,
  active,
  hub,
  empty,
}: {
  ids: number[];
  regions: Region[];
  /** the new list of ids, and the config's new regions */
  onChange(ids: number[], regions: Region[]): void;
  patches: Patch[];
  prepared: Prepared | null;
  mode: PickMode;
  onMode(m: PickMode): void;
  /** brush diameter, mm */
  brush: number;
  onBrush(d: number): void;
  /** the area the brush slider changes */
  active: number | null;
  /** offer "Hold the hub" (a propeller) */
  hub?: { center: Vec3; axis: Vec3; radius: number } | null;
  empty: string;
}) {
  const { setHover } = useSceneCtl();
  const mine = regions.filter((r) => ids.includes(r.id));
  const hasHub = mine.some((r) => r.shape.type === "cylinder");
  const remove = (id: number) => {
    const next = ids.filter((x) => x !== id);
    onChange(next, regions);
  };
  const setRegion = (r: Region) => onChange(ids, regions.map((x) => (x.id === r.id ? r : x)));
  const addHub = () => {
    if (!hub) return;
    const id = nextRegionId(regions);
    onChange([...ids, id], [...regions, { id, label: "Hub", shape: { type: "cylinder", axis: hub.axis, center_mm: hub.center, radius_mm: hub.radius } }]);
  };
  const resize = (d: number) => {
    onBrush(d);
    const r = mine.find((x) => x.id === active);
    if (r && r.shape.type === "sphere") setRegion({ ...r, shape: { ...r.shape, radius_mm: d / 2 } });
  };

  return (
    <div className="flex flex-col gap-3">
      <ToggleGroup type="single" value={mode} onValueChange={(v) => v && onMode(v as PickMode)} aria-label="How to pick" data-testid="pick-mode">
        <ToggleGroupItem value="faces">
          <MousePointerClickIcon />
          Faces
        </ToggleGroupItem>
        <ToggleGroupItem value="brush">
          <PaintbrushIcon />
          Brush
        </ToggleGroupItem>
      </ToggleGroup>
      <p className="text-xs text-muted-foreground">
        {mode === "faces"
          ? "Click whole faces in the 3D view. Smooth parts are often one big face: use the brush for those."
          : "Click the part to cover the area around that point. The slider sets how big the newest area is."}
      </p>
      {mode === "brush" && (
        <Label className="gap-3 font-normal">
          <span className="w-24 shrink-0 text-xs text-muted-foreground">Brush size</span>
          <Slider min={0.5} max={Math.max(4, brush * 4)} step={0.5} value={[brush]} onValueChange={([v]) => resize(v!)} aria-label="Brush size in mm across" data-testid="brush-size" />
          <span className="num w-16 shrink-0 text-right text-xs">{n(brush, 3)} mm</span>
        </Label>
      )}
      {hub && !hasHub && (
        <Button variant="outline" size="sm" className="self-start" onClick={addHub} data-testid="hold-hub">
          <CircleDotIcon />
          Hold the hub ({n(hub.radius * 2, 3)} mm across)
        </Button>
      )}
      {ids.length === 0 ? (
        <p className="rounded-lg bg-sunken px-3 py-2.5 text-sm text-muted-foreground">{empty}</p>
      ) : (
        <ul className="flex flex-col gap-1.5" aria-label="Picked">
          {ids.map((id) => {
            const r = regions.find((x) => x.id === id);
            if (r) return <RegionChip key={id} region={r} prepared={prepared} onRemove={() => remove(id)} onChange={setRegion} />;
            const p = patches.find((x) => x.id === id);
            const label = p ? patchLabel(p, prepared?.holes.get(id)) : `Face ${id}`;
            return (
              <li key={id} onMouseEnter={() => setHover(id)} onMouseLeave={() => setHover(null)}>
                <Badge className="max-w-full pr-0.5">
                  <span className="truncate">{label}</span>
                  <Button variant="ghost" size="icon-xs" className="rounded-full" onClick={() => remove(id)} aria-label={`Remove ${label}`}>
                    <XIcon />
                  </Button>
                </Badge>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function RegionChip({ region: r, prepared, onRemove, onChange }: { region: Region; prepared: Prepared | null; onRemove(): void; onChange(r: Region): void }) {
  const key = JSON.stringify(r.shape);
  const covers = useMemo(() => (prepared ? coverage(prepared.positions, prepared.indices, r.shape) : null), [prepared, key]);
  return (
    <li className="flex flex-col gap-1.5 rounded-md border bg-sunken/60 px-2.5 py-2" data-testid="region-chip">
      <span className="flex items-center gap-2 text-sm">
        <Badge tone="sky">Area</Badge>
        <span className="min-w-0 flex-1 truncate">{regionLabel(r)}</span>
        <Button variant="ghost" size="icon-xs" onClick={onRemove} aria-label={`Remove ${regionLabel(r)}`}>
          <XIcon />
        </Button>
      </span>
      {r.shape.type === "cylinder" && (
        <span className="flex items-center gap-2 text-xs text-muted-foreground">
          Across
          <NumberField label="Hub area, mm across" width="w-20" unit="mm" min={0.1} value={Number((r.shape.radius_mm * 2).toFixed(3))} onChange={(v) => r.shape.type === "cylinder" && onChange({ ...r, shape: { ...r.shape, radius_mm: v / 2 } })} />
        </span>
      )}
      {covers !== null && (
        <span className={covers ? "num text-2xs text-muted-foreground" : "text-2xs text-gold-text"}>
          {covers ? `covers ${n(covers)} bits of surface` : "covers no surface: click on the part, or make it bigger"}
        </span>
      )}
    </li>
  );
}

export { isRegion };
