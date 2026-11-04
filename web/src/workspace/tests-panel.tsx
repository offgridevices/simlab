// The Tests panel: the list of tests, picking a new one, setting it up,
// watching it run and reading its results.
import { useMemo, useState } from "react";
import { FanIcon, PlusIcon, ShieldCheckIcon, Trash2Icon, WindIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Alert } from "@/components/ui/alert";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Confirm } from "../components/confirm";
import { cn } from "@/lib/utils";
import { useApp } from "../lib/app";
import { useRun, type ProjectData } from "../lib/hooks";
import { ago, KIND_LABEL, KIND_QUESTION, pillFor, STATUS_LABEL, STATUS_TONE } from "../lib/format";
import type { Model, Study, StudyConfig, StudyKind } from "../lib/types";
import type { Prepared } from "../viewer/prepare";
import type { Nav } from "../screens/workspace";
import { Meter } from "../components/charts";
import { Panel, Segmented } from "./common";
import { useScene } from "./scene-state";
import { defaults, type Preset } from "./presets";
import { StudyForm } from "./study-form";
import { RunningView } from "./running";
import { StrengthResults } from "./results-strength";
import { WindResults } from "./results-wind";
import { PropResults } from "./results-prop";

export const KIND_ICON: Record<StudyKind, React.ReactNode> = {
  strength: <ShieldCheckIcon />,
  wind: <WindIcon />,
  prop: <FanIcon />,
};
const KIND_BG: Record<StudyKind, string> = { strength: "bg-ember/15 text-ember-text", wind: "bg-sky/15 text-sky-text", prop: "bg-plum/15 text-plum-text" };

export function KindIcon({ kind, className }: { kind: StudyKind; className?: string }) {
  return <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg [&_svg]:size-4", KIND_BG[kind], className)}>{KIND_ICON[kind]}</span>;
}

export interface PanelProps {
  data: ProjectData;
  model: Model | undefined;
  nav: Nav;
  prepared: Prepared | null;
  reload(): void;
}

export function TestsPanel(props: PanelProps) {
  const { nav, data } = props;
  if (nav.create === "pick") return <KindChooser {...props} />;
  if (nav.create === "strength" || nav.create === "wind" || nav.create === "prop") return <NewStudy {...props} kind={nav.create} />;
  const study = nav.study ? data.studies.find((s) => s.id === nav.study) : undefined;
  if (study) return <StudyView {...props} study={study} />;
  return <StudyList {...props} />;
}

function StudyList({ data, model, nav }: PanelProps) {
  useScene({ source: { kind: "model", modelId: model?.id } });
  const studies = [...data.studies].sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  return (
    <Panel
      title="Tests"
      testId="tests-panel"
      actions={
        <Button size="sm" onClick={() => nav.go({ new: "pick" })} disabled={!model || model.status !== "ready"} data-testid="new-test">
          <PlusIcon />
          New test
        </Button>
      }
    >
      {studies.length === 0 ? (
        <Card className="items-start gap-3 bg-sunken p-4">
          <p className="text-sm text-soft">No tests yet. Pick what you want to find out: will it break, how air flows around it, or how hard a propeller pushes.</p>
          <Button onClick={() => nav.go({ new: "pick" })} disabled={!model || model.status !== "ready"}>
            <PlusIcon />
            Set up a test
          </Button>
        </Card>
      ) : (
        <ul className="flex flex-col gap-2" data-testid="study-list">
          {studies.map((s) => {
            const runs = [...s.runs].sort((a, b) => b.created_at.localeCompare(a.created_at));
            const last = runs[0];
            const pill = pillFor(last);
            const active = runs.find((r) => r.status === "running");
            const waiting = runs.filter((r) => r.status === "queued").length;
            return (
              <li key={s.id}>
                <Card
                  role="button"
                  tabIndex={0}
                  className="gap-2 px-3 py-3 outline-none transition-colors hover:border-input hover:bg-foreground/[0.02] focus-visible:ring-[3px] focus-visible:ring-ring/50"
                  onClick={() => nav.go({ study: s.id, run: null })}
                  onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), nav.go({ study: s.id, run: null }))}
                  data-testid="study-row"
                >
                  <span className="flex items-center gap-3">
                    <KindIcon kind={s.kind} />
                    <span className="flex min-w-0 flex-1 flex-col">
                      <span className="truncate font-medium">{s.name}</span>
                      <span className="truncate text-xs text-muted-foreground">
                        {KIND_LABEL[s.kind]} · {s.runs.length ? `${s.runs.length} ${s.runs.length === 1 ? "run" : "runs"}, ${ago(last?.created_at)}` : "not run yet"}
                      </span>
                    </span>
                    {pill ? <Badge tone={pill.tone}>{pill.text}</Badge> : <Badge>Set up</Badge>}
                  </span>
                  {active && <Meter value={active.progress} tone="ember" label={`${s.name}: ${Math.round(active.progress * 100)}% done`} />}
                  {waiting > 0 && <span className="text-xs text-muted-foreground">{waiting === 1 ? "1 run waiting for its turn" : `${waiting} runs waiting for their turn`}</span>}
                </Card>
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}

const KINDS: { kind: StudyKind; detail: string }[] = [
  { kind: "strength", detail: "Puts loads on the part (motors, crashes, spinning) in the filament or resin you print it in, and finds where it would break." },
  { kind: "wind", detail: "Blows air past the part in a virtual wind tunnel: drag, lift, and pictures of the flow." },
  { kind: "prop", detail: "Spins a propeller through a range of speeds: how many grams it pushes and how much power it takes." },
];

function KindChooser({ model, nav }: PanelProps) {
  useScene({ source: { kind: "model", modelId: model?.id } });
  const isProp = model?.suggestions?.looks_like === "propeller";
  return (
    <Panel title="New test" back={() => nav.go({ new: null })} testId="kind-chooser">
      <p className="text-sm text-soft">What do you want to find out about {model?.name ?? "this part"}?</p>
      <div className="flex flex-col gap-2" role="list">
        {KINDS.map(({ kind, detail }) => (
          <Card
            key={kind}
            role="listitem"
            className="relative flex-row items-start gap-3 p-4 transition-colors focus-within:ring-[3px] focus-within:ring-ring/50 hover:border-ember/50 hover:bg-ember/[0.04]"
            data-testid={`kind-${kind}`}
          >
            <KindIcon kind={kind} className="size-10 [&_svg]:size-5" />
            <span className="flex min-w-0 flex-col gap-1">
              <span className="flex flex-wrap items-center gap-2">
                <span className="font-semibold">{KIND_LABEL[kind]}</span>
                {kind === "prop" && isProp && <Badge tone="plum">Looks like a propeller</Badge>}
              </span>
              <span className="text-base font-medium text-soft">{KIND_QUESTION[kind]}</span>
              <span className="text-sm text-muted-foreground">{detail}</span>
            </span>
            <Button variant="ghost" className="absolute inset-0 h-auto rounded-xl hover:bg-transparent focus-visible:ring-0" onClick={() => nav.go({ new: kind })} aria-label={`${KIND_LABEL[kind]}: ${KIND_QUESTION[kind]}`} />
          </Card>
        ))}
      </div>
    </Panel>
  );
}

function NewStudy({ kind, data, model, nav, prepared, reload }: PanelProps & { kind: StudyKind }) {
  const { materials } = useApp();
  const preset = useMemo<Preset | null>(() => {
    try {
      return nav.preset ? (JSON.parse(nav.preset) as Preset) : null;
    } catch {
      return null;
    }
  }, [nav.preset]);
  const initial = useMemo<StudyConfig>(() => {
    const d = defaults(kind, model, materials?.find((m) => m.process === "fdm")?.id ?? "pla");
    return preset && preset.config.kind === kind ? ({ ...d, ...preset.config } as StudyConfig) : d;
  }, [kind, model, preset, materials]);
  const count = data.studies.filter((s) => s.kind === kind).length;
  const name = preset?.name ?? `${KIND_LABEL[kind]} test${count ? ` ${count + 1}` : ""}`;
  if (!model) return null;
  return (
    <StudyForm
      key={`${kind}:${nav.preset ?? ""}`}
      projectId={data.project.id}
      model={model}
      prepared={prepared}
      initial={initial}
      initialName={name}
      onBack={() => nav.go({ new: "pick", preset: null })}
      onStarted={(studyId, runId) => {
        reload();
        nav.go({ new: null, preset: null, study: studyId, run: runId });
      }}
    />
  );
}

function StudyView({ data, study, nav, prepared, reload }: PanelProps & { study: Study }) {
  const { api } = useApp();
  const model = data.models.find((m) => m.id === study.model_id);
  const runs = [...study.runs].sort((a, b) => b.created_at.localeCompare(a.created_at));
  const runId = nav.run && runs.some((r) => r.id === nav.run) ? nav.run : runs[0]?.id ?? null;
  const [tab, setTab] = useState<"results" | "setup">(runs.length ? "results" : "setup");
  const shown = runs.length ? tab : "setup";

  const remove = async () => {
    await api.deleteStudy(study.id);
    reload();
    nav.go({ study: null, run: null });
  };

  if (shown === "setup" && model)
    return (
      <StudyForm
        key={study.id}
        projectId={data.project.id}
        model={model}
        prepared={prepared}
        studyId={study.id}
        initial={study.config}
        initialName={study.name}
        onBack={() => (runs.length ? setTab("results") : nav.go({ study: null, run: null }))}
        onStarted={(_s, r) => {
          reload();
          setTab("results");
          nav.go({ run: r }, true);
        }}
        tabs={runs.length ? <ShowTabs value={tab} onChange={setTab} /> : null}
      />
    );

  return (
    <Panel
      title={
        <span className="flex min-w-0 items-center gap-2">
          <KindIcon kind={study.kind} className="size-6 rounded-md [&_svg]:size-3.5" />
          <span className="truncate">{study.name}</span>
        </span>
      }
      back={() => nav.go({ study: null, run: null })}
      testId="study-view"
      actions={
        <Confirm
          trigger={
            <Button variant="ghost" size="icon-sm" aria-label="Delete this test and its runs" data-testid="delete-study">
              <Trash2Icon />
            </Button>
          }
          title={`Delete “${study.name}”?`}
          description={`Its setup and ${runs.length === 1 ? "its run" : `all ${runs.length} runs`} go for good.`}
          action="Delete"
          onConfirm={remove}
          testId="delete-study-confirm"
        />
      }
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <ShowTabs value={shown} onChange={setTab} />
        {runs.length > 1 && (
          <Select value={runId ?? ""} onValueChange={(v) => nav.go({ run: v }, true)}>
            <SelectTrigger className="w-auto max-w-full" aria-label="Which run" data-testid="run-picker">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {runs.map((r, i) => (
                <SelectItem key={r.id} value={r.id}>
                  Run {runs.length - i} · {STATUS_LABEL[r.status]} · {ago(r.created_at)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>
      {runId && <RunView key={runId} runId={runId} study={study} model={model} nav={nav} />}
    </Panel>
  );
}

/** Results or Setup (shadcn Tabs as a switch; the content is the panel below) */
function ShowTabs({ value, onChange }: { value: "results" | "setup"; onChange(v: "results" | "setup"): void }) {
  return (
    <Tabs value={value} onValueChange={(v) => onChange(v as "results" | "setup")} data-testid="study-tabs">
      <TabsList>
        <TabsTrigger value="results">Results</TabsTrigger>
        <TabsTrigger value="setup">Setup</TabsTrigger>
      </TabsList>
    </Tabs>
  );
}

function RunView({ runId, study, model, nav }: { runId: string; study: Study; model: Model | undefined; nav: Nav }) {
  const { run, error } = useRun(runId);
  useScene(run?.status === "done" && run.summary?.kind !== "prop" ? {} : { source: { kind: "model", modelId: study.model_id } });
  if (error) return <p className="text-sm text-destructive">{error}</p>;
  if (!run) return <p className="text-sm text-muted-foreground">Loading the run…</p>;
  if (run.status === "queued" || run.status === "running") return <RunningView run={run} />;
  if (run.status === "failed" || run.status === "stopped")
    return (
      <div className="flex flex-col gap-3">
        <Alert tone={run.status === "failed" ? "clay" : "neutral"}>
          <span>
            <span className="font-semibold">{run.status === "failed" ? "This run did not finish." : "You stopped this run."}</span> {run.error ?? run.message}
          </span>
        </Alert>
        <Badge tone={STATUS_TONE[run.status]}>{STATUS_LABEL[run.status]}</Badge>
      </div>
    );
  const s = run.summary;
  if (!s) return <p className="text-sm text-muted-foreground">This run finished without a summary.</p>;
  if (s.kind === "strength") return <StrengthResults run={run} summary={s} size={model?.size_mm ? Math.max(...model.size_mm) : 100} />;
  if (s.kind === "wind") return <WindResults run={run} summary={s} />;
  return <PropResults run={run} summary={s} model={model} onStrength={(preset) => nav.go({ study: null, run: null, new: "strength", preset: JSON.stringify(preset) })} />;
}
