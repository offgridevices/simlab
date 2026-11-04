// Setting up a test: a name, a few numbered steps (by kind), and a footer
// with the engine's estimate (memory and time) and the Run button. The test
// is saved when it first needs an estimate or runs, and saved again on
// every run, so a run always matches the setup shown.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { PlayIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useApp } from "../lib/app";
import { bytes, duration, KIND_LABEL, n } from "../lib/format";
import type { Estimate, Model, StudyConfig } from "../lib/types";
import type { Prepared } from "../viewer/prepare";
import { Meter } from "../components/charts";
import { Panel } from "./common";
import { KindIcon } from "./tests-panel";
import { StrengthSteps, strengthProblems } from "./strength-form";
import { WindSteps, windProblems } from "./wind-form";
import { PropSteps, propProblems } from "./prop-form";

export interface StepsProps<C extends StudyConfig> {
  config: C;
  set(c: C): void;
  model: Model;
  prepared: Prepared | null;
  step: number;
  setStep(n: number): void;
  estimate: ReactNode;
}

export function problems(c: StudyConfig): string[] {
  return c.kind === "strength" ? strengthProblems(c) : c.kind === "wind" ? windProblems(c) : propProblems(c);
}

export function StudyForm({
  projectId,
  model,
  prepared,
  studyId,
  initial,
  initialName,
  onBack,
  onStarted,
  tabs,
}: {
  projectId: string;
  model: Model;
  prepared: Prepared | null;
  studyId?: string;
  initial: StudyConfig;
  initialName: string;
  onBack(): void;
  onStarted(studyId: string, runId: string): void;
  tabs?: ReactNode;
}) {
  const { api } = useApp();
  const [config, setConfig] = useState<StudyConfig>(initial);
  const [name, setName] = useState(initialName);
  const [step, setStep] = useState(studyId ? -1 : 1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [est, setEst] = useState<{ value: Estimate | null; loading: boolean; error: string }>({ value: null, loading: false, error: "" });
  const saved = useRef<string | undefined>(studyId);
  // a new test saved only to get an estimate, then left without running, is not kept
  const draft = useRef(false);
  const started = useRef(false);
  /** the first save, while it is on its way: everything after it waits for it, so only one test is ever created */
  const creating = useRef<Promise<string> | null>(null);
  useEffect(
    () => () => {
      const pending = creating.current;
      if (!pending || started.current) return;
      pending.then((id) => (draft.current && !started.current ? api.deleteStudy(id) : undefined)).catch(() => {});
    },
    [api],
  );
  const issues = problems(config);
  const ready = issues.length === 0;

  /** save the setup (create it the first time) and return its id */
  const save = async (): Promise<string> => {
    const body = { name: name.trim() || initialName, config };
    if (saved.current) {
      await api.updateStudy(saved.current, body);
      return saved.current;
    }
    if (creating.current) {
      const id = await creating.current;
      await api.updateStudy(id, body);
      return id;
    }
    creating.current = api.createStudy(projectId, { ...body, model_id: model.id }).then((s) => {
      saved.current = s.id;
      draft.current = true;
      return s.id;
    });
    creating.current.catch(() => (creating.current = null));
    return creating.current;
  };

  // the engine's estimate, once the setup is complete; refreshed as it changes
  const needsEstimate = config.kind !== "prop" && ready;
  const key = JSON.stringify(config);
  useEffect(() => {
    if (!needsEstimate) return;
    let live = true;
    setEst((e) => ({ ...e, loading: true, error: "" }));
    const t = setTimeout(async () => {
      try {
        const id = await save();
        const e = await api.estimate(id);
        if (live) setEst({ value: e, loading: false, error: "" });
      } catch (err) {
        if (live) setEst({ value: null, loading: false, error: err instanceof Error ? err.message : "No estimate" });
      }
    }, 600);
    return () => {
      live = false;
      clearTimeout(t);
    };
    // `key` stands for the config
  }, [key, needsEstimate]);

  const run = async () => {
    if (!ready || busy) return;
    setBusy(true);
    setError("");
    try {
      started.current = true;
      const id = await save();
      const r = await api.startRun(id);
      onStarted(id, r.id);
    } catch (e) {
      started.current = false;
      setError(e instanceof Error ? e.message : "Could not start the run");
      setBusy(false);
    }
  };

  const estimate = needsEstimate ? <EstimateLine est={est} /> : null;
  const props = { model, prepared, step, setStep, estimate };

  return (
    <Panel
      title={
        <span className="flex min-w-0 items-center gap-2">
          <KindIcon kind={config.kind} className="size-6 rounded-md [&_svg]:size-3.5" />
          <span className="truncate">{studyId ? "Setup" : `New ${KIND_LABEL[config.kind].toLowerCase()} test`}</span>
        </span>
      }
      back={onBack}
      testId={`form-${config.kind}`}
      footer={
        <div className="flex flex-col gap-2">
          {!ready && <p className="text-xs text-muted-foreground" data-testid="form-problems">{issues[0]}</p>}
          {error && (
            <p className="text-xs text-destructive" role="alert">
              {error}
            </p>
          )}
          {est.value && !est.value.fits && needsEstimate && <p className="text-xs text-gold-text">{est.value.note || "This may not fit in your memory budget."}</p>}
          <Button onClick={run} disabled={!ready || busy} size="lg" className="w-full" data-testid="run">
            <PlayIcon />
            {busy ? "Starting…" : studyId ? "Run again" : "Run the test"}
          </Button>
        </div>
      }
    >
      {tabs}
      <label className="flex flex-col gap-1.5 text-sm font-medium">
        Name
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={initialName} data-testid="study-name" />
      </label>
      {config.kind === "strength" ? (
        <StrengthSteps {...props} config={config} set={setConfig} />
      ) : config.kind === "wind" ? (
        <WindSteps {...props} config={config} set={setConfig} />
      ) : (
        <PropSteps {...props} config={config} set={setConfig} />
      )}
    </Panel>
  );
}

function EstimateLine({ est }: { est: { value: Estimate | null; loading: boolean; error: string } }) {
  if (est.error) return <p className="text-xs text-muted-foreground">{est.error}</p>;
  const e = est.value;
  if (!e) return <p className="text-xs text-muted-foreground">{est.loading ? "Working out how long it takes…" : ""}</p>;
  const share = e.budget_bytes ? e.memory_bytes / e.budget_bytes : 0;
  const size = e.size?.elements ? `${n(e.size.elements)} pieces` : e.size?.cells ? `${n(e.size.cells)} air cells` : "";
  return (
    <div className="flex flex-col gap-1.5 rounded-lg bg-sunken px-3 py-2.5" data-testid="estimate" aria-live="polite">
      <div className="flex flex-wrap justify-between gap-x-3 text-xs">
        <span>
          About <span className="num font-medium">{duration(e.seconds)}</span>
          {size && <span className="text-muted-foreground"> · {size}</span>}
        </span>
        <span className="num text-muted-foreground">
          {bytes(e.memory_bytes)} of {bytes(e.budget_bytes)}
        </span>
      </div>
      <Meter value={share} tone={e.fits ? (share > 0.75 ? "gold" : "moss") : "clay"} label="Memory it needs, of your budget" />
      {est.loading && <span className="text-xs text-muted-foreground">Updating…</span>}
    </div>
  );
}
