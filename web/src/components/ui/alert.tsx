import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const alertVariants = cva("relative flex w-full items-start gap-3 rounded-lg px-4 py-3 text-sm [&>svg]:mt-0.5 [&>svg]:size-4 [&>svg]:shrink-0", {
  variants: {
    tone: {
      neutral: "bg-foreground/[0.05] text-soft",
      ember: "bg-ember/10 text-ember-text",
      moss: "bg-moss/10 text-moss-text",
      gold: "bg-gold/10 text-gold-text",
      clay: "bg-clay/10 text-clay-text",
    },
  },
  defaultVariants: { tone: "neutral" },
});

function Alert({ className, tone, ...props }: React.ComponentProps<"div"> & VariantProps<typeof alertVariants>) {
  return <div data-slot="alert" role="alert" className={cn(alertVariants({ tone }), className)} {...props} />;
}

export { Alert };
