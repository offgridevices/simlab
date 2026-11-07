import { describe, expect, it } from "vitest";
import { Quaternion, Vector3 } from "three";
import { poseFromUp, tiltDeg, upFromPose } from "../src/workspace/print-pose";
import { rpmFrom } from "../src/workspace/rpm-helper";
import type { Vec3 } from "../src/lib/types";

const close = (a: Vec3, b: Vec3) => a.forEach((v, i) => expect(v).toBeCloseTo(b[i]!, 3));

describe("print pose", () => {
  it("turns the part's up to the printer's up, and reads it back", () => {
    for (const up of [[0, 0, 1], [1, 0, 0], [0, -1, 0], [0, 0, -1], [0.6, 0, 0.8]] as Vec3[]) {
      const q = poseFromUp(up);
      const v = new Vector3(...up).applyQuaternion(new Quaternion(...q));
      close([v.x, v.y, v.z], [0, 0, 1]);
      close(upFromPose(q), up);
    }
  });
  it("says how far a part is tipped from how it was drawn", () => {
    expect(tiltDeg([0, 0, 1])).toBeCloseTo(0);
    expect(tiltDeg([1, 0, 0])).toBeCloseTo(90);
    expect(tiltDeg([0, 0, -1])).toBeCloseTo(180);
  });
});

describe("rpm from motor and battery", () => {
  it("is KV × 4.2 V a cell × 0.7 under load, to the nearest 100", () => {
    expect(rpmFrom(1900, 6)).toBe(33500);
    expect(rpmFrom(2450, 4)).toBe(28800);
  });
});
