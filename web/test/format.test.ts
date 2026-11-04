import { describe, expect, it } from "vitest";
import { ago, bytes, dirLabel, n, patchLabel, pillFor } from "../src/lib/format";
import { parse } from "../src/lib/router";
import { unitsQuestion } from "../src/workspace/model-panel";

const done = (headline: string) => ({ status: "done" as const, headline, progress: 1 });

describe("result pills", () => {
  it("turns headlines into short pills with the right colour", () => {
    expect(pillFor(done("Safe: 3.1× stronger than this load needs"))).toEqual({ tone: "moss", text: "Safe 3.1×" });
    expect(pillFor(done("Marginal: only 1.3× stronger than this load needs"))).toEqual({ tone: "gold", text: "Marginal 1.3×" });
    expect(pillFor(done("Breaks at about 28,500 rpm, at the blade root"))).toEqual({ tone: "clay", text: "Breaks at 28,500 rpm" });
    expect(pillFor(done("Drag 0.42 N at 15 m/s, about 43 g of push"))).toEqual({ tone: "sky", text: "Drag 0.42 N at 15 m/s" });
  });
  it("shows where a run is when it has no result yet", () => {
    expect(pillFor({ status: "running", headline: null, progress: 0.42 })?.text).toBe("Running 42%");
    expect(pillFor({ status: "queued", headline: null, progress: 0 })?.text).toBe("Waiting");
    expect(pillFor(null)).toBeNull();
  });
});

describe("numbers in words", () => {
  it("rounds sensibly", () => {
    expect(n(3.14159)).toBe("3.14");
    expect(n(28512)).toBe("28,512");
    expect(n(0.000123)).toBe("0.000123");
    expect(n(NaN)).toBe("–");
    expect(bytes(2.5 * 1024 ** 3)).toBe("2.5 GB");
    expect(ago(new Date(Date.now() - 3 * 3600_000).toISOString())).toBe("3 h ago");
    expect(dirLabel([0, 0, -5])).toBe("−Z");
  });
  it("names faces", () => {
    expect(patchLabel({ id: 1, kind: "cylinder", area_mm2: 30, center_mm: [0, 0, 0], radius_mm: 1.6 })).toBe("Hole, 3.2 mm across");
    expect(patchLabel({ id: 1, kind: "cylinder", area_mm2: 30, center_mm: [0, 0, 0], radius_mm: 6.5 }, false)).toBe("Round face, 13 mm across");
    expect(patchLabel({ id: 1, kind: "plane", area_mm2: 120, center_mm: [0, 0, 0], label: "Battery pad" })).toBe("Battery pad");
  });
});

describe("units check", () => {
  it("asks when a part is tiny or huge, and offers units that make sense", () => {
    expect(unitsQuestion({ size_mm: [120, 80, 5], unit_mm: 1 })).toBeNull();
    const q = unitsQuestion({ size_mm: [1.2, 1.2, 0.12], unit_mm: 1 })!;
    expect(q.across).toBe(1.2);
    expect(q.options.map((o) => o.word)).toEqual(["inches", "centimetres", "metres"]);
    expect(unitsQuestion({ size_mm: [76000, 100, 10], unit_mm: 1 })!.options.map((o) => o.word)).toEqual(["micrometres"]);
  });
});

describe("routes", () => {
  it("reads hash routes", () => {
    expect(parse("#/")).toEqual({ name: "home" });
    expect(parse("")).toEqual({ name: "home" });
    expect(parse("#/settings")).toEqual({ name: "settings" });
    const r = parse("#/p/prj_1a?view=tests&study=s1");
    expect(r.name).toBe("project");
    if (r.name === "project") {
      expect(r.id).toBe("prj_1a");
      expect(r.query.get("study")).toBe("s1");
    }
    expect(parse("#/nope").name).toBe("notfound");
  });
});
