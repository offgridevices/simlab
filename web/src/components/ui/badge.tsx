import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

// pills: a tint of one named colour with its readable text
const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full px-2 py-0.5 text-xs font-medium [&>svg]:size-3 [&>svg]:pointer-events-none",
  {
    variants: {
      tone: {
        neutral: "bg-foreground/[0.07] text-soft",
        ember: "bg-ember/15 text-ember-text",
        moss: "bg-moss/15 text-moss-text",
        sky: "bg-sky/15 text-sky-text",
        gold: "bg-gold/15 text-gold-text",
        plum: "bg-plum/15 text-plum-text",
        clay: "bg-clay/15 text-clay-text",
        solid: "bg-primary text-primary-foreground",
        outline: "border border-input text-soft",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

function Badge({ className, tone, ...props }: React.ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return <span data-slot="badge" className={cn(badgeVariants({ tone }), className)} {...props} />;
}

export { Badge, badgeVariants };
