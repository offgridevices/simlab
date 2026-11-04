import type { ReactNode } from "react";
import { ChevronLeftIcon, SettingsIcon } from "lucide-react";
import { Lockup } from "./brand";
import { MOCK, useApp } from "../lib/app";
import { href } from "../lib/router";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";

export function TopBar({ children, back, current, actions }: { children?: ReactNode; back?: boolean; current?: "settings"; actions?: ReactNode }) {
  const { link } = useApp();
  return (
    <header className="flex min-h-14 shrink-0 items-center gap-3 border-b px-3 sm:px-4">
      {back ? (
        <>
          <Button variant="ghost" size="sm" asChild className="-ml-1 px-2">
            <a href={href("/")} aria-label="All projects" data-testid="back">
              <ChevronLeftIcon />
              <span className={children ? "hidden sm:inline" : undefined}>Projects</span>
            </a>
          </Button>
          {children && <span className="hidden text-muted-foreground/60 sm:inline">/</span>}
        </>
      ) : (
        <Lockup />
      )}
      <div className="flex min-w-0 grow items-center gap-3">{children}</div>
      {link !== "open" && (
        <Badge tone="gold" title="Waiting for live updates from SimLab's engine" data-testid="link-state">
          {link === "connecting" ? "Connecting" : "Reconnecting"}
        </Badge>
      )}
      {MOCK && (
        <Badge tone="gold" title="Demo mode: made-up parts and results, no engine" data-testid="demo-tag">
          Demo
        </Badge>
      )}
      {actions}
      <Button variant={current === "settings" ? "secondary" : "ghost"} size="sm" asChild className="-mr-1">
        <a href={href("/settings")} aria-current={current === "settings" ? "page" : undefined} data-testid="nav-settings" aria-label="Settings">
          <SettingsIcon />
          <span className="hidden sm:inline">Settings</span>
        </a>
      </Button>
    </header>
  );
}

/** A full-height screen: the top bar, then whatever fills the rest. */
export function Screen({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={`flex min-h-dvh flex-col ${className ?? ""}`}>{children}</div>;
}

/** the middle of an empty area */
export function Centre({ children }: { children: ReactNode }) {
  return <div className="grid min-h-60 flex-1 place-items-center p-6">{children}</div>;
}
