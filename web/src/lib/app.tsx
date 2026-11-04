// The app's shared pieces: the API client (the engine, or demo mode's fake
// one), the live event stream fanned out to whoever listens, and the
// material and motor library.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createApi, sseSubscribe, type Api, type LinkState } from "./api";
import type { EngineEvent, Material, Motor } from "./types";

export const MOCK = import.meta.env.VITE_MOCK === "1";

async function makeApi(): Promise<Api> {
  if (MOCK) {
    const { mockTransport } = await import("../mock/engine");
    return createApi(mockTransport());
  }
  return createApi({ base: "", fetch: (input, init) => fetch(input, init), subscribe: sseSubscribe("/api/events") });
}

interface AppValue {
  api: Api;
  link: LinkState;
  /** listen to live events; returns a stop function */
  listen(f: (e: EngineEvent) => void): () => void;
  materials: Material[] | null;
  motors: Motor[] | null;
  reloadLibrary(): void;
}

const Ctx = createContext<AppValue | null>(null);

export function AppProvider({ children, fallback }: { children: ReactNode; fallback: ReactNode }) {
  const [api, setApi] = useState<Api | null>(null);
  const [link, setLink] = useState<LinkState>("connecting");
  const listeners = useRef(new Set<(e: EngineEvent) => void>());
  const [materials, setMaterials] = useState<Material[] | null>(null);
  const [motors, setMotors] = useState<Motor[] | null>(null);
  const [libTick, setLibTick] = useState(0);

  useEffect(() => {
    let live = true;
    makeApi().then((a) => live && setApi(a));
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (!api) return;
    return api.subscribe((e) => listeners.current.forEach((l) => l(e)), setLink);
  }, [api]);

  useEffect(() => {
    if (!api) return;
    let live = true;
    api.materials().then((m) => live && setMaterials(m), () => live && setMaterials([]));
    api.motors().then((m) => live && setMotors(m), () => live && setMotors([]));
    return () => {
      live = false;
    };
  }, [api, libTick]);

  const listen = useCallback((f: (e: EngineEvent) => void) => {
    listeners.current.add(f);
    return () => {
      listeners.current.delete(f);
    };
  }, []);
  const reloadLibrary = useCallback(() => setLibTick((t) => t + 1), []);

  const value = useMemo(() => (api ? { api, link, listen, materials, motors, reloadLibrary } : null), [api, link, listen, materials, motors, reloadLibrary]);
  if (!value) return <>{fallback}</>;
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp outside AppProvider");
  return v;
}

/** run `f` for every live event while the component is on screen */
export function useEvents(f: (e: EngineEvent) => void) {
  const { listen } = useApp();
  const ref = useRef(f);
  ref.current = f;
  useEffect(() => listen((e) => ref.current(e)), [listen]);
}
