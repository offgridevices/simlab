// Data hooks: a project kept current by live events, a run with its live
// metrics, and the binary buffers (cached, since a big part is many MB).
import { useCallback, useEffect, useRef, useState } from "react";
import { useApp, useEvents } from "./app";
import type { Api } from "./api";
import type { Model, Project, Run, RunBrief, Slice, Study, SurfaceBuffers } from "./types";

export interface ProjectData {
  project: Project;
  models: Model[];
  studies: Study[];
}

export function useProject(id: string) {
  const { api, link } = useApp();
  const [data, setData] = useState<ProjectData | null>(null);
  const [error, setError] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const current = useRef(id);
  current.current = id;
  const latest = useRef(data);
  latest.current = data;
  const load = useCallback(() => {
    api.project(id).then(
      (d) => {
        // a late answer for a project we have left is dropped
        if (current.current !== id) return;
        setData(d);
        setError("");
      },
      (e: Error) => current.current === id && setError(e.message),
    );
  }, [api, id]);
  // many events arrive close together while a run moves: reload at most a few times a second
  const soon = useCallback(() => {
    if (timer.current) return;
    timer.current = setTimeout(() => {
      timer.current = undefined;
      load();
    }, 300);
  }, [load]);
  useEffect(() => {
    setData(null);
    setError("");
    load();
    return () => {
      clearTimeout(timer.current);
      timer.current = undefined;
    };
  }, [load]);
  // events missed while the link was down: catch up once it is back
  const wasDown = useRef(false);
  useEffect(() => {
    if (link === "reconnecting") wasDown.current = true;
    else if (link === "open" && wasDown.current) {
      wasDown.current = false;
      load();
    }
  }, [link, load]);
  useEvents((e) => {
    if (e.type === "project" && e.project_id === id) soon();
    else if (e.type === "model" && e.model.project_id === id)
      setData((d) => (d ? { ...d, models: d.models.some((m) => m.id === e.model.id) ? d.models.map((m) => (m.id === e.model.id ? e.model : m)) : [...d.models, e.model] } : d));
    else if (e.type === "run" && e.run.project_id === id) {
      const r = e.run;
      const known = !!latest.current?.studies.some((s) => s.id === r.study_id || s.runs.some((x) => x.id === r.id));
      setData((d) => {
        if (!d) return d;
        // progress ticks may not say which test the run belongs to: find it by its id
        const studies = d.studies.map((s) => {
          const has = s.runs.some((x) => x.id === r.id);
          if (!has && s.id !== r.study_id) return s;
          const patch = { status: r.status, progress: r.progress, ...(r.headline !== undefined ? { headline: r.headline } : {}), ...(r.finished_at !== undefined ? { finished_at: r.finished_at } : {}) };
          const brief: RunBrief = { id: r.id, engine: r.engine ?? "native", created_at: r.created_at ?? new Date().toISOString(), ...patch };
          return { ...s, runs: has ? s.runs.map((x) => (x.id === r.id ? { ...x, ...patch } : x)) : [...s.runs, brief] };
        });
        return { ...d, studies };
      });
      // a run we have not seen (started elsewhere, by Claude say), or a finished one: fetch the whole project
      if (!known || (r.status !== "running" && r.status !== "queued")) soon();
    }
  });
  return { data, error, reload: load, setData };
}

/** one run, kept current: progress and message from run events, metric points as they arrive */
export function useRun(id: string | null | undefined) {
  const { api } = useApp();
  const [run, setRun] = useState<Run | null>(null);
  const [error, setError] = useState("");
  const status = useRef<string>("");
  const current = useRef(id);
  current.current = id;
  const load = useCallback(() => {
    if (!id) return;
    api.run(id).then(
      (r) => {
        if (current.current !== id) return;
        status.current = r.status;
        setRun(r);
        setError("");
      },
      (e: Error) => current.current === id && setError(e.message),
    );
  }, [api, id]);
  useEffect(() => {
    setRun(null);
    setError("");
    status.current = "";
    load();
  }, [load]);
  useEvents((e) => {
    if (!id) return;
    if (e.type === "run" && e.run.id === id) {
      // a new status (started, finished, failed) brings the full run
      if (e.run.status !== status.current) {
        status.current = e.run.status;
        load();
      } else setRun((r) => (r ? { ...r, progress: e.run.progress, message: e.run.message } : r));
    } else if (e.type === "metric" && e.run_id === id)
      setRun((r) => {
        if (!r) return r;
        const prev = r.metrics[e.name] ?? [];
        if (prev.length && prev[prev.length - 1]![0] >= e.step) return r;
        return { ...r, metrics: { ...r.metrics, [e.name]: [...prev, [e.step, e.value]] } };
      });
  });
  return { run, error, reload: load };
}

// ---------------------------------------------------------------- buffers

const cache = new Map<string, Promise<unknown>>();
const MAX = 24;
function cached<T>(key: string, f: () => Promise<T>): Promise<T> {
  let p = cache.get(key) as Promise<T> | undefined;
  if (!p) {
    p = f();
    cache.set(key, p);
    p.catch(() => cache.delete(key));
    // forget the oldest (Map keeps insertion order)
    while (cache.size > MAX) cache.delete(cache.keys().next().value!);
  }
  return p;
}

export function useAsync<T>(key: string | null, f: () => Promise<T>): { value: T | null; error: string; loading: boolean } {
  const [state, setState] = useState<{ key: string | null; value: T | null; error: string }>({ key: null, value: null, error: "" });
  const fn = useRef(f);
  fn.current = f;
  useEffect(() => {
    if (!key) return;
    let live = true;
    cached(key, () => fn.current()).then(
      (value) => live && setState({ key, value, error: "" }),
      (e: Error) => live && setState({ key, value: null, error: e.message }),
    );
    return () => {
      live = false;
    };
  }, [key]);
  const fresh = state.key === key;
  return { value: fresh ? state.value : null, error: fresh ? state.error : "", loading: !!key && !fresh };
}

export function surfaceKey(m: Pick<Model, "id" | "unit_mm" | "status">) {
  return m.status === "ready" ? `surface:${m.id}:${m.unit_mm}` : null;
}
export const loadSurface = (api: Api, id: string) => api.surface(id);

export function useSurface(model: Model | null | undefined) {
  const { api } = useApp();
  return useAsync<SurfaceBuffers>(model ? surfaceKey(model) : null, () => loadSurface(api, model!.id));
}

export function useRunMesh(run: Run | null | undefined) {
  const { api } = useApp();
  return useAsync<SurfaceBuffers>(run?.status === "done" && run.summary?.kind !== "prop" ? `runmesh:${run.id}` : null, () => api.runMesh(run!.id));
}

export function useField(runId: string | null | undefined, name: string | null | undefined) {
  const { api } = useApp();
  return useAsync<Float32Array>(runId && name ? `field:${runId}:${name}` : null, () => api.field(runId!, name!));
}

export function useSlice(runId: string | null | undefined, name: string | null, field: string | null) {
  const { api } = useApp();
  return useAsync<{ slice: Slice; values: Float32Array }>(runId && name && field ? `slice:${runId}:${name}:${field}` : null, async () => {
    const slice = await api.slice(runId!, name!);
    const values = await api.sliceField(runId!, name!, field!);
    return { slice, values };
  });
}
