// Projects: drop a part to start, or try an example; below, every project
// with its last result.
import { useEffect, useRef, useState, type DragEvent } from "react";
import { BoxIcon, FlaskConicalIcon, Trash2Icon, UploadIcon } from "lucide-react";
import { Screen, TopBar } from "../components/topbar";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Loader } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useApp, useEvents } from "../lib/app";
import { href, navigate } from "../lib/router";
import { ago, pillFor } from "../lib/format";
import { cssVar, useResolvedTheme } from "../lib/theme";
import type { ProjectSummary } from "../lib/types";
import type { SampleName } from "../lib/samples";

const ACCEPT = ".stl,.step,.stp,.obj,.3mf";
const EXAMPLES: { id: SampleName; label: string; dot: string }[] = [
  { id: "propeller", label: "A propeller", dot: "bg-plum" },
  { id: "frame", label: "A drone frame", dot: "bg-moss" },
  { id: "mount", label: "A motor mount", dot: "bg-sky" },
];

/** "quad_frame-v3.step" -> "Quad frame v3" */
export function nameFromFile(file: string): string {
  const base = file.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").replace(/\s+/g, " ").trim();
  return base ? base[0]!.toUpperCase() + base.slice(1) : "New part";
}

export default function Home() {
  const { api } = useApp();
  const [list, setList] = useState<ProjectSummary[] | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [over, setOver] = useState(false);
  const [asking, setAsking] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  const load = () => api.projects().then(setList, (e: Error) => setError(e.message));
  useEffect(() => {
    load();
    // eslint-style: load once per api
  }, [api]);
  useEvents((e) => {
    if (e.type === "project" || (e.type === "run" && e.run.status !== "running")) load();
  });

  const start = async (file: File | Blob, fileName: string, projectName: string) => {
    if (busy) return;
    if (!/\.(stl|step|stp|obj|3mf)$/i.test(fileName)) {
      setError("SimLab reads STL, STEP, OBJ and 3MF files. Export your part as one of those and drop it again.");
      return;
    }
    setError("");
    setBusy(`Uploading ${fileName}…`);
    try {
      const p = await api.createProject({ name: projectName });
      await api.uploadModel(p.id, file, { fileName });
      navigate(`/p/${p.id}?view=model`);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not start the project");
      setBusy("");
    }
  };
  const onFiles = (files: FileList | null) => {
    const f = files?.[0];
    if (f) start(f, f.name, nameFromFile(f.name));
  };
  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setOver(false);
    onFiles(e.dataTransfer.files);
  };
  const tryExample = async (id: SampleName) => {
    setBusy("Making the example part…");
    const { SAMPLES, toStl } = await import("../lib/samples");
    const s = SAMPLES[id];
    const mesh = s.make();
    setBusy("");
    await start(toStl(mesh, s.project), s.file, s.project);
  };

  const remove = async (p: ProjectSummary) => {
    setDeleting(p.id);
    try {
      await api.deleteProject(p.id);
      setList((l) => l?.filter((x) => x.id !== p.id) ?? null);
      setAsking(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not delete the project");
    } finally {
      setDeleting(null);
    }
  };

  return (
    <Screen>
      <TopBar />
      <main className="mx-auto flex w-full max-w-3xl flex-col gap-14 px-4 pb-20 pt-12 sm:pt-20">
        <section className="flex flex-col gap-5">
          <Badge tone="ember" className="self-start">
            <FlaskConicalIcon />
            New test
          </Badge>
          <h1 className="text-4xl font-semibold leading-[1.05] tracking-tight text-balance sm:text-5xl">What do you want to test?</h1>
          <p className="max-w-[58ch] text-base text-soft sm:text-lg">
            Drop in a 3D model of a drone part. SimLab checks if it will break, how air flows around it, or how hard a propeller pushes, all on this Mac.
          </p>
          <div
            role="button"
            tabIndex={0}
            aria-label="Choose a 3D model file: STL, STEP, OBJ or 3MF"
            aria-busy={!!busy}
            onClick={() => !busy && input.current?.click()}
            onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), !busy && input.current?.click())}
            onDragOver={(e) => {
              e.preventDefault();
              setOver(true);
            }}
            onDragLeave={() => setOver(false)}
            onDrop={onDrop}
            data-testid="drop-zone"
            className={cn(
              "flex min-h-48 flex-col items-center justify-center gap-3 rounded-xl border-2 border-dashed border-input bg-card px-6 py-10 text-center outline-none transition-colors hover:border-ring/60 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/30",
              over && "border-ember bg-ember/[0.06]",
            )}
          >
            {busy ? (
              <>
                <Loader />
                <span className="text-sm text-soft">{busy}</span>
              </>
            ) : (
              <>
                <span className="grid size-12 place-items-center rounded-full bg-ember/15 text-ember-text">
                  <UploadIcon className="size-5" />
                </span>
                <span className="text-base font-medium">{over ? "Let go to start" : "Drop a part here"}</span>
                <span className="text-sm text-muted-foreground">
                  STL, STEP, OBJ or 3MF · or <span className="text-ember-text underline underline-offset-4">click to choose a file</span>
                </span>
              </>
            )}
            <input ref={input} type="file" accept={ACCEPT} className="hidden" onChange={(e) => onFiles(e.target.files)} data-testid="file-input" />
          </div>
          <div className="flex flex-wrap items-center gap-2" aria-label="Examples">
            <span className="text-sm text-muted-foreground">No file handy? Try</span>
            {EXAMPLES.map((x) => (
              <button
                key={x.id}
                type="button"
                disabled={!!busy}
                onClick={() => tryExample(x.id)}
                className="inline-flex items-center gap-2 rounded-full border bg-card px-3 py-1.5 text-sm text-soft transition-colors outline-none hover:border-input hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50"
                data-testid={`example-${x.id}`}
              >
                <span className={cn("size-2 shrink-0 rounded-full", x.dot)} />
                {x.label}
              </button>
            ))}
          </div>
          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </section>

        <section className="flex flex-col gap-3">
          <div className="flex items-baseline gap-2">
            <h2 className="text-lg font-semibold">Your projects</h2>
            {list && list.length > 0 && <span className="num text-xs text-muted-foreground">{list.length}</span>}
          </div>
          {list === null ? (
            <Loader className="my-6" />
          ) : list.length === 0 ? (
            <p className="py-4 text-sm text-muted-foreground">Nothing yet. Your first part will show up here.</p>
          ) : (
            <ul className="overflow-hidden rounded-xl border bg-card" data-testid="project-list">
              {list.map((p) => {
                const pill = pillFor(p.last_run);
                return (
                  <li key={p.id} className="flex items-center gap-2 border-b pr-2 last:border-b-0 hover:bg-foreground/[0.03]" data-testid="project-row">
                    <a className="flex min-w-0 flex-1 items-center gap-3 py-3 pl-3 outline-none focus-visible:bg-accent sm:pl-4" href={href(`/p/${p.id}`)}>
                      <Thumb modelId={p.thumbnail_model_id ?? null} />
                      <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-4 gap-y-1">
                        <span className="flex min-w-0 flex-[1_1_12rem] flex-col">
                          <span className="truncate font-medium">{p.name}</span>
                          <span className="truncate text-xs text-muted-foreground">
                            {count(p.models, "part")} · {count(p.studies, "test")} · {count(p.runs, "run")}
                          </span>
                        </span>
                        <span className="flex items-center gap-3">
                          {pill ? <Badge tone={pill.tone}>{pill.text}</Badge> : <Badge>Not tested yet</Badge>}
                          <span className="num w-20 whitespace-nowrap text-right text-xs text-muted-foreground">{ago(p.updated_at)}</span>
                        </span>
                      </span>
                    </a>
                    {asking === p.id ? (
                      <span className="flex items-center gap-2" role="group" aria-label={`Delete ${p.name}?`}>
                        <span className="hidden text-sm sm:inline">Delete it and its results?</span>
                        <Button size="xs" variant="destructive" onClick={() => remove(p)} disabled={deleting === p.id} data-testid="delete-confirm">
                          {deleting === p.id ? "Deleting…" : "Delete"}
                        </Button>
                        <Button size="xs" variant="outline" onClick={() => setAsking(null)} disabled={deleting === p.id}>
                          Keep
                        </Button>
                      </span>
                    ) : (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button variant="ghost" size="icon-sm" onClick={() => setAsking(p.id)} aria-label={`Delete ${p.name}`} data-testid="delete-project">
                            <Trash2Icon />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>Delete</TooltipContent>
                      </Tooltip>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </section>
      </main>
    </Screen>
  );
}

const count = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** the part's picture, or a box icon while it draws (or when it is too big to draw here) */
function Thumb({ modelId }: { modelId: string | null }) {
  const { api } = useApp();
  const theme = useResolvedTheme();
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!modelId) return;
    let live = true;
    import("../viewer/thumb")
      .then(({ thumbnail }) => thumbnail(api, modelId, { part: cssVar("--part"), dark: theme === "dark" }))
      .then((u) => live && setUrl(u), () => live && setUrl(null));
    return () => {
      live = false;
    };
  }, [api, modelId, theme]);
  return (
    <span className="grid size-12 shrink-0 place-items-center overflow-hidden rounded-lg bg-[radial-gradient(ellipse_at_50%_40%,var(--viewer-a)_0%,var(--viewer-b)_90%)] text-muted-foreground">
      {url ? <img src={url} alt="" className="size-12 object-contain" /> : <BoxIcon className="size-5" />}
    </span>
  );
}
