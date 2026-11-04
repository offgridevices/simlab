import { describe, expect, it } from "vitest";
import { colorize, cssGradient, legendTicks, lut, normalize, sample, SAFETY_MAX, scaleFor } from "../src/lib/colormaps";

const hue = ([r, g, b]: number[]) => (r! > g! && r! > b! ? "red" : g! >= r! && g! > b! ? "green" : b! > r! && b! > g! ? "blue" : "other");
const lightness = ([r, g, b]: number[]) => 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;

describe("colour scales", () => {
  it("magnitude scales get steadily lighter (perceptually ordered)", () => {
    for (const name of ["viridis", "cividis"] as const) {
      let last = -1;
      for (let t = 0; t <= 1.0001; t += 0.125) {
        const l = lightness(sample(name, t));
        expect(l).toBeGreaterThan(last);
        last = l;
      }
    }
  });

  it("stays in range outside 0..1", () => {
    for (const name of ["viridis", "cividis", "coolwarm", "safety"] as const)
      for (const t of [-1, 0, 0.5, 1, 2]) for (const c of sample(name, t)) expect(c).toBeGreaterThanOrEqual(0), expect(c).toBeLessThanOrEqual(1);
  });

  it("safety factor: clear steps at 1 and 2", () => {
    const at = (sf: number) => sample("safety", sf / SAFETY_MAX).map((c) => Math.round(c * 255));
    // just below and just above each step are different colours; inside a band they barely change
    const jump = (a: number[], b: number[]) => Math.abs(a[0]! - b[0]!) + Math.abs(a[1]! - b[1]!) + Math.abs(a[2]! - b[2]!);
    expect(jump(at(0.99), at(1.01))).toBeGreaterThan(60);
    expect(jump(at(1.99), at(2.01))).toBeGreaterThan(60);
    expect(jump(at(1.3), at(1.6))).toBeLessThan(30);
    expect(hue(at(0.5))).toBe("red");
    expect(hue(at(3))).toBe("green");
  });

  it("diverging is grey at zero and cool/warm either side", () => {
    const s = scaleFor({ name: "pressure", min: -50, max: 200, scale: "linear", diverging: true });
    expect(s.map).toBe("coolwarm");
    expect([s.min, s.max]).toEqual([-200, 200]);
    expect(normalize(0, s)).toBeCloseTo(0.5);
    const [r, g, b] = sample("coolwarm", 0.5);
    expect(Math.max(r, g, b) - Math.min(r, g, b)).toBeLessThan(0.05);
    expect(hue(sample("coolwarm", 0))).toBe("blue");
    expect(hue(sample("coolwarm", 1))).toBe("red");
  });

  it("picks the scale by field", () => {
    expect(scaleFor({ name: "safety_factor", min: 0.4, max: 100, scale: "linear" }).map).toBe("safety");
    expect(scaleFor({ name: "displacement", min: 0, max: 3, scale: "linear" }).map).toBe("cividis");
    expect(scaleFor({ name: "von_mises", min: 0, max: 40, scale: "linear" }).map).toBe("viridis");
    expect(scaleFor({ name: "max_principal", min: -5, max: 40, scale: "linear", diverging: true }).map).toBe("coolwarm");
    const log = scaleFor({ name: "von_mises", min: 0.1, max: 1000, scale: "log" });
    expect(log.log).toBe(true);
    expect(normalize(10, log)).toBeCloseTo(0.5);
    expect(legendTicks(log)).toEqual([0.1, 1, 10, 100, 1000]);
    expect(legendTicks({ map: "viridis", min: 0.4, max: 31.9, log: false, diverging: false })).toEqual([10, 20, 30]);
  });

  it("builds a 256-texel lookup table and a CSS gradient", () => {
    const t = lut("viridis");
    expect(t.length).toBe(256 * 4);
    expect([t[0], t[1], t[2], t[3]]).toEqual([0x44, 0x01, 0x54, 255]);
    expect(cssGradient("safety", 4)).toMatch(/^linear-gradient\(to right, #/);
  });

  it("colours slices with NaN (inside the part) see-through", () => {
    const out = colorize(new Float32Array([0, NaN, 10]), { map: "viridis", min: 0, max: 10, log: false, diverging: false });
    expect(out[3]).toBeGreaterThan(0);
    expect(out[7]).toBe(0);
    expect(out[11]).toBeGreaterThan(0);
  });
});
