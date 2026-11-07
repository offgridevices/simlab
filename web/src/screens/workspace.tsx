// One project: a rail of views on the left (Model, Tests, Compare), the 3D
// view in the middle, the setup or results of what is selected on the
// right. On a phone, the 3D view and the panel are two panes switched at the
// bottom of the screen.
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { BoxIcon, CheckIcon, FlaskConicalIcon, PencilIcon, TableIcon } from "lucide-react";
import { Screen, TopBar, Centre } from "../components/topbar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Drawer, DrawerContent, DrawerDescription, DrawerTitle } from "@/components/ui/drawer";
import { Loader } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useApp } from "../lib/app";
import { href, navigate } from "../lib/router";
import { useAsync, useProject, useSurface, type ProjectData } from "../lib/hooks";
import { Viewer, type ViewerHandle } from "../viewer/viewer";
import { prepare } from "../viewer/prepare";
import { DEFAULT_SCENE, SceneContext, type SceneCtl, type SceneState } from "../workspace/scene-state";
import { ModelPanel } from "../workspace/model-panel";
import { TestsPanel } from "../workspace/tests-panel";
import { CompareView } from "../workspace/compare";
import type { Model } from "../lib/types";

export type View = "model" | "tests" | "compare";
/** where the phone's panel sheet rests: a peek over the 3D view, or nearly full height */
const SNAPS = [0.45, 0.94];
/** the sheet's own height, a share of the window. Vaul moves the sheet down by (1 − snap) of the
 * window from where it sits, so (snap − (1 − SHEET)) of the window shows; the panel must fit in that */
const SHEET = 0.94;
/** the grab handle above the panel: 1rem gap + 0.375rem bar */
const HANDLE = "1.375rem";

function useMediaQuery(q: string): boolean {
  const [on, setOn] = useState(() => typeof matchMedia !== "undefined" && matchMedia(q).matches);
  useEffect(() => {
    const m = matchMedia(q);
    const f = () => setOn(m.matches);
    m.addEventListener("change", f);
    f();
    return () => m.removeEventListener("change", f);
  }, [q]);
  return on;
}
const RAIL: { id: View; label: string; icon: ReactNode }[] = [
  { id: "model", label: "Model", icon: <BoxIcon /> },
  { id: "tests", label: "Tests", icon: <FlaskConicalIcon /> },
  { id: "compare", label: "Compare", icon: <TableIcon /> },
];

export interface Nav {
  view: View;
  study: string | null;
  run: string | null;
  /** a new test being set up: "pick" (choose a kind) or the kind */
  create: string | null;
  /** a pre-filled setup to start from (JSON, from "Test strength at this RPM" and the model suggestions) */
  preset: string | null;
  /** the part shown, when the project has more than one */
  model: string | null;
  go(to: Partial<Record<"view" | "study" | "run" | "new" | "preset" | "model", string | null>>, replace?: boolean): void;
}

export default function Workspace({ id, query }: { id: string; query: URLSearchParams }) {
  const { data, error, reload, setData } = useProject(id);
  const view = (["model", "tests", "compare"].includes(query.get("view") ?? "") ? query.get("view") : data && data.studies.length ? "tests" : "model") as View;

  const nav: Nav = useMemo(
    () => ({
      view,
      study: query.get("study"),
      run: query.get("run"),
      create: query.get("new"),
      preset: query.get("preset"),
      model: query.get("model"),
      go(to, replace = false) {
        const q = new URLSearchParams(query);
        for (const [k, v] of Object.entries(to)) {
          if (v === null || v === undefined || v === "") q.delete(k);
          else q.set(k, v);
        }
        if (to.view && to.view !== view) for (const k of ["study", "run", "new", "preset"]) if (!(k in to)) q.delete(k);
        navigate(`/p/${id}?${q}`, replace);
      },
    }),
    [query, view, id],
  );

  if (error && !data)
    return (
      <Screen>
        <TopBar back />
        <Centre>
          <div className="flex max-w-md flex-col items-start gap-4">
            <h1 className="text-2xl font-semibold tracking-tight">This project could not be opened</h1>
            <p className="text-soft">{error}</p>
            <Button variant="outline" asChild>
              <a href={href("/")}>All projects</a>
            </Button>
          </div>
        </Centre>
      </Screen>
    );
  if (!data)
    return (
      <Screen>
        <TopBar back />
        <Centre>
          <Loader />
        </Centre>
      </Screen>
    );
  return <Loaded data={data} nav={nav} reload={reload} setData={setData} />;
}

function Loaded({
  data,
  nav,
  reload,
  setData,
}: {
  data: ProjectData;
  nav: Nav;
  reload(): void;
  setData: React.Dispatch<React.SetStateAction<ProjectData | null>>;
}) {
  const { api } = useApp();
  const viewer = useRef<ViewerHandle | null>(null);
  const phone = useMediaQuery("(max-width: 699px)");
  const [snap, setSnap] = useState<number | string | null>(SNAPS[0]!);
  const [overlay, setOverlay] = useState<HTMLDivElement | null>(null);
  const [requests, setRequests] = useState<Map<symbol, Partial<SceneState>>>(() => new Map());
  const [hovered, setHovered] = useState<number | null>(null);
  const [hoverFromPanel, setHoverFromPanel] = useState<number | null>(null);

  const ctl: SceneCtl = useMemo(
    () => ({
      set(owner, s) {
        setRequests((m) => {
          const n = new Map(m);
          n.delete(owner);
          if (s) n.set(owner, s);
          return n;
        });
      },
      viewer,
      hovered,
      setHover: setHoverFromPanel,
      overlay,
    }),
    [hovered, overlay],
  );
  const scene: SceneState = useMemo(() => Object.assign({}, DEFAULT_SCENE, ...requests.values()), [requests]);

  // which surface: a model of this project, or a run's result surface. A
  // run's part is the model its test was set up on, never the one picked in
  // the Model panel: its size and faces label the result.
  const src = scene.source;
  const runModelId = src.kind === "run" ? data.studies.find((s) => s.runs.some((r) => r.id === src.runId))?.model_id : undefined;
  const model: Model | undefined =
    (src.kind === "model" && src.modelId ? data.models.find((m) => m.id === src.modelId) : undefined) ??
    (src.kind === "run" ? data.models.find((m) => m.id === runModelId) : undefined) ??
    (src.kind === "run" && runModelId ? undefined : data.models.find((m) => m.id === nav.model) ?? data.models[0]);
  // the part a panel works on: the one picked in the Model panel, or the first
  const current = data.models.find((m) => m.id === nav.model) ?? data.models[0];
  const runId = scene.source.kind === "run" ? scene.source.runId : null;
  const surf = useSurface(runId ? null : model);
  const runMesh = useAsync(runId ? `runmesh:${runId}` : null, () => api.runMesh(runId!));
  const source = runId ? runMesh : surf;
  // rebuilt only when the surface itself changes, not on every project reload (a new
  // patches array each time); a part's faces only change with its surface
  const patchesKey = runId ? "" : `${model?.id}:${model?.unit_mm}:${model?.patches?.length ?? 0}`;
  const prepared = useMemo(() => (source.value ? prepare(source.value, runId ? undefined : model?.patches) : null), [source.value, runId, patchesKey]);
  const status: "loading" | "ready" | "error" | "empty" = !runId && !model ? "empty" : source.error ? "error" : !prepared || (!runId && model?.status !== "ready") ? (model?.status === "failed" ? "error" : "loading") : "ready";
  const message =
    status === "empty"
      ? "No part in this project yet. Add one from the Model panel."
      : status === "error"
        ? source.error || model?.message || "The part could not be read"
        : !runId && model?.status === "processing"
          ? model.message || "Reading the part and finding its faces"
          : !runId && model?.status === "waiting"
            ? "Waiting for Gmsh. Set it up in the Model panel and this part opens by itself."
          : runId
            ? "Loading the result"
            : `Loading the part${model?.triangles ? ` (${model.triangles.toLocaleString("en-US")} triangles)` : ""}`;

  const onHover = useCallback((id: number | null) => setHovered(id), []);
  const rename = async (name: string) => {
    const p = await api.updateProject(data.project.id, { name });
    setData((d) => (d ? { ...d, project: { ...d.project, ...p } } : d));
  };

  const panel =
    nav.view === "model" ? (
      <ModelPanel data={data} model={current} nav={nav} prepared={prepared} reload={reload} />
    ) : nav.view === "tests" ? (
      <TestsPanel data={data} model={current} nav={nav} prepared={prepared} reload={reload} />
    ) : null;

  const running = data.studies.some((s) => s.runs.some((r) => r.status === "running" || r.status === "queued"));

  return (
    <SceneContext.Provider value={ctl}>
      <div className="flex h-dvh flex-col overflow-hidden">
        <TopBar back>
          <ProjectName name={data.project.name} onSave={rename} />
        </TopBar>
        <div className="flex min-h-0 flex-1 flex-col gap-2 p-2 min-[700px]:flex-row min-[700px]:gap-3 min-[700px]:p-3">
          <nav className="flex shrink-0 gap-1 overflow-x-auto min-[700px]:flex-col min-[700px]:overflow-visible [scrollbar-width:none]" role="tablist" aria-orientation="vertical" aria-label="Views">
            {RAIL.map((t) => {
              const selected = nav.view === t.id;
              return (
                <Tooltip key={t.id}>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      role="tab"
                      aria-selected={selected}
                      onClick={() => nav.go({ view: t.id })}
                      data-testid={`rail-${t.id}`}
                      className={cn(
                        "relative h-11 shrink-0 rounded-lg px-3 text-muted-foreground min-[700px]:w-11 min-[700px]:px-0 [&_svg:not([class*='size-'])]:size-5",
                        selected && "bg-ember/15 text-ember-text hover:bg-ember/20 hover:text-ember-text",
                      )}
                    >
                      {t.icon}
                      <span className="min-[700px]:sr-only">{t.label}</span>
                      {t.id === "tests" && running && (
                        <span aria-hidden="true" className="absolute right-1 top-1 size-2.5 animate-spin rounded-full border-[1.5px] border-ember border-t-transparent [animation-duration:1.2s]" />
                      )}
                      {t.id === "tests" && running && <span className="sr-only"> (a test is running)</span>}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="right" className="max-[699px]:hidden">
                    {t.label}
                  </TooltipContent>
                </Tooltip>
              );
            })}
          </nav>

          {nav.view === "compare" ? (
            <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl border bg-card" aria-label="Compare runs">
              <CompareView data={data} />
            </section>
          ) : (
            <>
              <section
                className={cn("relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl border bg-card", phone && "mb-[calc(45dvh-3rem)]")}
                aria-label="3D view"
              >
                <Viewer
                  ref={viewer}
                  prepared={status === "ready" ? prepared : null}
                  status={status}
                  message={message}
                  marks={scene.marks}
                  pickable={!!scene.onPick}
                  hoverable={scene.hoverable}
                  onPick={scene.onPick}
                  onPoint={scene.onPoint}
                  regions={scene.regions}
                  onHover={onHover}
                  hoverId={hoverFromPanel}
                  field={scene.field}
                  probe={scene.probe}
                  size={model?.size_mm ?? null}
                  deform={scene.deform}
                  arrows={scene.arrows}
                  plate={scene.plate}
                  pose={scene.pose}
                  live={scene.live}
                  spin={scene.spin}
                  slices={scene.slices}
                  critical={scene.critical}
                  hint={scene.hint ?? "Drag to turn · scroll or pinch to zoom"}
                >
                  <div ref={setOverlay} className="pointer-events-none absolute inset-x-3 top-20 flex justify-center @min-[560px]:top-auto @min-[560px]:bottom-3 @min-[560px]:left-24 @min-[560px]:right-36" />
                </Viewer>
              </section>
              {phone ? (
                // on a phone the panel is a sheet over the 3D view: drag it up to read, down to look
                <Drawer open modal={false} dismissible={false} snapPoints={SNAPS} activeSnapPoint={snap} setActiveSnapPoint={setSnap}>
                  <DrawerContent overlay={false} className="h-[94dvh] border-x data-[vaul-drawer-direction=bottom]:max-h-none data-[vaul-drawer-direction=bottom]:mt-0" data-testid="panel-sheet">
                    <DrawerTitle className="sr-only">{nav.view === "model" ? "Model" : "Tests"}</DrawerTitle>
                    <DrawerDescription className="sr-only">Drag up to read more, down to see the part</DrawerDescription>
                    <div className="flex min-h-0 flex-1 flex-col" style={{ maxHeight: `calc(${((typeof snap === "number" ? snap : SNAPS[0]!) - (1 - SHEET)) * 100}dvh - ${HANDLE})` }}>
                      {panel}
                    </div>
                  </DrawerContent>
                </Drawer>
              ) : (
                <aside className="flex min-h-0 w-[320px] shrink-0 flex-col overflow-hidden rounded-xl border bg-card lg:w-[360px] xl:w-[400px]" aria-label={nav.view === "model" ? "Model" : "Tests"}>
                  {panel}
                </aside>
              )}
            </>
          )}
        </div>

      </div>
    </SceneContext.Provider>
  );
}

/** the project's name; click to rename */
function ProjectName({ name, onSave }: { name: string; onSave(n: string): Promise<void> }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(name);
  const [busy, setBusy] = useState(false);
  useEffect(() => setValue(name), [name]);
  const save = async () => {
    const v = value.trim();
    if (!v || v === name) {
      setEditing(false);
      setValue(name);
      return;
    }
    setBusy(true);
    try {
      await onSave(v);
      setEditing(false);
    } finally {
      setBusy(false);
    }
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") save();
    if (e.key === "Escape") {
      setValue(name);
      setEditing(false);
    }
  };
  if (editing)
    return (
      <span className="flex min-w-0 items-center gap-1">
        <Input
          autoFocus
          className="h-8 min-w-0 max-w-72 flex-1 px-2 font-medium"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={onKey}
          onBlur={save}
          aria-label="Project name"
          disabled={busy}
          data-testid="project-name-input"
        />
        <Button size="icon-sm" variant="ghost" onMouseDown={(e) => e.preventDefault()} onClick={save} aria-label="Save the name">
          <CheckIcon />
        </Button>
      </span>
    );
  return (
    <Button variant="ghost" size="sm" className="group min-w-0 justify-start px-1.5 text-foreground" onClick={() => setEditing(true)} title="Rename the project" data-testid="project-name">
      <span className="truncate">{name}</span>
      <PencilIcon className="size-3.5 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" aria-hidden="true" />
    </Button>
  );
}
