// Panel building blocks: a titled section, a row of a label and a value,
// a little disclosure for the details most people skip.
import type { ReactNode } from "react";
import { ChevronDownIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export function Section({ title, aside, children, className }: { title: ReactNode; aside?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={cn("flex flex-col gap-3", className)}>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3 className="text-sm font-semibold">{title}</h3>
        {aside && <div className="text-xs text-muted-foreground">{aside}</div>}
      </div>
      {children}
    </section>
  );
}

export function Stat({ label, value, unit, hint, className }: { label: ReactNode; value: ReactNode; unit?: string; hint?: ReactNode; className?: string }) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-0.5 rounded-lg bg-sunken px-3 py-2.5", className)}>
      <span className="truncate text-xs text-muted-foreground">{label}</span>
      <span className="num text-lg font-semibold leading-tight">
        {value}
        {unit && <span className="ml-1 text-sm font-normal text-muted-foreground">{unit}</span>}
      </span>
      {hint && <span className="text-xs text-muted-foreground">{hint}</span>}
    </div>
  );
}

export function Row({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-1.5 text-sm">
      <span className="text-muted-foreground">{label}</span>
      <span className="num min-w-0 text-right">{children}</span>
    </div>
  );
}

export function Details({ summary, children, testId, defaultOpen }: { summary: ReactNode; children: ReactNode; testId?: string; defaultOpen?: boolean }) {
  return (
    <details className="group rounded-lg border" data-testid={testId} open={defaultOpen}>
      <summary className="flex cursor-pointer list-none items-center gap-2 rounded-lg px-3 py-2.5 text-sm font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 [&::-webkit-details-marker]:hidden">
        <span className="grow">{summary}</span>
        <ChevronDownIcon className="size-4 text-muted-foreground transition-transform group-open:rotate-180" aria-hidden="true" />
      </summary>
      <div className="border-t px-3 py-2">{children}</div>
    </details>
  );
}
