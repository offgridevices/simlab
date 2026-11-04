// Compare: every finished run in the project in one sortable table; tick
// two to see them side by side.
import { useMemo, useState } from "react";
import { ArrowDownIcon, ArrowUpIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { cn } from "@/lib/utils";
import { useApp } from "../lib/app";
import { useAsync, useField, type ProjectData } from "../lib/hooks";
import { cssGradient, scaleFor } from "../lib/colormaps";
import { ago, G, KIND_LABEL, n, pillFor, STATUS_LABEL } from "../lib/format";
import type { Run, Study } from "../lib/types";
import { prepare } from "../viewer/prepare";
import { Viewer } from "../viewer/viewer";
import { Stat } from "../components/section";
import { KindIcon } from "./tests-panel";
import { VerdictCard } from "./results-strength";

interface RowData {
  run: Run;
  study: Study;
  key: number | null;
  keyLabel: string;
  mass: number | null;
  stress: number | null;
}

function rowOf(run: Run, study: Study): RowData {
  const s = run.summary;
  if (s?.kind === "strength") return { run, study, key: s.safety_factor, keyLabel: `${s.safety_factor.toFixed(1)}× safety`, mass: s.mass_g, stress: s.max_von_mises_mpa };
  if (s?.kind === "wind") return { run, study, key: s.thrust_n ?? s.drag_n, keyLabel: s.thrust_n ? `${n(s.thrust_n)} N thrust` : `${n(s.drag_n)} N drag`, mass: null, stress: null };
  if (s?.kind === "prop") {
    const best = Math.max(...s.points.map((p) => p.thrust_g));
    return { run, study, key: best, keyLabel: `${n(best)} g push`, mass: null, stress: null };
  }
  return { run, study, key: null, keyLabel: STATUS_LABEL[run.status], mass: null, stress: null };
}

type Col = "test" | "material" | "key" | "mass" | "stress" | "when";

export function CompareView({ data }: { data: ProjectData }) {
  const { api } = useApp();
  const briefs = data.studies.flatMap((s) => s.runs.filter((r) => r.status === "done").map((r) => ({ r, s })));
  const key = briefs.length ? `compare:${briefs.map((b) => b.r.id).join(",")}` : null;
  const runs = useAsync(key, () => Promise.all(briefs.map(async ({ r, s }) => rowOf(await api.run(r.id), s))));
  const [sort, setSort] = useState<{ col: Col; up: boolean }>({ col: "when", up: false });
  const [picked, setPicked] = useState<string[]>([]);

  const rows = useMemo(() => {
    const list = [...(runs.value ?? [])];
    const v = (x: RowData): string | number =>
      sort.col === "test" ? x.study.name : sort.col === "material" ? x.run.material?.name ?? "" : sort.col === "key" ? x.key ?? -Infinity : sort.col === "mass" ? x.mass ?? -Infinity : sort.col === "stress" ? x.stress ?? -Infinity : x.run.created_at;
    list.sort((a, b) => {
      const A = v(a), B = v(b);
      const c = typeof A === "number" && typeof B === "number" ? A - B : String(A).localeCompare(String(B));
      return sort.up ? c : -c;
    });
    return list;
  }, [runs.value, sort]);

  const head = (col: Col, label: string, right?: boolean, hide?: string) => (
    <TableHead className={cn(right && "text-right", hide)} aria-sort={sort.col === col ? (sort.up ? "ascending" : "descending") : "none"}>
      <button
        type="button"
        className={cn("inline-flex items-center gap-1 rounded outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50", sort.col === col && "text-foreground")}
        onClick={() => setSort((s) => ({ col, up: s.col === col ? !s.up : col === "test" || col === "material" }))}
      >
        {label}
        {sort.col === col && (sort.up ? <ArrowUpIcon className="size-3" /> : <ArrowDownIcon className="size-3" />)}
      </button>
    </TableHead>
  );
  const pick = (id: string) => setPicked((p) => (p.includes(id) ? p.filter((x) => x !== id) : [...p, id].slice(-2)));
  const pair = picked.map((id) => rows.find((r) => r.run.id === id)).filter((r): r is RowData => !!r);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto" data-testid="compare">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-4 py-6 sm:px-6">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div className="flex flex-col gap-1">
            <h2 className="text-2xl font-semibold tracking-tight">Compare</h2>
            <p className="text-soft">Every finished run in {data.project.name}. Tick two to see them side by side.</p>
          </div>
          {picked.length > 0 && (
            <Button variant="ghost" size="sm" onClick={() => setPicked([])}>
              Clear the ticks
            </Button>
          )}
        </div>
        {!briefs.length ? (
          <p className="rounded-xl bg-sunken p-4 text-sm text-muted-foreground">No finished runs yet. Run a test and it shows up here.</p>
        ) : runs.loading && !runs.value ? (
          <p className="text-sm text-muted-foreground">Loading the runs…</p>
        ) : (
          <div className="overflow-hidden rounded-xl border">
            <Table className="max-sm:[&_td]:px-2 max-sm:[&_th]:px-2">
              <TableHeader>
                <TableRow>
                  <TableHead className="w-10">
                    <span className="sr-only">Pick</span>
                  </TableHead>
                  {head("test", "Test")}
                  {head("material", "Material", false, "max-md:hidden")}
                  {head("key", "Main number", true)}
                  {head("mass", "Weight", true, "max-lg:hidden")}
                  {head("stress", "Biggest stress", true, "max-lg:hidden")}
                  <TableHead className="max-sm:hidden">Result</TableHead>
                  {head("when", "When", true, "max-sm:hidden")}
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((r) => {
                  const pill = pillFor(r.run);
                  const on = picked.includes(r.run.id);
                  return (
                    <TableRow key={r.run.id} data-state={on ? "selected" : undefined} className="cursor-pointer" onClick={() => pick(r.run.id)} data-testid="compare-row">
                      <TableCell>
                        <input
                          type="checkbox"
                          className="size-4 accent-[var(--ember)]"
                          checked={on}
                          onChange={() => pick(r.run.id)}
                          onClick={(e) => e.stopPropagation()}
                          aria-label={`Compare ${r.study.name}`}
                        />
                      </TableCell>
                      <TableCell>
                        <span className="flex min-w-0 items-center gap-2">
                          <KindIcon kind={r.study.kind} className="size-6 rounded-md [&_svg]:size-3.5" />
                          <span className="flex min-w-0 flex-col">
                            <span className="font-medium sm:truncate">{r.study.name}</span>
                            <span className="text-xs text-muted-foreground">
                              {KIND_LABEL[r.study.kind]}
                              {pill && <span className="sm:hidden"> · {pill.text}</span>}
                            </span>
                          </span>
                        </span>
                      </TableCell>
                      <TableCell className="whitespace-nowrap max-md:hidden">{r.run.material?.name ?? <span className="text-muted-foreground">–</span>}</TableCell>
                      <TableCell className="num text-right sm:whitespace-nowrap">{r.keyLabel}</TableCell>
                      <TableCell className="num whitespace-nowrap text-right max-lg:hidden">{r.mass !== null ? `${n(r.mass)} g` : "–"}</TableCell>
                      <TableCell className="num whitespace-nowrap text-right max-lg:hidden">{r.stress !== null ? `${n(r.stress)} MPa` : "–"}</TableCell>
                      <TableCell className="max-sm:hidden">{pill && <Badge tone={pill.tone}>{pill.text}</Badge>}</TableCell>
                      <TableCell className="num whitespace-nowrap text-right text-xs text-muted-foreground max-sm:hidden">{ago(r.run.created_at)}</TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          </div>
        )}
        {pair.length === 2 ? (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2" data-testid="side-by-side">
            {pair.map((r) => (
              <SideCard key={r.run.id} row={r} other={pair.find((x) => x !== r)!} />
            ))}
          </div>
        ) : (
          briefs.length > 1 && <p className="text-sm text-muted-foreground">{picked.length === 1 ? "Tick one more run to compare." : "Tick two runs to compare them side by side."}</p>
        )}
      </div>
    </div>
  );
}

function SideCard({ row, other }: { row: RowData; other: RowData }) {
  const s = row.run.summary;
  const o = other.run.summary;
  const better = (a: number | null | undefined, b: number | null | undefined, higher = true) => (a == null || b == null || a === b ? null : higher ? a > b : a < b);
  return (
    <div className="flex min-w-0 flex-col gap-3 rounded-xl border p-4">
      <div className="flex items-center gap-2">
        <KindIcon kind={row.study.kind} />
        <span className="flex min-w-0 flex-col">
          <span className="truncate font-semibold">{row.study.name}</span>
          <span className="text-xs text-muted-foreground">
            {row.run.material?.name ?? KIND_LABEL[row.study.kind]} · {ago(row.run.created_at)}
          </span>
        </span>
      </div>
      {row.run.status === "done" && s?.kind !== "prop" && <MiniView run={row.run} />}
      {s?.kind === "strength" && (
        <>
          <VerdictCard s={s} compact />
          <div className="grid grid-cols-2 gap-2">
            <Stat label="Safety factor" value={`${s.safety_factor.toFixed(1)}×`} hint={mark(better(s.safety_factor, o?.kind === "strength" ? o.safety_factor : null))} />
            <Stat label="Weight" value={n(s.mass_g)} unit="g" hint={mark(better(s.mass_g, o?.kind === "strength" ? o.mass_g : null, false), "lighter")} />
            <Stat label="Biggest stress" value={n(s.max_von_mises_mpa)} unit="MPa" />
            <Stat label="Biggest bend" value={n(s.max_displacement_mm)} unit="mm" hint={mark(better(s.max_displacement_mm, o?.kind === "strength" ? o.max_displacement_mm : null, false), "stiffer")} />
          </div>
        </>
      )}
      {s?.kind === "wind" && (
        <div className="grid grid-cols-2 gap-2">
          <Stat label="Drag" value={n(s.drag_n)} unit="N" hint={mark(better(s.drag_n, o?.kind === "wind" ? o.drag_n : null, false), "less drag")} />
          <Stat label="Lift" value={n(s.lift_n)} unit="N" />
          {s.thrust_n != null && <Stat label="Thrust" value={n(s.thrust_n)} unit="N" hint={`${n((s.thrust_n / G) * 1000)} g`} />}
          {s.cd != null && <Stat label="Drag coefficient" value={n(s.cd, 3)} />}
        </div>
      )}
      {s?.kind === "prop" && (
        <div className="grid grid-cols-2 gap-2">
          <Stat label="Most push" value={n(Math.max(...s.points.map((p) => p.thrust_g)))} unit="g" />
          <Stat label="Most power" value={n(Math.max(...s.points.map((p) => p.power_w)))} unit="W" />
        </div>
      )}
      <p className="text-sm text-soft">{s?.headline}</p>
    </div>
  );
}

const mark = (b: boolean | null, word = "stronger") => (b === null ? undefined : b ? `The ${word} of the two` : undefined);

/** a small view of a run's result surface, coloured by its main field */
function MiniView({ run }: { run: Run }) {
  const { api } = useApp();
  const mesh = useAsync(`runmesh:${run.id}`, () => api.runMesh(run.id));
  const fi = run.fields.find((f) => f.name === "safety_factor") ?? run.fields.find((f) => f.name === "pressure") ?? null;
  const field = useField(run.id, fi?.name ?? null);
  const prepared = useMemo(() => (mesh.value ? prepare(mesh.value) : null), [mesh.value]);
  const scale = fi ? scaleFor(fi) : null;
  const fieldProp = useMemo(() => (field.value && scale ? { values: field.value, scale } : null), [field.value, scale?.map, scale?.min, scale?.max]);
  return (
    <div className="relative flex h-56 overflow-hidden rounded-lg border">
      <Viewer prepared={prepared} status={mesh.error ? "error" : prepared ? "ready" : "loading"} message={mesh.error || "Loading the result"} field={fieldProp} compact critical={run.summary?.kind === "strength" ? run.summary.critical_point_mm : null} label={`3D view of ${run.id}`} />
      {scale && fi && (
        <div className="pointer-events-none absolute inset-x-2 bottom-2 rounded-md bg-card/90 px-2 py-1 text-[10px] text-muted-foreground backdrop-blur">
          <div className="mb-1 h-1.5 rounded-full" style={{ background: cssGradient(scale.map) }} />
          {fi.label}
        </div>
      )}
    </div>
  );
}
