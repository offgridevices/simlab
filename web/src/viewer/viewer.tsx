// The 3D view as a React component: the canvas on a neutral backdrop, a
// ground grid with its square size, an axis compass that doubles as a view
// cube (click an axis to look along it), preset views, zoom buttons, the
// ring at the weakest spot, and a value probe: point at the part to read the
// exact value there, click to pin it (up to four). Everything the scene shows
// arrives as props; the scene itself (scene.ts) is imperative.
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type ReactNode } from "react";
import { Vector3, type Camera } from "three";
import { BoxIcon, HouseIcon, MaximizeIcon, MinusIcon, PinIcon, PlusIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Loader } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { cssVar, useResolvedTheme } from "../lib/theme";
import type { ColorScale } from "../lib/colormaps";
import type { RegionShape, Slice, Vec3 } from "../lib/types";
import { regionMask } from "../lib/regions";
import { n, type Tone } from "../lib/format";
import type { Prepared } from "./prepare";
import { SceneView, type ProbeHit, type Quat, type ViewPreset } from "./scene";

export interface ToneMark {
  ids: number[];
  tone: Tone;
  alpha?: number;
}
export interface ToneArrow {
  origin: Vec3;
  dir: Vec3;
  length?: number;
  tone: Tone;
}

export interface ViewerProps {
  prepared: Prepared | null;
  status: "loading" | "ready" | "error" | "empty";
  message?: string;
  marks?: ToneMark[];
  pickable?: boolean;
  hoverable?: boolean;
  onPick?(id: number): void;
  onHover?(id: number | null): void;
  /** a face group highlighted from outside (hovering its name in a list) */
  hoverId?: number | null;
  field?: { values: Float32Array; scale: ColorScale } | null;
  /** areas picked by shape, coloured where they cover the surface */
  regions?: { shape: RegionShape; tone: Tone; alpha?: number }[];
  /** the brush: clicking the part reports the point */
  onPoint?(p: Vec3): void;
  /** what the probe reads (the shown field's name and unit); null = no probe */
  probe?: { label: string; unit: string } | null;
  deform?: { disp: Float32Array; k: number } | null;
  arrows?: ToneArrow[];
  /** the printer's bed under the part */
  plate?: { up: Vec3 } | null;
  /** how the part is turned to sit on the bed (null: as drawn) */
  pose?: Quat | null;
  /** a test shown as it runs */
  live?: LiveProps | null;
  spin?: { center: Vec3; axis: Vec3; radius: number } | null;
  slices?: { key: string; slice: Slice; rgba: Uint8Array }[];
  critical?: Vec3 | null;
  hint?: ReactNode;
  /** the part's size, mm, for the readout in the corner */
  size?: Vec3 | null;
  /** small views (Compare): no compass, no zoom buttons, no probe */
  compact?: boolean;
  children?: ReactNode;
  className?: string;
  label?: string;
}

export interface LiveProps {
  spin?: { center: Vec3; axis: Vec3 } | null;
  flow?: { kind: "around" | "through"; dir: Vec3; center: Vec3; radius: number } | null;
  pulse?: boolean;
}

export interface ViewerHandle {
  focus(p: Vec3): void;
  view(v: ViewPreset): void;
  reset(): void;
}

declare global {
  interface Window {
    __simlab3d?: SceneView["stats"];
  }
}

const toneVar = (t: Tone) => cssVar(t === "neutral" ? "--soft" : `--${t}`);
const MAX_PINS = 4;

const VIEWS: { id: ViewPreset; label: string }[] = [
  { id: "iso", label: "Three-quarter" },
  { id: "top", label: "Top" },
  { id: "front", label: "Front" },
  { id: "right", label: "Right side" },
  { id: "back", label: "Back" },
  { id: "left", label: "Left side" },
  { id: "bottom", label: "Bottom" },
];

/** a value with its unit, as the probe shows it */
export function probeText(v: number, unit: string): string {
  if (!Number.isFinite(v)) return "no value";
  if (unit === "×") return v >= 100 ? "100× or more" : `${v.toFixed(2)}×`;
  return `${n(v, 3)}${unit ? ` ${unit}` : ""}`;
}

export const Viewer = forwardRef<ViewerHandle, ViewerProps>(function Viewer(p, ref) {
  const host = useRef<HTMLDivElement>(null);
  const anchors = useRef<HTMLDivElement>(null);
  const gizmo = useRef<SVGSVGElement>(null);
  const view = useRef<SceneView | null>(null);
  const [failed, setFailed] = useState("");
  const [grid, setGrid] = useState(0);
  const [hover, setHover] = useState<(ProbeHit & { x: number; y: number }) | null>(null);
  const [pins, setPins] = useState<ProbeHit[]>([]);
  const theme = useResolvedTheme();
  const handlers = useRef({ onPick: p.onPick, onHover: p.onHover, onPoint: p.onPoint });
  handlers.current = { onPick: p.onPick, onHover: p.onHover, onPoint: p.onPoint };
  const probing = !!p.probe && !!p.field && !p.compact;

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    let v: SceneView;
    try {
      v = new SceneView(el, anchors.current);
    } catch (e) {
      setFailed(e instanceof Error ? `The 3D view could not start: ${e.message}` : "The 3D view could not start");
      return;
    }
    view.current = v;
    window.__simlab3d = v.stats;
    v.onPick = (id) => handlers.current.onPick?.(id);
    v.onPoint = (pt) => handlers.current.onPoint?.(pt);
    v.onHover = (id) => handlers.current.onHover?.(id);
    v.onProbe = setHover;
    v.onProbePin = (hit) => setPins((ps) => [...ps, hit].slice(-MAX_PINS));
    v.onGrid = setGrid;
    v.onFrame = (cam) => drawGizmo(gizmo.current, cam);
    v.applyTheme();
    return () => {
      v.dispose();
      view.current = null;
    };
  }, []);

  useImperativeHandle(ref, () => ({
    focus: (pt) => view.current?.focus(pt),
    view: (k) => view.current?.view(k),
    reset: () => view.current?.reset(),
  }));

  useEffect(() => view.current?.setMesh(p.prepared), [p.prepared]);
  // every colour comes from the tokens: read them again when the theme changes
  useEffect(() => view.current?.applyTheme(), [theme, p.prepared]);
  useEffect(() => {
    view.current?.setMarks((p.marks ?? []).map((m) => ({ ids: m.ids, color: toneVar(m.tone), alpha: m.alpha })));
  }, [p.marks, p.prepared, theme]);
  // which triangles each area covers (same rule as the engine), worked out here
  const shapesKey = JSON.stringify(p.regions?.map((r) => r.shape) ?? []);
  const mask = useMemo(
    () => (p.prepared && p.regions?.length ? regionMask(p.prepared.positions, p.prepared.indices, p.regions.map((r) => r.shape)) : null),
    [p.prepared, shapesKey],
  );
  useEffect(() => {
    view.current?.setRegions({ mask, colors: (p.regions ?? []).map((r) => ({ color: toneVar(r.tone), alpha: r.alpha ?? 0.6 })) });
  }, [mask, theme, JSON.stringify(p.regions?.map((r) => [r.tone, r.alpha]))]);
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    v.pointing = !!p.onPoint;
    v.pickable = !!p.pickable;
    v.hoverable = !!p.hoverable || !!p.pickable;
    v.probing = probing && v.canProbe;
    host.current?.style.setProperty("cursor", p.pickable || p.onPoint || (probing && v.canProbe) ? "crosshair" : "");
    if (!probing) setHover(null);
  }, [p.pickable, p.hoverable, probing, !!p.onPoint]);
  useEffect(() => {
    if (p.hoverId !== undefined) view.current?.setHoverId(p.hoverId);
  }, [p.hoverId]);
  useEffect(() => view.current?.setField(p.field?.values ?? null, p.field?.scale ?? null), [p.field, p.prepared, theme]);
  // pins belong to one field: a new field (or result) clears them
  useEffect(() => setPins([]), [p.field?.values, p.prepared]);
  useEffect(() => view.current?.setDeform(p.deform?.disp ?? null, p.deform?.k ?? 0), [p.deform, p.prepared]);
  useEffect(() => {
    view.current?.setOverlays({
      arrows: (p.arrows ?? []).map((a) => ({ origin: a.origin, dir: a.dir, length: a.length, color: toneVar(a.tone) })),
      plate: p.plate ? { up: p.plate.up, color: toneVar("sky") } : null,
      spin: p.spin ? { ...p.spin, color: toneVar("plum") } : null,
    });
  }, [p.arrows, p.plate, p.spin, p.prepared, theme]);
  useEffect(() => view.current?.setSlices(p.slices ?? []), [p.slices]);
  const poseKey = JSON.stringify(p.pose ?? null);
  useEffect(() => view.current?.setPose(p.pose ?? null), [poseKey, p.prepared]);
  const liveKey = JSON.stringify(p.live ?? null);
  useEffect(() => {
    const l = p.live;
    view.current?.setLive(l ? { spin: l.spin, pulse: l.pulse, flow: l.flow ? { ...l.flow, color: toneVar("sky") } : null } : null);
  }, [liveKey, p.prepared, theme]);
  // markers moved: draw a frame so they land in place
  useEffect(() => view.current?.request(), [p.critical, pins]);

  const onKey = (e: React.KeyboardEvent) => {
    const v = view.current;
    if (!v || e.target !== host.current) return;
    const step = e.shiftKey ? 0.4 : 0.12;
    const k = e.key;
    if (k === "ArrowLeft") v.orbit(-step, 0);
    else if (k === "ArrowRight") v.orbit(step, 0);
    else if (k === "ArrowUp") v.orbit(0, step);
    else if (k === "ArrowDown") v.orbit(0, -step);
    else if (k === "+" || k === "=") v.zoom(0.8);
    else if (k === "-" || k === "_") v.zoom(1.25);
    else if (k === "0" || k === "Home") v.reset();
    else return;
    e.preventDefault();
  };
  const unpin = useCallback((i: number) => setPins((ps) => ps.filter((_, j) => j !== i)), []);

  const status = failed ? "error" : p.status;
  const unit = p.probe?.unit ?? "";
  return (
    <div className={cn("@container viewer-backdrop relative min-h-0 min-w-0 flex-1 overflow-hidden", p.className)}>
      <div
        ref={host}
        className="absolute inset-0 touch-none outline-none focus-visible:ring-[3px] focus-visible:ring-inset focus-visible:ring-ring/50"
        tabIndex={0}
        role="img"
        aria-label={p.label ?? "3D view of the part. Drag to turn it, scroll or pinch to zoom; arrow keys turn it, plus and minus zoom, 0 resets."}
        onKeyDown={onKey}
        data-testid="viewer"
        data-status={status}
        data-probe={probing ? "on" : "off"}
      >
        {/* points that follow the part: the weakest spot, pinned values; moved every frame by the scene */}
        <div ref={anchors} className="pointer-events-none absolute inset-0 z-10" aria-hidden="true">
          {p.critical && (
            <div data-anchor={p.critical.join(",")} className="absolute left-0 top-0">
              <svg width="56" height="56" viewBox="-28 -28 56 56" className="-translate-x-1/2 -translate-y-1/2 overflow-visible">
                <circle r="16" fill="none" stroke="var(--clay)" strokeWidth="3" className="hh-pulse" />
                <circle r="16" fill="none" stroke="var(--clay)" strokeWidth="3" className="hh-pulse [animation-delay:0.8s]" />
                <circle r="5" fill="var(--clay)" stroke="var(--card)" strokeWidth="2" />
              </svg>
            </div>
          )}
          {pins.map((pin, i) => (
            <div key={i} data-anchor={pin.pos.join(",")} className="absolute left-0 top-0">
              <span className="absolute -translate-x-1/2 -translate-y-1/2 size-2.5 rounded-full border-2 border-hud bg-hud-foreground" />
              <span className="num absolute left-2 -translate-y-full whitespace-nowrap rounded-sm bg-hud px-1.5 py-0.5 text-2xs text-hud-foreground shadow-float">
                {i + 1} · {probeText(pin.value, unit)}
              </span>
            </div>
          ))}
        </div>
      </div>
      {status !== "ready" && (
        <div className="pointer-events-none absolute inset-0 grid place-items-center p-6 text-center text-sm text-muted-foreground">
          {status === "loading" ? (
            <div className="flex flex-col items-center gap-3">
              <Loader />
              {p.message ?? "Loading the part"}
            </div>
          ) : (
            <span className="max-w-sm">{failed || p.message}</span>
          )}
        </div>
      )}

      {/* the readout corner: what to do, the part's size, the grid's square */}
      {status === "ready" && (
        <div className="pointer-events-none absolute left-3 top-3 flex max-w-[calc(100%-7rem)] flex-col items-start gap-1">
          {p.hint && <span className="max-w-full truncate rounded-sm bg-hud px-2 py-1 text-xs text-hud-muted shadow-float">{p.hint}</span>}
          {!p.compact && (p.size || grid > 0) && (
            <span className="num rounded-sm bg-hud px-2 py-1 text-2xs text-hud-muted shadow-float" data-testid="viewer-scale">
              {p.size && `${p.size.map((v) => n(v, 3)).join(" × ")} mm`}
              {p.size && grid > 0 && " · "}
              {grid > 0 && `grid ${n(grid)} mm`}
            </span>
          )}
          {pins.length > 0 && (
            <div className="pointer-events-auto flex flex-col gap-0.5 rounded-sm bg-hud px-2 py-1.5 text-2xs text-hud-foreground shadow-float" data-testid="pins">
              <span className="flex items-center gap-1 text-hud-muted">
                <PinIcon className="size-3" />
                {p.probe?.label}
              </span>
              {pins.map((pin, i) => (
                <span key={i} className="num flex items-center justify-between gap-3">
                  {i + 1}. {probeText(pin.value, unit)}
                  <Button variant="ghost" size="icon-xs" onClick={() => unpin(i)} aria-label={`Remove pin ${i + 1}`}>
                    <XIcon />
                  </Button>
                </span>
              ))}
            </div>
          )}
        </div>
      )}

      {hover && probing && (
        <div
          className="num pointer-events-none absolute z-20 rounded-sm bg-hud px-2 py-1 text-xs text-hud-foreground shadow-float"
          style={{ left: hover.x + 14, top: hover.y + 14 }}
          role="status"
          data-testid="probe"
        >
          {probeText(hover.value, unit)}
          <span className="block text-2xs text-hud-muted">{pins.length < MAX_PINS ? "click to pin" : "pins full: remove one"}</span>
        </div>
      )}

      {!p.compact && (
        <>
          <div className="absolute right-3 top-3 flex items-center gap-0.5 rounded-lg border bg-card/95 p-0.5 shadow-float backdrop-blur">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="sm" aria-label="Pick a view" data-testid="view-presets">
                  <BoxIcon />
                  <span className="max-sm:sr-only">View</span>
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuLabel>Look from</DropdownMenuLabel>
                {VIEWS.map((v) => (
                  <DropdownMenuItem key={v.id} onSelect={() => view.current?.view(v.id)}>
                    {v.label}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
          <div className="absolute bottom-3 right-3 flex items-center gap-0.5 rounded-lg border bg-card/95 p-0.5 shadow-float backdrop-blur">
            <Button variant="ghost" size="icon-sm" onClick={() => view.current?.zoom(1.25)} aria-label="Zoom out">
              <MinusIcon />
            </Button>
            <Button variant="ghost" size="icon-sm" onClick={() => view.current?.reset()} aria-label="Fit the part in view" data-testid="reset-view">
              <MaximizeIcon />
            </Button>
            <Button variant="ghost" size="icon-sm" onClick={() => view.current?.zoom(0.8)} aria-label="Zoom in">
              <PlusIcon />
            </Button>
          </div>
          <Button
            variant="ghost"
            size="icon-xs"
            style={{ visibility: status === "ready" ? "visible" : "hidden" }}
            className="absolute bottom-2 left-[5.75rem] rounded-full border bg-card/95 shadow-float backdrop-blur"
            onClick={() => view.current?.reset()}
            aria-label="Home view: the whole part, from the front corner"
            title="Home view (or double-click the axes)"
            data-testid="home-view"
          >
            <HouseIcon />
          </Button>
          <svg
            ref={gizmo}
            onDoubleClick={(e) => {
              e.preventDefault();
              view.current?.reset();
            }}
            style={{ visibility: status === "ready" ? "visible" : "hidden" }}
            width="88"
            height="88"
            viewBox="-44 -44 88 88"
            className="absolute bottom-2 left-2 select-none overflow-visible"
            // a double-click must not select words elsewhere on the page
            onMouseDown={(e) => e.detail > 1 && e.preventDefault()}
            role="group"
            aria-label="The printer's axes: click one to look along it, double-click for the home view"
          >
            {(["x", "y", "z"] as const).map((a) => (
              <g key={a} data-axis={a}>
                <line data-line x1="0" y1="0" x2="0" y2="0" stroke={`var(--axis-${a})`} strokeWidth="2" strokeLinecap="round" />
                <g
                  data-pos
                  role="button"
                  tabIndex={0}
                  aria-label={`Look from +${a.toUpperCase()}`}
                  className="cursor-pointer outline-none [&:focus-visible>circle]:stroke-ring [&:focus-visible>circle]:stroke-[3]"
                  onClick={() => view.current?.lookFrom(AXIS_DIR[a])}
                  onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), view.current?.lookFrom(AXIS_DIR[a]))}
                >
                  <circle r="8.5" fill={`var(--axis-${a})`} />
                  <text textAnchor="middle" dy="0.35em" className="select-none fill-on-color text-2xs font-semibold">
                    {a.toUpperCase()}
                  </text>
                </g>
                <circle data-neg r="5" fill="var(--card)" stroke={`var(--axis-${a})`} strokeWidth="1.5" className="cursor-pointer" onClick={() => view.current?.lookFrom(AXIS_DIR[a].map((c) => -c) as Vec3)}>
                  <title>{`Look from −${a.toUpperCase()}`}</title>
                </circle>
              </g>
            ))}
          </svg>
        </>
      )}
      {p.children}
    </div>
  );
});

const AXIS_DIR: Record<"x" | "y" | "z", Vec3> = { x: [1, 0, 0], y: [0, 1, 0], z: [0, 0, 1] };

/** turn the compass to match the camera; the axis nearest the viewer is drawn last */
function drawGizmo(svg: SVGSVGElement | null, cam: Camera) {
  if (!svg) return;
  const inv = cam.quaternion.clone().invert();
  const items: { g: SVGGElement; z: number }[] = [];
  for (const a of ["x", "y", "z"] as const) {
    const g = svg.querySelector<SVGGElement>(`[data-axis="${a}"]`);
    if (!g) continue;
    const d = AXIS_DIR[a];
    const { x: rx, y: ry, z: rz } = new Vector3(d[0], d[1], d[2]).applyQuaternion(inv);
    const L = 30;
    g.querySelector("[data-line]")!.setAttribute("x2", String(rx * L));
    g.querySelector("[data-line]")!.setAttribute("y2", String(-ry * L));
    g.querySelector("[data-pos]")!.setAttribute("transform", `translate(${rx * L} ${-ry * L})`);
    const neg = g.querySelector("[data-neg]")!;
    neg.setAttribute("cx", String(-rx * L * 0.7));
    neg.setAttribute("cy", String(ry * L * 0.7));
    items.push({ g, z: rz });
  }
  items.sort((a, b) => a.z - b.z).forEach(({ g }) => svg.appendChild(g));
}
