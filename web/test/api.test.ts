import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, checkSurface, createApi } from "../src/lib/api";
import { mockTransport } from "../src/mock/engine";
import { makeFrame, toStl } from "../src/lib/samples";
import { prepare } from "../src/viewer/prepare";
import type { EngineEvent, StrengthConfig } from "../src/lib/types";

describe("API client", () => {
  it("sends JSON, reads JSON, and turns engine errors into plain messages", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const api = createApi({
      base: "http://engine",
      subscribe: () => () => {},
      fetch: async (url, init) => {
        calls.push({ url, init });
        if (url.endsWith("/api/projects/missing")) return new Response(JSON.stringify({ error: "No such project" }), { status: 404, headers: { "content-type": "application/json" } });
        return new Response(JSON.stringify({ id: "p1", name: "Frame", notes: "", created_at: "", updated_at: "" }), { status: 201, headers: { "content-type": "application/json" } });
      },
    });
    const p = await api.createProject({ name: "Frame" });
    expect(p.id).toBe("p1");
    expect(calls[0]!.url).toBe("http://engine/api/projects");
    expect(calls[0]!.init?.method).toBe("POST");
    expect(JSON.parse(String(calls[0]!.init?.body))).toEqual({ name: "Frame" });
    await expect(api.project("missing")).rejects.toThrow("No such project");
    await expect(api.project("missing")).rejects.toBeInstanceOf(ApiError);
  });

  it("reads binary buffers as typed arrays and checks they agree", async () => {
    const pos = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]);
    const bufs: Record<string, ArrayBufferView> = {
      "positions.f32": pos,
      "normals.f32": new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1]),
      "indices.u32": new Uint32Array([0, 1, 2]),
      "patch.u32": new Uint32Array([7]),
    };
    const api = createApi({
      base: "",
      subscribe: () => () => {},
      fetch: async (url) => new Response(bufs[url.split("/").pop()!]!.buffer as ArrayBuffer),
    });
    const s = await api.surface("m1");
    expect(s.positions).toEqual(pos);
    expect(Array.from(s.patch!)).toEqual([7]);
    expect(() => checkSurface({ ...s, indices: new Uint32Array([0, 1, 9]) })).toThrow(/past the end/);
  });

  it("says plainly when the engine is not there", async () => {
    const api = createApi({ base: "", subscribe: () => () => {}, fetch: async () => Promise.reject(new TypeError("Failed to fetch")) });
    await expect(api.health()).rejects.toThrow(/not answering/);
  });
});

describe("demo engine, through the real client", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("takes an upload, finds its faces, runs a strength test to the end with live events", async () => {
    const api = createApi(mockTransport({ latency: 0 }));
    const events: EngineEvent[] = [];
    api.subscribe((e) => events.push(e));
    const p = await api.createProject({ name: "Test frame" });
    const m = await api.uploadModel(p.id, toStl(makeFrame()), { fileName: "quad-frame.stl" });
    expect(m.status).toBe("processing");
    await vi.advanceTimersByTimeAsync(1500);
    const ready = await api.model(m.id);
    expect(ready.status).toBe("ready");
    expect(ready.suggestions?.motor_mounts).toHaveLength(4);
    expect(events.some((e) => e.type === "model" && e.model.id === m.id)).toBe(true);

    const s = await api.surface(m.id);
    expect(s.patch!.length).toBe(s.indices.length / 3);
    const prep = prepare(s, ready.patches);
    // every hole on the frame faces its own axis
    const holes = ready.patches!.filter((x) => /hole/i.test(x.label ?? ""));
    expect(holes.length).toBeGreaterThan(10);
    for (const h of holes) expect(prep.holes.get(h.id)).toBe(true);

    const mounts = ready.suggestions!.motor_mounts!;
    const config: StrengthConfig = {
      kind: "strength",
      material_id: "petg",
      print_up: [0, 0, 1],
      support: { type: "free_flying" },
      loads: mounts.map((g) => ({ type: "motor", patches: g.patches, thrust_n: 14, torque_nm: 0.15, axis: [0, 0, 1], motor_mass_g: 33 })),
      quality: { type: "draft" },
    };
    const study = await api.createStudy(p.id, { name: "Punch-out", model_id: m.id, config });
    const est = await api.estimate(study.id);
    expect(est.fits).toBe(true);
    expect(est.memory_bytes).toBeGreaterThan(0);

    const run = await api.startRun(study.id);
    expect(run.status).toBe("queued");
    await vi.advanceTimersByTimeAsync(10_000);
    const done = await api.run(run.id);
    expect(done.status).toBe("done");
    expect(done.summary?.kind).toBe("strength");
    expect(done.fields.map((f) => f.name)).toContain("safety_factor");
    expect(events.some((e) => e.type === "metric" && e.run_id === run.id)).toBe(true);
    expect(events.filter((e) => e.type === "run" && e.run.id === run.id).at(-1)).toMatchObject({ run: { status: "done", project_id: p.id } });

    const mesh = await api.runMesh(run.id);
    const sf = await api.field(run.id, "safety_factor");
    expect(sf.length).toBe(mesh.positions.length / 3);

    const list = await api.projects();
    expect(list.find((x) => x.id === p.id)?.last_run?.headline).toBe(done.headline);
  });
});
