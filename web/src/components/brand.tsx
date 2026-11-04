// The OffGrid ring, drawn from its geometry (OffGrid brand v3.2.2): an open
// ring of radius 58 and stroke 22 with round caps, and a node of radius 17 at
// the top, in a 200-unit box. The ring's centre is at (100, 108).
import { memo } from "react";
import { cn } from "@/lib/utils";

export const Mark = memo(function Mark({ size = 22, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 200 200" aria-hidden="true">
      <path d="M124.5 55.4A58 58 0 1 1 75.5 55.4" fill="none" stroke="var(--ember)" strokeWidth="22" strokeLinecap="round" />
      <circle cx="100" cy="40" r="17" fill="var(--ember)" />
    </svg>
  );
});

/** "SimLab by OffGrid", the mark and the name. */
export const Lockup = memo(function Lockup({ href = "#/", by = true, className }: { href?: string; by?: boolean; className?: string }) {
  return (
    <a
      className={cn("inline-flex items-center gap-2.5 whitespace-nowrap rounded-md text-md font-semibold tracking-tight no-underline outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50", className)}
      href={href}
      aria-label="SimLab by OffGrid, home"
    >
      <Mark />
      <span>SimLab</span>
      {by && <span className="hidden text-xs font-normal text-muted-foreground sm:inline">by OffGrid</span>}
    </a>
  );
});
