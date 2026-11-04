// What the 3D view shows is decided by whichever panel is open on the right:
// the panel says which surface (the model, or a run's result), which faces to
// colour, what clicking a face does, the field, arrows and so on. The
// workspace owns the view and merges what the panel asked for.
import { createContext, useContext, useEffect, useRef } from "react";
import type { ColorScale } from "../lib/colormaps";
import type { RegionShape, Slice, Vec3 } from "../lib/types";
import type { Tone } from "../lib/format";
import type { ToneArrow, ToneMark, ViewerHandle } from "../viewer/viewer";

export interface SceneState {
  /** whose surface: the project's model, or a finished run's result surface */
  source: { kind: "model"; modelId?: string } | { kind: "run"; runId: string };
  marks: ToneMark[];
  /** clicking a face calls this; undefined = faces are not clickable */
  onPick?: (id: number) => void;
  /** the brush: clicking the part gives the 3D point (takes over from onPick) */
  onPoint?: (p: Vec3) => void;
  /** areas picked by shape, coloured where they cover the surface */
  regions: { shape: RegionShape; tone: Tone; alpha?: number }[];
  hoverable: boolean;
  field: { values: Float32Array; scale: ColorScale } | null;
  /** point at the part to read the shown field: its name and unit */
  probe: { label: string; unit: string } | null;
  deform: { disp: Float32Array; k: number } | null;
  arrows: ToneArrow[];
  plate: { up: Vec3 } | null;
  spin: { center: Vec3; axis: Vec3; radius: number } | null;
  slices: { key: string; slice: Slice; rgba: Uint8Array }[];
  critical: Vec3 | null;
  /** a line over the top-left of the view */
  hint: string | null;
}

export const DEFAULT_SCENE: SceneState = {
  source: { kind: "model" },
  marks: [],
  regions: [],
  hoverable: true,
  field: null,
  probe: null,
  deform: null,
  arrows: [],
  plate: null,
  spin: null,
  slices: [],
  critical: null,
  hint: null,
};

export interface SceneCtl {
  set(owner: symbol, s: Partial<SceneState> | null): void;
  viewer: React.RefObject<ViewerHandle | null>;
  /** the face the pointer is over in the 3D view, for panels that name it */
  hovered: number | null;
  /** highlight a face from a panel (hovering its name) */
  setHover(id: number | null): void;
  /** where panels portal things drawn over the view (a colour legend) */
  overlay: HTMLElement | null;
}

export const SceneContext = createContext<SceneCtl | null>(null);

export function useSceneCtl(): SceneCtl {
  const c = useContext(SceneContext);
  if (!c) throw new Error("useSceneCtl outside the workspace");
  return c;
}

/**
 * Ask the 3D view to show something while this panel is open. The latest
 * call wins; the request is withdrawn when the panel closes. `onPick` may
 * change every render: the newest one is always called.
 */
export function useScene(s: Partial<SceneState>) {
  const ctl = useSceneCtl();
  const owner = useRef(Symbol("scene")).current;
  const pick = useRef(s.onPick);
  pick.current = s.onPick;
  const point = useRef(s.onPoint);
  point.current = s.onPoint;
  const hasPick = !!s.onPick;
  const hasPoint = !!s.onPoint;
  const { onPick: _p, onPoint: _q, ...rest } = s;
  void _p;
  void _q;
  // typed arrays by identity (they come from caches and memos), everything else by value
  const key = JSON.stringify(rest, (_k, v) => (v instanceof Float32Array || v instanceof Uint8Array ? `${v.constructor.name}:${v.length}:${identity(v)}` : v));
  useEffect(() => {
    ctl.set(owner, { ...s, onPick: hasPick ? (id: number) => pick.current?.(id) : undefined, onPoint: hasPoint ? (p: Vec3) => point.current?.(p) : undefined });
    // `key` stands for everything in `s`
  }, [key, hasPick, hasPoint]);
  useEffect(() => () => ctl.set(owner, null), []);
}

const ids = new WeakMap<object, number>();
let next = 1;
function identity(o: object): number {
  let i = ids.get(o);
  if (!i) {
    i = next++;
    ids.set(o, i);
  }
  return i;
}
