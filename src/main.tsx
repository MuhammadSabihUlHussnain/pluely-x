// Window router: the Tauri window label picks the UI.
//   main               → overlay panel
//   dashboard          → sessions, settings, onboarding (route /dashboard/*)
//   capture-overlay-*  → region selector (route /capture?monitor=<name>)
// In a plain browser (mock mode) the label is derived from the URL path.

import React from "react";
import ReactDOM from "react-dom/client";
import { ErrorBoundary } from "react-error-boundary";
import { CaptureApp } from "./app/capture/CaptureApp";
import { DashboardApp } from "./app/dashboard/DashboardApp";
import { OverlayApp } from "./app/overlay/OverlayApp";
import { native } from "./core/native";
import "./global.css";

type Surface = "overlay" | "dashboard" | "capture";

function surfaceFor(label: string): Surface {
  if (label === "main") return "overlay";
  if (label.startsWith("capture-overlay")) return "capture";
  return "dashboard";
}

const surface = surfaceFor(native.windowLabel());
document.body.classList.add(`surface-${surface}`);
// Browser mock only: paint a stand-in "app behind the overlay" so translucency reads.
if (native.isMock && surface === "overlay") document.body.classList.add("mock-backdrop");
document.title = surface === "dashboard" ? "Pluely X" : surface === "capture" ? "Pluely X — select a region" : "Pluely X overlay";

// The overlay and region picker are always dark; the dashboard follows Windows.
const media = window.matchMedia("(prefers-color-scheme: dark)");
const applyTheme = () => document.documentElement.classList.toggle("dark", surface !== "dashboard" || media.matches);
applyTheme();
media.addEventListener("change", applyTheme);

function Crash({ error }: { error: unknown }) {
  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="max-w-[360px] rounded-xl border border-border bg-card p-4 text-[13.5px]">
        <p className="font-semibold">Something broke in this window.</p>
        <p className="mt-1 text-muted-foreground">{error instanceof Error ? error.message : String(error)}</p>
        <button
          type="button"
          onClick={() => window.location.reload()}
          className="mt-3 rounded-lg bg-primary px-3 py-1.5 font-medium text-primary-foreground"
        >
          Reload
        </button>
      </div>
    </div>
  );
}

function logCrash(error: unknown) {
  const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
  void native.logWrite("error", `[${surface}] render crash: ${message}`).catch(() => {});
}

const App = surface === "overlay" ? OverlayApp : surface === "capture" ? CaptureApp : DashboardApp;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary FallbackComponent={Crash} onError={logCrash}>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
