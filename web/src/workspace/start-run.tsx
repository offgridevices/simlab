// Starting a run, including the one thing that can stand in its way: an
// accurate air test (OpenFOAM) or a CalculiX check needs Docker. When the
// engine answers "needs Docker", this asks the user the system check's own
// consent question, runs the fix (start Colima, build the image), waits for
// it, and starts the run again.
import { useRef, useState, type ReactNode } from "react";
import { Confirm } from "../components/confirm";
import { ApiError } from "../lib/api";
import { useApp } from "../lib/app";
import type { Check, Engine, EngineEvent } from "../lib/types";

/** running a finished test again with another engine, from its results */
export interface Again {
  rerun(engine: Engine): void;
  starting: boolean;
  startError: string;
  /** while Docker is being set up for it */
  setting: string;
  dialog: ReactNode;
}

interface Pending {
  check: Check;
  studyId: string;
  engine?: Engine;
  done(runId: string): void;
  fail(message: string): void;
}

/** resolves with the check once its fix has finished (ok, or what is still missing) */
function fixed(listen: (f: (e: EngineEvent) => void) => () => void, id: string, timeoutMs = 15 * 60_000): Promise<Check> {
  return new Promise((resolve) => {
    let seenWorking = false;
    const stop = listen((e) => {
      if (e.type !== "check" || e.check.id !== id) return;
      if (e.check.status === "working") {
        seenWorking = true;
        return;
      }
      // the first answer after the fix began (or an error straight away)
      if (seenWorking || e.check.status === "error") {
        stop();
        clearTimeout(t);
        resolve(e.check);
      }
    });
    const t = setTimeout(() => {
      stop();
      resolve({ id, label: "", status: "error", detail: "Setting it up took too long. Open Settings → System check to see where it stands." });
    }, timeoutMs);
  });
}

export function useStartRun() {
  const { api, listen } = useApp();
  const [asking, setAskingState] = useState<Pending | null>(null);
  // the question being asked, read synchronously: the dialog's close event
  // follows a click on its action, and must not count as "Not now"
  const pending = useRef<Pending | null>(null);
  const setAsking = (p: Pending | null) => {
    pending.current = p;
    setAskingState(p);
  };
  const [setting, setSetting] = useState("");

  /** starts a run; `done` gets its id. Throws plain-words errors, except "needs Docker", which asks first. */
  const start = async (studyId: string, engine: Engine | undefined, done: (runId: string) => void, fail: (message: string) => void): Promise<void> => {
    try {
      const r = await api.startRun(studyId, engine);
      done(r.id);
    } catch (e) {
      if (e instanceof ApiError && e.needs) {
        if (e.needs.fix) {
          setAsking({ check: e.needs, studyId, engine, done, fail });
          return;
        }
        fail(`${e.message} ${e.needs.detail}`);
        return;
      }
      fail(e instanceof Error ? e.message : "Could not start the run");
    }
  };

  const agree = async () => {
    const p = pending.current;
    if (!p) return;
    setAsking(null);
    setSetting(`${p.check.fix?.label ?? "Setting up"}… the run starts by itself when it is ready.`);
    const result = fixed(listen, p.check.id);
    try {
      await api.fix(p.check.id);
    } catch (e) {
      setSetting("");
      p.fail(e instanceof Error ? e.message : "Could not start the fix");
      return;
    }
    const c = await result;
    setSetting("");
    if (c.status === "ok") await start(p.studyId, p.engine, p.done, p.fail);
    else if (c.fix) setAsking({ ...p, check: c });
    else p.fail(c.detail || "Docker is still not ready. Open Settings → System check.");
  };

  const dialog = (
    <Confirm
      open={!!asking}
      onOpenChange={(o) => {
        const p = pending.current;
        if (!o && p) {
          setAsking(null);
          p.fail("Not started: this test needs Docker. You can start it any time from Settings → System check.");
        }
      }}
      title={asking?.check.fix?.label ? `${asking.check.fix.label} first?` : "Set up Docker first?"}
      description={asking ? `This test runs in Docker, which is not ready yet. ${asking.check.fix?.asks ?? ""}` : ""}
      action={asking?.check.fix?.label ?? "Go ahead"}
      cancel="Not now"
      destructive={false}
      onConfirm={agree}
      testId="needs-confirm"
    />
  );
  return { start, dialog, setting };
}
