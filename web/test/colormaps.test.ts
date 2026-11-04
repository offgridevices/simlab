import { describe, expect, it } from "vitest";
import { colorize, cssGradient, legendTicks, lut, normalize, sample, SAFETY_MAX, scaleFor } from "../src/lib/colormaps";

const hue = ([r, g, b]: number[]) => (r! > g! && r! > b! ? "red" : g! >= r! && g! > b! ? "green" : b! > r! && b! > g! ? "blue" : "other");

describe("colour scales", () => {
  it("turbo runs from dark blue through green to dark red", () => {
    expect(hue(sample("turbo", 0.05))).toBe("blue");
    expect(hue(sample("turbo", 0.55))).toBe("green");
    expect(hue(sample("turbo", 0.97))).toBe("red");
  });

  it("stays in range outside 0..1", () => {
    for (const name of ["turbo", "viridis", "diverging", "safety"] as const)
      for (const t of [-1, 0, 0.5, 1, 2]) for (const c of sample(name, t)) expect(c).toBeGreaterThanOrEqual(0), expect(c).toBeLessThanOrEqual(1);
  });

  it("safety factor: clay below 1, gold between 1 and 2, moss above 2", () => {
    const at = (sf: number) => sample("safety", sf / SAFETY_MAX).map((c) => Math.round(c * 255));
    expect(at(0.5)[0]).toBeGreaterThan(180); // clay is red-orange
    expect(at(0.5)[1]).toBeLessThan(at(1.5)[1]!); // gold is more yellow
    expect(at(1.5)).toEqual([240, 184, 74]); // gold itself
    expect(hue(at(3))).toBe("green");
  });

  it("diverging is grey at zero and cool/warm either side", () => {
    const s = scaleFor({ name: "pressure", min: -50, max: 200, scale: "linear", diverging: true });
    expect(s.min).toBe(-200);
    expect(s.max).toBe(200);
    expect(normalize(0, s)).toBeCloseTo(0.5);
    const [r, g, b] = sample("diverging", 0.5);
    expect(Math.max(r, g, b) - Math.min(r, g, b)).toBeLessThan(0.05);
    expect(hue(sample("diverging", 0))).toBe("blue");
    expect(hue(sample("diverging", 1))).toBe("red");
  });

  it("picks the scale by field", () => {
    expect(scaleFor({ name: "safety_factor", min: 0.4, max: 100, scale: "linear" }).map).toBe("safety");
    expect(scaleFor({ name: "displacement", min: 0, max: 3, scale: "linear" }).map).toBe("viridis");
    expect(scaleFor({ name: "von_mises", min: 0, max: 40, scale: "linear" }).map).toBe("turbo");
    const log = scaleFor({ name: "von_mises", min: 0.1, max: 1000, scale: "log" });
    expect(log.log).toBe(true);
    expect(normalize(10, log)).toBeCloseTo(0.5);
    expect(legendTicks(log)).toEqual([0.1, 1, 10, 100, 1000]);
  });

  it("builds a 256-texel lookup table and a CSS gradient", () => {
    const t = lut("viridis");
    expect(t.length).toBe(256 * 4);
    expect([t[0], t[1], t[2], t[3]]).toEqual([0x44, 0x01, 0x54, 255]);
    expect(cssGradient("turbo", 4)).toMatch(/^linear-gradient\(to right, rgb\(/);
  });

  it("colours slices with NaN (inside the part) see-through", () => {
    const out = colorize(new Float32Array([0, NaN, 10]), { map: "turbo", min: 0, max: 10, log: false, diverging: false });
    expect(out[3]).toBeGreaterThan(0);
    expect(out[7]).toBe(0);
    expect(out[11]).toBeGreaterThan(0);
  });
});
