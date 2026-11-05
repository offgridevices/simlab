// A typed client for the engine's HTTP API (docs/API.md). Everything goes
// through one `fetch` and one event source, so demo mode swaps in a fake
// engine (src/mock) behind the very same client code.
import type {
  Check,
  EngineEvent,
  Estimate,
  Health,
  Material,
  Model,
  Motor,
  Project,
  ProjectSummary,
  Run,
  Settings,
  Slice,
  Study,
  StudyConfig,
  SurfaceBuffers,
  UpdateResult,
} from "./types";

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export type Fetch = (input: string, init?: RequestInit) => Promise<Response>;
/** live events: calls `on` for each, `state` when the link opens or drops; returns a stop function */
export type Subscribe = (on: (e: EngineEvent) => void, state?: (s: LinkState) => void) => () => void;
export type LinkState = "connecting" | "open" | "reconnecting";

export interface Transport {
  base: string;
  fetch: Fetch;
  subscribe: Subscribe;
}

/** the engine's live events over Server-Sent Events, reconnecting on its own */
export function sseSubscribe(url: string): Subscribe {
  return (on, state) => {
    let es: EventSource | null = null;
    let closed = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    const open = () => {
      state?.("connecting");
      es = new EventSource(url);
      es.onopen = () => state?.("open");
      es.onmessage = (m) => {
        for (const line of String(m.data).split("\n")) {
          if (!line.trim()) continue;
          try {
            on(JSON.parse(line) as EngineEvent);
          } catch {
            /* not JSON: ignore */
          }
        }
      };
      es.onerror = () => {
        state?.("reconnecting");
        // EventSource retries by itself unless the server closed it for good
        if (es?.readyState === EventSource.CLOSED && !closed) {
          es.close();
          retry = setTimeout(open, 2000);
        }
      };
    };
    open();
    return () => {
      closed = true;
      clearTimeout(retry);
      es?.close();
    };
  };
}

const enc = encodeURIComponent;

export function createApi(t: Transport) {
  const url = (p: string) => `${t.base}${p}`;

  async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
    const init: RequestInit = { method, headers: {} };
    if (body instanceof FormData) init.body = body;
    else if (body !== undefined) {
      init.body = JSON.stringify(body);
      (init.headers as Record<string, string>)["content-type"] = "application/json";
    }
    let res: Response;
    try {
      res = await t.fetch(url(path), init);
    } catch {
      throw new ApiError("SimLab's engine is not answering. Is the app still open?", 0);
    }
    if (!res.ok) throw new ApiError(await errorText(res), res.status);
    if (res.status === 204) return undefined as T;
    const type = res.headers.get("content-type") ?? "";
    if (type.includes("application/json")) return (await res.json()) as T;
    const text = await res.text();
    return (text ? JSON.parse(text) : undefined) as T;
  }

  async function bin(path: string): Promise<ArrayBuffer> {
    let res: Response;
    try {
      res = await t.fetch(url(path));
    } catch {
      throw new ApiError("SimLab's engine is not answering. Is the app still open?", 0);
    }
    if (!res.ok) throw new ApiError(await errorText(res), res.status);
    return res.arrayBuffer();
  }

  async function text(path: string): Promise<string> {
    const res = await t.fetch(url(path));
    if (!res.ok) throw new ApiError(await errorText(res), res.status);
    return res.text();
  }

  return {
    subscribe: t.subscribe,

    // system
    health: () => req<Health>("GET", "/api/health"),
    fix: (id: string) => req<unknown>("POST", "/api/system/fix", { id }),
    update: () => req<UpdateResult>("POST", "/api/system/update"),
    settings: () => req<Settings>("GET", "/api/settings"),
    saveSettings: (s: Settings) => req<Settings>("PUT", "/api/settings", s),

    // library
    materials: () => req<Material[]>("GET", "/api/materials"),
    saveMaterial: (m: Material) => req<Material>("POST", "/api/materials", m),
    deleteMaterial: (id: string) => req<void>("DELETE", `/api/materials/${enc(id)}`),
    motors: () => req<Motor[]>("GET", "/api/motors"),
    saveMotor: (m: Motor) => req<Motor>("POST", "/api/motors", m),
    deleteMotor: (id: string) => req<void>("DELETE", `/api/motors/${enc(id)}`),

    // projects and models
    projects: () => req<ProjectSummary[]>("GET", "/api/projects"),
    createProject: (p: { name: string; notes?: string }) => req<Project>("POST", "/api/projects", p),
    project: (id: string) => req<{ project: Project; models: Model[]; studies: Study[] }>("GET", `/api/projects/${enc(id)}`),
    updateProject: (id: string, p: { name?: string; notes?: string }) => req<Project>("PATCH", `/api/projects/${enc(id)}`, p),
    deleteProject: (id: string) => req<void>("DELETE", `/api/projects/${enc(id)}`),
    uploadModel: (projectId: string, file: File | Blob, opts: { name?: string; unit_mm?: number; fileName?: string } = {}) => {
      const fd = new FormData();
      fd.append("file", file, opts.fileName ?? (file instanceof File ? file.name : "model.stl"));
      if (opts.name) fd.append("name", opts.name);
      if (opts.unit_mm) fd.append("unit_mm", String(opts.unit_mm));
      return req<Model>("POST", `/api/projects/${enc(projectId)}/models`, fd);
    },
    model: (id: string) => req<Model>("GET", `/api/models/${enc(id)}`),
    updateModel: (id: string, p: { name?: string; unit_mm?: number; patch_labels?: Record<string, string> }) => req<Model>("PATCH", `/api/models/${enc(id)}`, p),
    deleteModel: (id: string) => req<void>("DELETE", `/api/models/${enc(id)}`),
    async surface(id: string): Promise<SurfaceBuffers> {
      const b = (n: string) => bin(`/api/models/${enc(id)}/surface/${n}`);
      const [p, n, i, patch] = await Promise.all([b("positions.f32"), b("normals.f32"), b("indices.u32"), b("patch.u32")]);
      return checkSurface({ positions: new Float32Array(p), normals: new Float32Array(n), indices: new Uint32Array(i), patch: new Uint32Array(patch) });
    },
    modelFileUrl: (id: string) => url(`/api/models/${enc(id)}/file`),

    // studies and runs
    createStudy: (projectId: string, s: { name: string; model_id: string; config: StudyConfig }) => req<Study>("POST", `/api/projects/${enc(projectId)}/studies`, s),
    study: (id: string) => req<Study>("GET", `/api/studies/${enc(id)}`),
    updateStudy: (id: string, p: { name?: string; config?: StudyConfig }) => req<Study>("PATCH", `/api/studies/${enc(id)}`, p),
    deleteStudy: (id: string) => req<void>("DELETE", `/api/studies/${enc(id)}`),
    /** tolerant of the size numbers arriving at the top level (the wind tunnel's estimate) */
    async estimate(id: string): Promise<Estimate> {
      const r = await req<Estimate & { cells?: number; elements?: number; nodes?: number }>("POST", `/api/studies/${enc(id)}/estimate`);
      return { ...r, size: r.size ?? { cells: r.cells, elements: r.elements, nodes: r.nodes } };
    },
    startRun: (studyId: string, engine: "native" | "reference" = "native") => req<Run>("POST", `/api/studies/${enc(studyId)}/runs`, { engine }),
    run: (id: string) => req<Run>("GET", `/api/runs/${enc(id)}`),
    stopRun: (id: string) => req<unknown>("POST", `/api/runs/${enc(id)}/stop`),
    deleteRun: (id: string) => req<void>("DELETE", `/api/runs/${enc(id)}`),
    async runMesh(id: string): Promise<SurfaceBuffers> {
      const b = (n: string) => bin(`/api/runs/${enc(id)}/mesh/${n}`);
      const [p, n, i] = await Promise.all([b("positions.f32"), b("normals.f32"), b("indices.u32")]);
      return checkSurface({ positions: new Float32Array(p), normals: new Float32Array(n), indices: new Uint32Array(i) });
    },
    field: async (runId: string, name: string) => new Float32Array(await bin(`/api/runs/${enc(runId)}/field/${enc(name)}`)),
    slice: (runId: string, name: string) => req<Slice>("GET", `/api/runs/${enc(runId)}/slice/${enc(name)}`),
    sliceField: async (runId: string, name: string, field: string) => new Float32Array(await bin(`/api/runs/${enc(runId)}/slice/${enc(name)}/${enc(field)}`)),
    log: (runId: string) => text(`/api/runs/${enc(runId)}/log`),
  };
}
export type Api = ReturnType<typeof createApi>;

async function errorText(res: Response): Promise<string> {
  try {
    const t = await res.text();
    try {
      const j = JSON.parse(t) as { error?: unknown };
      if (typeof j.error === "string" && j.error) return j.error;
    } catch {
      /* not JSON */
    }
    if (t && t.length < 300) return t;
  } catch {
    /* no body */
  }
  return `The engine said ${res.status}${res.statusText ? ` (${res.statusText})` : ""}`;
}

/** the buffers agree with each other, or a plain error */
export function checkSurface(s: SurfaceBuffers): SurfaceBuffers {
  const verts = s.positions.length / 3;
  if (s.positions.length % 3 || s.normals.length !== s.positions.length) throw new ApiError("The part's surface came back damaged (positions and normals differ).", 0);
  if (s.indices.length % 3) throw new ApiError("The part's surface came back damaged (triangles are incomplete).", 0);
  if (s.patch && s.patch.length !== s.indices.length / 3) throw new ApiError("The part's surface came back damaged (one face group per triangle expected).", 0);
  let max = 0;
  for (let i = 0; i < s.indices.length; i++) if (s.indices[i]! > max) max = s.indices[i]!;
  if (s.indices.length && max >= verts) throw new ApiError("The part's surface came back damaged (a triangle points past the end).", 0);
  return s;
}
