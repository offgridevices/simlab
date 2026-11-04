import { cn } from "@/lib/utils";

/** A thin bar sliding while something loads. */
function Loader({ className }: { className?: string }) {
  return (
    <div role="progressbar" aria-label="Loading" className={cn("relative h-0.5 w-40 overflow-hidden rounded-full bg-foreground/10", className)}>
      <span className="absolute inset-y-0 left-0 w-2/5 rounded-full bg-primary [animation:hh-slide_1.1s_cubic-bezier(.4,0,.2,1)_infinite]" />
    </div>
  );
}

export { Loader };
