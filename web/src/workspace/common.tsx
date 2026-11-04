// Pieces the right-hand panels share: the panel frame, numbered steps,
// number fields with units, a direction picker, the face chips.
import { useEffect, useId, useState, type ReactNode } from "react";
import { ArrowLeftIcon, ChevronDownIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { dirLabel, patchLabel } from "../lib/format";
import type { Patch, Vec3 } from "../lib/types";
import type { Prepared } from "../viewer/prepare";
import { useSceneCtl } from "./scene-state";

/** a panel: a header that stays, a body that scrolls, an optional footer that stays */
export function Panel({ title, back, actions, children, footer, testId }: { title: ReactNode; back?: () => void; actions?: ReactNode; children: ReactNode; footer?: ReactNode; testId?: string }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid={testId}>
      <div className="flex min-h-12 shrink-0 items-center gap-2 border-b px-3">
        {back && (
          <Button variant="ghost" size="icon-sm" onClick={back} aria-label="Back" data-testid="panel-back">
            <ArrowLeftIcon />
          </Button>
        )}
        <div className={cn("flex min-w-0 grow items-center gap-2 font-semibold", !back && "pl-1")}>{title}</div>
        {actions}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
        <div className="flex flex-col gap-5 p-4">{children}</div>
      </div>
      {footer && <div className="shrink-0 border-t bg-card p-3">{footer}</div>}
    </div>
  );
}

/** one numbered step of a setup: open shows its body, closed shows its summary */
export function Step({ n, title, summary, open, onOpen, done, children, testId }: { n: number; title: string; summary?: ReactNode; open: boolean; onOpen(): void; done?: boolean; children: ReactNode; testId?: string }) {
  const id = useId();
  return (
    <section className={cn("rounded-xl border transition-colors", open ? "border-input bg-card" : "bg-card")} data-testid={testId} data-open={open}>
      <h3>
        <button
          type="button"
          className="flex w-full items-start gap-3 rounded-xl px-3 py-3 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
          onClick={onOpen}
          aria-expanded={open}
          aria-controls={id}
        >
          <span
            className={cn(
              "num grid size-6 shrink-0 place-items-center rounded-full text-xs font-semibold",
              open ? "bg-primary text-primary-foreground" : done ? "bg-moss/20 text-moss-text" : "bg-foreground/[0.07] text-muted-foreground",
            )}
            aria-hidden="true"
          >
            {n}
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="text-sm font-semibold">{title}</span>
            {!open && summary && <span className="truncate text-xs text-muted-foreground">{summary}</span>}
          </span>
          <ChevronDownIcon className={cn("mt-0.5 size-4 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} aria-hidden="true" />
        </button>
      </h3>
      {open && (
        <div id={id} className="flex flex-col gap-3 px-3 pb-4">
          {children}
        </div>
      )}
    </section>
  );
}

/** a number field that keeps what you type while it is not yet a number */
export function NumberField({
  value,
  onChange,
  unit,
  label,
  min,
  max,
  step,
  className,
  testId,
  width = "w-24",
}: {
  value: number;
  onChange(v: number): void;
  unit?: string;
  label: string;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  testId?: string;
  width?: string;
}) {
  const [text, setText] = useState(fmtInput(value));
  useEffect(() => {
    if (Number(text) !== value) setText(fmtInput(value));
    // only when the value changes from outside
  }, [value]);
  const id = useId();
  return (
    <span className={cn("inline-flex items-center gap-2", className)}>
      <label htmlFor={id} className="sr-only">
        {label}
      </label>
      <Input
        id={id}
        type="number"
        inputMode="decimal"
        className={cn("num", width)}
        value={text}
        min={min}
        max={max}
        step={step ?? "any"}
        onChange={(e) => {
          setText(e.target.value);
          const v = Number(e.target.value);
          if (e.target.value.trim() !== "" && Number.isFinite(v)) onChange(clamp(v, min, max));
        }}
        onBlur={() => setText(fmtInput(value))}
        data-testid={testId}
      />
      {unit && <span className="text-sm text-muted-foreground">{unit}</span>}
    </span>
  );
}
const fmtInput = (v: number) => (Number.isFinite(v) ? String(Number(v.toPrecision(Math.abs(v) < 10 ? 4 : 6))) : "");
const clamp = (v: number, lo?: number, hi?: number) => Math.min(hi ?? Infinity, Math.max(lo ?? -Infinity, v));

export const AXES: { label: string; v: Vec3 }[] = [
  { label: "+X", v: [1, 0, 0] },
  { label: "−X", v: [-1, 0, 0] },
  { label: "+Y", v: [0, 1, 0] },
  { label: "−Y", v: [0, -1, 0] },
  { label: "+Z", v: [0, 0, 1] },
  { label: "−Z", v: [0, 0, -1] },
];

const same = (a: Vec3, b: Vec3) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) < 1e-6;
export const unit = (v: Vec3): Vec3 => {
  const l = Math.hypot(...v) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};

/** six axis buttons, and three numbers for any other direction */
export function AxisPicker({ value, onChange, label, testId }: { value: Vec3; onChange(v: Vec3): void; label: string; testId?: string }) {
  const custom = !AXES.some((a) => same(a.v, unit(value)));
  const [showCustom, setShowCustom] = useState(custom);
  return (
    <div className="flex flex-col gap-2" data-testid={testId}>
      <div className="grid grid-cols-6 gap-1" role="radiogroup" aria-label={label}>
        {AXES.map((a) => {
          const on = same(a.v, unit(value));
          return (
            <button
              key={a.label}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => {
                onChange(a.v);
                setShowCustom(false);
              }}
              className={cn(
                "num h-8 rounded-md border text-xs font-medium outline-none transition-colors hover:border-input focus-visible:ring-[3px] focus-visible:ring-ring/50",
                on ? "border-ember/60 bg-ember/10 text-ember-text" : "bg-sunken/60 text-soft",
              )}
            >
              {a.label}
            </button>
          );
        })}
      </div>
      {showCustom || custom ? (
        <div className="flex flex-wrap items-center gap-2">
          {(["x", "y", "z"] as const).map((k, i) => (
            <span key={k} className="inline-flex items-center gap-1 text-xs text-muted-foreground">
              {k.toUpperCase()}
              <NumberField label={`${label}, ${k}`} value={value[i]!} width="w-16" onChange={(v) => onChange(value.map((c, j) => (j === i ? v : c)) as Vec3)} />
            </span>
          ))}
        </div>
      ) : (
        <button type="button" className="self-start text-xs text-ember-text underline-offset-4 hover:underline" onClick={() => setShowCustom(true)}>
          Some other direction…
        </button>
      )}
    </div>
  );
}

/** the faces picked so far, as removable chips; hovering one lights it up in the 3D view */
export function FaceChips({ ids, patches, prepared, onRemove, empty }: { ids: number[]; patches: Patch[]; prepared: Prepared | null; onRemove(id: number): void; empty: ReactNode }) {
  const { setHover } = useSceneCtl();
  if (!ids.length) return <p className="rounded-lg bg-sunken px-3 py-2.5 text-sm text-muted-foreground">{empty}</p>;
  return (
    <ul className="flex flex-wrap gap-1.5" aria-label="Faces picked">
      {ids.map((id) => {
        const p = patches.find((x) => x.id === id);
        const label = p ? patchLabel(p, prepared?.holes.get(id)) : `Face ${id}`;
        return (
          <li key={id} onMouseEnter={() => setHover(id)} onMouseLeave={() => setHover(null)} className="inline-flex max-w-full items-center gap-1 rounded-full bg-foreground/[0.07] py-0.5 pl-2.5 pr-0.5 text-xs text-soft">
            <span className="truncate">{label}</span>
            <button type="button" className="grid size-5 shrink-0 place-items-center rounded-full outline-none hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring/60" onClick={() => onRemove(id)} aria-label={`Remove ${label}`}>
              <XIcon className="size-3" />
            </button>
          </li>
        );
      })}
    </ul>
  );
}

export { dirLabel };

/** toggle one id in a list */
export const toggle = (list: number[], id: number) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);

/** the middle of the faces, area-weighted */
export function facesCentre(ids: number[], patches: Patch[]): Vec3 | null {
  const ps = ids.map((i) => patches.find((p) => p.id === i)).filter((p): p is Patch => !!p);
  if (!ps.length) return null;
  const a = ps.reduce((s, p) => s + p.area_mm2, 0) || 1;
  return [0, 1, 2].map((k) => ps.reduce((s, p) => s + p.center_mm[k]! * p.area_mm2, 0) / a) as Vec3;
}

/** a segmented choice, for two to four short options */
export function Segmented<T extends string>({ value, options, onChange, label, testId }: { value: T; options: { id: NoInfer<T>; label: ReactNode }[]; onChange(v: NoInfer<T>): void; label: string; testId?: string }) {
  return (
    <div className="inline-flex h-9 w-fit max-w-full items-center gap-0.5 rounded-lg bg-sunken p-[3px]" role="radiogroup" aria-label={label} data-testid={testId}>
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          role="radio"
          aria-checked={value === o.id}
          onClick={() => onChange(o.id)}
          className={cn(
            "inline-flex h-full items-center justify-center gap-1.5 whitespace-nowrap rounded-md px-3 text-sm font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50",
            value === o.id && "bg-secondary text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
