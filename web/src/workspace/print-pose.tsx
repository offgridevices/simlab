// How the part sits on the printer's bed. Shown turned onto a flat bed in
// the 3D view; tip it about the printer's X and Y (and turn it on the bed)
// in steps, lay a face flat by clicking it, or pick an axis. The test only
// needs which way is up (the layers stack that way); the turn on the bed is
// kept for the view while the setup is open.
import { useEffect, useRef, useState } from "react";
import { Quaternion, Vector3 } from "three";
import { RotateCcwIcon, RotateCwIcon, Undo2Icon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { n } from "../lib/format";
import type { Vec3 } from "../lib/types";
import type { Quat } from "../viewer/scene";
import { unit } from "./common";

const Z = new Vector3(0, 0, 1);
const STEPS = [15, 45, 90];

/** the smallest turn that puts the part's `up` at the printer's +Z */
export function poseFromUp(up: Vec3): Quat {
  const u = new Vector3(...unit(up));
  const q = new Quaternion().setFromUnitVectors(u, Z);
  return [q.x, q.y, q.z, q.w];
}
/** which way the part's own frame points up on the printer */
export function upFromPose(q: Quat): Vec3 {
  const v = Z.clone().applyQuaternion(new Quaternion(...q).invert());
  return [v.x, v.y, v.z].map((x) => Number((Math.abs(x) < 1e-9 ? 0 : x).toFixed(4))) as Vec3;
}
const close = (a: Vec3, b: Vec3) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) < 2e-3;

/**
 * The pose for a part whose up is `up`, kept between renders: turns made here
 * stay as they are (with their turn on the bed); an up set from elsewhere
 * (an axis button, a face clicked) starts again from the smallest turn.
 */
export function usePose(up: Vec3): [Quat, (q: Quat) => void] {
  const [pose, setPose] = useState<Quat>(() => poseFromUp(up));
  const mine = useRef(pose);
  useEffect(() => {
    if (!close(upFromPose(mine.current), unit(up))) {
      const q = poseFromUp(up);
      mine.current = q;
      setPose(q);
    }
  }, [up[0], up[1], up[2]]);
  return [
    pose,
    (q) => {
      mine.current = q;
      setPose(q);
    },
  ];
}

/** tip about the printer's X or Y, or turn about its Z (on the bed), by some degrees */
function turn(q: Quat, axis: "x" | "y" | "z", deg: number): Quat {
  const ax = new Vector3(axis === "x" ? 1 : 0, axis === "y" ? 1 : 0, axis === "z" ? 1 : 0);
  const r = new Quaternion().setFromAxisAngle(ax, (deg * Math.PI) / 180).multiply(new Quaternion(...q));
  r.normalize();
  return [r.x, r.y, r.z, r.w];
}

/** how far the part's own up is tipped from the printer's up, degrees */
export function tiltDeg(up: Vec3): number {
  const u = unit(up);
  return (Math.acos(Math.max(-1, Math.min(1, u[2]))) * 180) / Math.PI;
}

export function PoseControls({ pose, onPose }: { pose: Quat; onPose(q: Quat, up: Vec3): void }) {
  const [step, setStep] = useState(15);
  const apply = (axis: "x" | "y" | "z", sign: 1 | -1) => {
    const q = turn(pose, axis, sign * step);
    onPose(q, upFromPose(q));
  };
  const row = (axis: "x" | "y" | "z", label: string, detail: string) => (
    <div className="flex items-center gap-2">
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="text-sm">{label}</span>
        <span className="text-2xs text-muted-foreground">{detail}</span>
      </span>
      <Button size="icon-sm" variant="outline" onClick={() => apply(axis, -1)} aria-label={`${label}: ${step}° one way`} data-testid={`tip-${axis}-minus`}>
        <RotateCcwIcon />
      </Button>
      <Button size="icon-sm" variant="outline" onClick={() => apply(axis, 1)} aria-label={`${label}: ${step}° the other way`} data-testid={`tip-${axis}-plus`}>
        <RotateCwIcon />
      </Button>
    </div>
  );
  return (
    <div className="flex flex-col gap-2.5 rounded-lg border bg-sunken/40 p-3" data-testid="pose-controls">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-xs font-medium text-muted-foreground">Turn it on the bed</span>
        <div className="flex items-center gap-1" role="group" aria-label="How far each click turns it">
          {STEPS.map((s) => (
            <Button key={s} size="xs" variant={s === step ? "default" : "ghost"} onClick={() => setStep(s)} aria-pressed={s === step} className="num">
              {s}°
            </Button>
          ))}
        </div>
      </div>
      {row("x", "Tip forward / back", "about the printer's X (red)")}
      {row("y", "Tip left / right", "about the printer's Y (green)")}
      {row("z", "Spin on the bed", "about Z: looks different, same layers")}
      <div className="flex items-center justify-between gap-2">
        <span className="num text-xs text-muted-foreground">Tipped {n(tiltDeg(upFromPose(pose)), 3)}° from how it was drawn</span>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => {
            const q: Quat = [0, 0, 0, 1];
            onPose(q, [0, 0, 1]);
          }}
        >
          <Undo2Icon />
          As drawn
        </Button>
      </div>
    </div>
  );
}
