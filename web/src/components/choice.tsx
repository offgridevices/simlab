// A choice drawn as a row: a radio dot or a checkbox, a title, a line of
// detail. Used for questions in the chat and for options in Settings.
import type { ComponentProps, ReactNode } from "react";
import { CheckIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export function ChoiceRow({
  checked,
  multi,
  title,
  detail,
  tag,
  className,
  ...props
}: Omit<ComponentProps<"button">, "title"> & { checked: boolean; multi?: boolean; title: ReactNode; detail?: ReactNode; tag?: ReactNode }) {
  return (
    <button
      type="button"
      role={multi ? "checkbox" : "radio"}
      aria-checked={checked}
      className={cn(
        "flex w-full items-start gap-3 rounded-lg border bg-card px-3.5 py-3 text-left transition-colors outline-none hover:border-input focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-60",
        checked && "border-ember/60 bg-ember/[0.06] hover:border-ember/60",
        className,
      )}
      {...props}
    >
      <span
        aria-hidden="true"
        className={cn(
          "mt-0.5 grid size-4 shrink-0 place-items-center border border-input",
          multi ? "rounded-[4px]" : "rounded-full",
          checked && "border-primary bg-primary text-primary-foreground",
        )}
      >
        {checked && (multi ? <CheckIcon className="size-3" strokeWidth={3} /> : <span className="size-1.5 rounded-full bg-primary-foreground" />)}
      </span>
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="flex flex-wrap items-center gap-2 font-medium">
          {title}
          {tag}
        </span>
        {detail && <span className="text-sm text-muted-foreground">{detail}</span>}
      </span>
    </button>
  );
}
