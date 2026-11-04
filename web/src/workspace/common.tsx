// Pieces the right-hand panels share, all built from shadcn components: the
// panel frame (ScrollArea), numbered steps (Accordion items), number fields
// with units (Input), a direction picker and segmented choices (ToggleGroup),
// the picked faces (Badge), and the "How sure" line (Collapsible).
import { useEffect, useId, useState, type ReactNode } from "react";
import { ArrowLeftIcon, ShieldQuestionIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { AccordionContent, AccordionItem, AccordionTrigger } from "@/components/ui/accordion";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { dirLabel, n, patchLabel } from "../lib/format";
import type { Accuracy, Patch, Vec3 } from "../lib/types";
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
      <ScrollArea className="min-h-0 flex-1 [&>[data-slot=scroll-area-viewport]>div]:!block">
        <div className="flex min-w-0 flex-col gap-5 p-4">{children}</div>
      </ScrollArea>
      {footer && <div className="shrink-0 border-t bg-card p-3">{footer}</div>}
    </div>
  );
}

/** one numbered step of a setup, inside an <Accordion>: open shows its body, closed shows its summary */
export function Step({ n: num, title, summary, done, children, testId }: { n: number; title: string; summary?: ReactNode; done?: boolean; children: ReactNode; testId?: string }) {
  return (
    <AccordionItem value={String(num)} className="group/step rounded-xl border bg-card data-[state=open]:border-input" data-testid={testId}>
      <AccordionTrigger className="items-start gap-3 px-3 py-3 hover:no-underline">
        <span
          className={cn(
            "num grid size-6 shrink-0 place-items-center rounded-full bg-foreground/[0.07] text-xs font-semibold text-muted-foreground",
            done && "bg-moss/20 text-moss-text",
            "group-data-[state=open]/step:bg-primary group-data-[state=open]/step:text-primary-foreground",
          )}
          aria-hidden="true"
        >
          {num}
        </span>
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-sm font-semibold">{title}</span>
          {summary && <span className="truncate text-xs font-normal text-muted-foreground group-data-[state=open]/step:hidden">{summary}</span>}
        </span>
      </AccordionTrigger>
      <AccordionContent className="flex flex-col gap-3 px-3 pb-4">{children}</AccordionContent>
    </AccordionItem>
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
  const current = AXES.find((a) => same(a.v, unit(value)));
  const [showCustom, setShowCustom] = useState(!current);
  return (
    <div className="flex flex-col gap-2" data-testid={testId}>
      <ToggleGroup
        type="single"
        className="grid w-full grid-cols-6"
        value={current?.label ?? ""}
        onValueChange={(v) => {
          const a = AXES.find((x) => x.label === v);
          if (a) {
            onChange(a.v);
            setShowCustom(false);
          }
        }}
        aria-label={label}
      >
        {AXES.map((a) => (
          <ToggleGroupItem key={a.label} value={a.label} className="num px-0 text-xs">
            {a.label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {showCustom || !current ? (
        <div className="flex flex-wrap items-center gap-2">
          {(["x", "y", "z"] as const).map((k, i) => (
            <span key={k} className="inline-flex items-center gap-1 text-xs text-muted-foreground">
              {k.toUpperCase()}
              <NumberField label={`${label}, ${k}`} value={value[i]!} width="w-16" onChange={(v) => onChange(value.map((c, j) => (j === i ? v : c)) as Vec3)} />
            </span>
          ))}
        </div>
      ) : (
        <Button variant="link" size="xs" className="self-start px-0" onClick={() => setShowCustom(true)}>
          Some other direction…
        </Button>
      )}
    </div>
  );
}

/** the faces picked so far, as removable badges; hovering one lights it up in the 3D view */
export function FaceChips({ ids, patches, prepared, onRemove, empty }: { ids: number[]; patches: Patch[]; prepared: Prepared | null; onRemove(id: number): void; empty: ReactNode }) {
  const { setHover } = useSceneCtl();
  if (!ids.length) return <p className="rounded-lg bg-sunken px-3 py-2.5 text-sm text-muted-foreground">{empty}</p>;
  return (
    <ul className="flex flex-wrap gap-1.5" aria-label="Faces picked">
      {ids.map((id) => {
        const p = patches.find((x) => x.id === id);
        const label = p ? patchLabel(p, prepared?.holes.get(id)) : `Face ${id}`;
        return (
          <li key={id} onMouseEnter={() => setHover(id)} onMouseLeave={() => setHover(null)} className="max-w-full">
            <Badge className="max-w-full pr-0.5">
              <span className="truncate">{label}</span>
              <Button variant="ghost" size="icon-xs" className="rounded-full" onClick={() => onRemove(id)} aria-label={`Remove ${label}`}>
                <XIcon />
              </Button>
            </Badge>
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

/** a segmented choice (shadcn ToggleGroup, one always chosen), for two to four short options */
export function Segmented<T extends string>({ value, options, onChange, label, testId, className }: { value: T; options: { id: NoInfer<T>; label: ReactNode }[]; onChange(v: NoInfer<T>): void; label: string; testId?: string; className?: string }) {
  return (
    <ToggleGroup type="single" value={value} onValueChange={(v) => v && onChange(v as T)} aria-label={label} data-testid={testId} className={cn("h-auto min-h-9 max-w-full flex-wrap [&>button]:h-7.5", className)}>
      {options.map((o) => (
        <ToggleGroupItem key={o.id} value={o.id}>
          {o.label}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

/**
 * "How sure": how this kind of result was worked out, what it was checked
 * against and how close it came, plus this run's own numbers (solver, mesh).
 */
export function HowSure({ accuracy, facts }: { accuracy?: Accuracy | null; facts: (string | null | undefined | false)[] }) {
  const line = facts.filter(Boolean).join(" · ");
  return (
    <Collapsible className="group rounded-lg border bg-sunken" data-testid="how-sure">
      <CollapsibleTrigger className="flex w-full items-start gap-2 rounded-lg px-3 py-2.5 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50">
        <ShieldQuestionIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-2xs font-medium uppercase tracking-wider text-muted-foreground">How sure</span>
          <span className="text-xs text-soft">{accuracy ? firstSentence(accuracy.typical_error) : "No accuracy note for this result."}</span>
          {line && <span className="num text-2xs text-muted-foreground">{line}</span>}
        </span>
        {accuracy && <span className="mt-0.5 text-xs text-ember-text group-data-[state=open]:hidden">More</span>}
      </CollapsibleTrigger>
      {accuracy && (
        <CollapsibleContent className="flex flex-col gap-2 border-t px-3 py-2.5 text-xs text-soft">
          <p>
            <span className="font-medium text-foreground">How it is worked out: </span>
            {accuracy.method}
          </p>
          <p>
            <span className="font-medium text-foreground">Checked against: </span>
            {accuracy.checked_against}
          </p>
          <p>
            <span className="font-medium text-foreground">Typical error: </span>
            {accuracy.typical_error}
          </p>
        </CollapsibleContent>
      )}
    </Collapsible>
  );
}
const firstSentence = (s: string) => (/^.*?[.;](\s|$)/.exec(s)?.[0] ?? s).replace(/;\s*$/, ".").trim();

/** "180,000 pieces of 1.2 mm" */
export const meshFact = (elements?: number | null, elementMm?: number | null) =>
  elements ? `${n(elements)} pieces${elementMm ? ` of ${n(elementMm, 2)} mm` : ""}` : null;
