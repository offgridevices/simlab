import { lazy, Suspense } from "react";
import { parse, useHash, href } from "./lib/router";
import { Screen, TopBar, Centre } from "./components/topbar";
import { Loader } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";
import Home from "./screens/home";

// the workspace brings three.js and Settings is rarely opened: their own chunks
const Workspace = lazy(() => import("./screens/workspace"));
const Settings = lazy(() => import("./screens/settings"));

export default function App() {
  const route = parse(useHash());
  const fallback = (
    <div className="grid min-h-dvh place-items-center">
      <Loader />
    </div>
  );
  switch (route.name) {
    case "home":
      return <Home />;
    case "project":
      return (
        <Suspense fallback={fallback}>
          <Workspace key={route.id} id={route.id} query={route.query} />
        </Suspense>
      );
    case "settings":
      return (
        <Suspense fallback={fallback}>
          <Settings />
        </Suspense>
      );
    default:
      return (
        <Screen>
          <TopBar back />
          <Centre>
            <div className="flex max-w-md flex-col items-start gap-4">
              <h1 className="text-2xl font-semibold tracking-tight">Nothing here</h1>
              <p className="text-soft">This page does not exist.</p>
              <Button variant="outline" asChild>
                <a href={href("/")}>All projects</a>
              </Button>
            </div>
          </Centre>
        </Screen>
      );
  }
}
