// The 3D view as a React component: the canvas, a small axis compass that
// doubles as a view cube (click an axis to look along it), preset views,
// zoom buttons, and the "where it breaks" ring. Everything the scene shows
// arrives as props; the scene itself (scene.ts) is imperative.
import { forwardRef, useEffect, useImperativeHandle, useRef, useState, type ReactNode } from "react";
import { Vector3, type Camera } from "three";
import { BoxIcon, MaximizeIcon, MinusIcon, PlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Loader } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { cssVar, useResolvedTheme } from "../lib/theme";
import type { ColorScale } from "../lib/colormaps";
import type { Vec3 } from "../lib/types";
import type { Tone } from "../lib/format";
import type { Prepared } from "./prepare";
import { SceneView, type ViewPreset } from "./scene";

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
  deform?: { disp: Float32Array; k: number } | null;
  arrows?: ToneArrow[];
  plate?: { up: Vec3 } | null;
  spin?: { center: Vec3; axis: Vec3; radius: number } | null;
  slices?: { key: string; slice: import("../lib/types").Slice; rgba: Uint8Array }[];
  critical?: Vec3 | null;
  hint?: ReactNode;
  /** small views (Compare): no compass, no zoom buttons */
  compact?: boolean;
  children?: ReactNode;
  className?: string;
  label?: string;
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

const toneVar = (t: Tone) => cssVar(t === "neutral" ? "--soft" : `--${t}`) || "#888";

const VIEWS: { id: ViewPreset; label: string }[] = [
  { id: "iso", label: "Three-quarter" },
  { id: "top", label: "Top" },
  { id: "front", label: "Front" },
  { id: "right", label: "Right side" },
  { id: "back", label: "Back" },
  { id: "left", label: "Left side" },
  { id: "bottom", label: "Bottom" },
];

export const Viewer = forwardRef<ViewerHandle, ViewerProps>(function Viewer(p, ref) {
  const host = useRef<HTMLDivElement>(null);
  const marker = useRef<HTMLDivElement>(null);
  const gizmo = useRef<SVGSVGElement>(null);
  const view = useRef<SceneView | null>(null);
  const [failed, setFailed] = useState("");
  const theme = useResolvedTheme();
  const handlers = useRef({ onPick: p.onPick, onHover: p.onHover });
  handlers.current = { onPick: p.onPick, onHover: p.onHover };

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    let v: SceneView;
    try {
      v = new SceneView(el, marker.current);
    } catch (e) {
      setFailed(e instanceof Error ? `The 3D view could not start: ${e.message}` : "The 3D view could not start");
      return;
    }
    view.current = v;
    window.__simlab3d = v.stats;
    v.onPick = (id) => handlers.current.onPick?.(id);
    v.onHover = (id) => handlers.current.onHover?.(id);
    v.onFrame = (cam) => drawGizmo(gizmo.current, cam);
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
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    v.setPartColor(cssVar("--part") || "#b9b0a0", cssVar("--ember") || "#ff6a00");
    v.setLights(theme === "dark");
  }, [theme, p.prepared]);
  useEffect(() => {
    view.current?.setMarks((p.marks ?? []).map((m) => ({ ids: m.ids, color: toneVar(m.tone), alpha: m.alpha })));
  }, [p.marks, p.prepared, theme]);
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    v.pickable = !!p.pickable;
    v.hoverable = !!p.hoverable || !!p.pickable;
    host.current?.style.setProperty("cursor", p.pickable ? "pointer" : "");
  }, [p.pickable, p.hoverable]);
  useEffect(() => {
    if (p.hoverId !== undefined) view.current?.setHoverId(p.hoverId);
  }, [p.hoverId]);
  useEffect(() => view.current?.setField(p.field?.values ?? null, p.field?.scale ?? null), [p.field, p.prepared]);
  useEffect(() => view.current?.setDeform(p.deform?.disp ?? null, p.deform?.k ?? 0), [p.deform, p.prepared]);
  useEffect(() => {
    view.current?.setOverlays({
      arrows: (p.arrows ?? []).map((a) => ({ origin: a.origin, dir: a.dir, length: a.length, color: toneVar(a.tone) })),
      plate: p.plate ? { up: p.plate.up, color: toneVar("sky") } : null,
      spin: p.spin ? { ...p.spin, color: toneVar("plum") } : null,
    });
  }, [p.arrows, p.plate, p.spin, p.prepared, theme]);
  useEffect(() => view.current?.setSlices(p.slices ?? []), [p.slices]);
  useEffect(() => view.current?.setCritical(p.critical ?? null), [p.critical, p.prepared]);

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

  const status = failed ? "error" : p.status;
  return (
    <div className={cn("@container relative min-h-0 min-w-0 flex-1 overflow-hidden bg-[radial-gradient(ellipse_at_50%_40%,var(--viewer-a)_0%,var(--viewer-b)_75%)]", p.className)}>
      <div
        ref={host}
        className="absolute inset-0 touch-none outline-none focus-visible:ring-[3px] focus-visible:ring-inset focus-visible:ring-ring/50"
        tabIndex={0}
        role="img"
        aria-label={p.label ?? "3D view of the part. Drag to turn it, scroll or pinch to zoom; arrow keys turn it, plus and minus zoom, 0 resets."}
        onKeyDown={onKey}
        data-testid="viewer"
        data-status={status}
      >
        {/* the ring that breathes out from where it breaks; moved every frame by the scene */}
        <div ref={marker} className="pointer-events-none absolute left-0 top-0 z-10" style={{ display: "none" }} aria-hidden="true">
          <svg width="56" height="56" viewBox="-28 -28 56 56" className="-translate-x-1/2 -translate-y-1/2 overflow-visible">
            <circle r="16" fill="none" stroke="var(--clay)" strokeWidth="3" className="hh-pulse" />
            <circle r="16" fill="none" stroke="var(--clay)" strokeWidth="3" className="hh-pulse [animation-delay:0.8s]" />
            <circle r="5" fill="var(--clay)" stroke="var(--card)" strokeWidth="2" />
          </svg>
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
      {p.hint && status === "ready" && (
        <span className="pointer-events-none absolute left-3 top-3 max-w-[calc(100%-6rem)] rounded-md bg-card/75 px-2 py-1 text-xs text-muted-foreground backdrop-blur">{p.hint}</span>
      )}
      {!p.compact && (
        <>
          <div className="absolute right-3 top-3 flex items-center gap-0.5 rounded-lg border bg-card/95 p-0.5 shadow-lg shadow-black/20 backdrop-blur">
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
          <div className="absolute bottom-3 right-3 flex items-center gap-0.5 rounded-lg border bg-card/95 p-0.5 shadow-lg shadow-black/20 backdrop-blur">
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
          <svg ref={gizmo} style={{ visibility: status === "ready" ? "visible" : "hidden" }} width="88" height="88" viewBox="-44 -44 88 88" className="absolute bottom-2 left-2 overflow-visible" role="group" aria-label="Axes: click one to look along it">
            {(["x", "y", "z"] as const).map((a) => (
              <g key={a} data-axis={a}>
                <line data-line x1="0" y1="0" x2="0" y2="0" stroke={`var(--${AXIS_TONE[a]})`} strokeWidth="2" strokeLinecap="round" />
                <g
                  data-pos
                  role="button"
                  tabIndex={0}
                  aria-label={`Look from +${a.toUpperCase()}`}
                  className="cursor-pointer outline-none [&:focus-visible>circle]:stroke-ring [&:focus-visible>circle]:stroke-[3]"
                  onClick={() => view.current?.lookFrom(AXIS_DIR[a])}
                  onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), view.current?.lookFrom(AXIS_DIR[a]))}
                >
                  <circle r="8.5" fill={`var(--${AXIS_TONE[a]})`} />
                  <text textAnchor="middle" dy="0.35em" className="fill-[#1b1813] text-[10px] font-semibold select-none">
                    {a.toUpperCase()}
                  </text>
                </g>
                <circle
                  data-neg
                  r="5"
                  fill="var(--card)"
                  stroke={`var(--${AXIS_TONE[a]})`}
                  strokeWidth="1.5"
                  className="cursor-pointer"
                  onClick={() => view.current?.lookFrom(AXIS_DIR[a].map((c) => -c) as Vec3)}
                >
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

const AXIS_TONE = { x: "clay", y: "moss", z: "sky" } as const;
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
    const line = g.querySelector("[data-line]")!;
    line.setAttribute("x2", String(rx * L));
    line.setAttribute("y2", String(-ry * L));
    g.querySelector("[data-pos]")!.setAttribute("transform", `translate(${rx * L} ${-ry * L})`);
    const neg = g.querySelector("[data-neg]")!;
    neg.setAttribute("cx", String(-rx * L * 0.7));
    neg.setAttribute("cy", String(ry * L * 0.7));
    items.push({ g, z: rz });
  }
  items.sort((a, b) => a.z - b.z).forEach(({ g }) => svg.appendChild(g));
}
