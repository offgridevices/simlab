// A tiny hash router: three screens do not need a routing library, and
// hash links work from any static server (the engine, Tauri, a file).
//   #/                    projects
//   #/p/<id>?view=tests   a project (view, study, run, new: what the panels show)
//   #/settings
import { useSyncExternalStore } from "react";

export type Route =
  | { name: "home" }
  | { name: "project"; id: string; query: URLSearchParams }
  | { name: "settings" }
  | { name: "notfound" };

export function parse(hash: string): Route {
  const h = hash.replace(/^#/, "") || "/";
  const [path = "/", qs = ""] = h.split("?");
  const p = path.replace(/\/+$/, "") || "/";
  if (p === "/") return { name: "home" };
  if (p === "/settings") return { name: "settings" };
  const m = /^\/p\/([\w.-]+)$/.exec(p);
  if (m) return { name: "project", id: m[1]!, query: new URLSearchParams(qs) };
  return { name: "notfound" };
}

const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());
if (typeof window !== "undefined") window.addEventListener("hashchange", emit);

/** go to a route ("/p/abc?view=tests"); replace keeps the back button tidy for small changes */
export function navigate(to: string, replace = false) {
  const hash = `#${to}`;
  if (location.hash === hash) return;
  if (replace) {
    history.replaceState(null, "", hash);
    emit();
  } else location.hash = to;
}

const getHash = () => location.hash;
export function useHash(): string {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    getHash,
    () => "",
  );
}

/** a link target for an <a href> */
export const href = (to: string) => `#${to}`;
