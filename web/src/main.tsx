// Fonts: Geist and Geist Mono, variable, Latin only (OFL, self-hosted).
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./styles/globals.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider } from "./lib/app";
import App from "./App";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Loader } from "@/components/ui/skeleton";
import { Toaster } from "@/components/ui/sonner";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AppProvider
      fallback={
        <div className="grid min-h-dvh place-items-center">
          <Loader />
        </div>
      }
    >
      <TooltipProvider>
        <App />
        <Toaster position="bottom-center" />
      </TooltipProvider>
    </AppProvider>
  </StrictMode>,
);
