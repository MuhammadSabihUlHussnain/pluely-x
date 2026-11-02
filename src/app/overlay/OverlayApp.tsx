import { AlertTriangle, Keyboard, Maximize2, MousePointerClick, X } from "lucide-react";
import { useEffect, useState } from "react";
import { IconButton, Kbd } from "@/components/common";
import { TooltipProvider } from "@/components/ui/tooltip";
import { MODE_META, SHORTCUT_LABELS } from "@/core/defaults";
import { useLive } from "@/core/live/session";
import { native } from "@/core/native";
import { displayAccelerator } from "@/core/shortcuts";
import { useSettings, useShortcutHealth } from "@/core/settings";
import { cn } from "@/lib/utils";
import { ModeIcon } from "../shared/modes";
import { AnswerStack } from "./AnswerStack";
import { AudioCheckBanner } from "./AudioCheckBanner";
import { ScanPanel } from "./ScanPanel";
import { AskBox } from "./AskBox";
import { expand, initOverlay, openDashboard } from "./controller";
import { useOverlay } from "./store";
import { ListenButton, onDragAreaMouseDown, TopBar } from "./TopBar";
import { TranscriptStrip } from "./TranscriptStrip";

export function OverlayApp() {
  const collapsed = useOverlay((s) => s.collapsed);
  const opacity = useSettings((s) => s.settings.overlay.opacity);

  useEffect(() => {
    let dispose: (() => void) | undefined;
    void initOverlay().then((d) => (dispose = d));
    return () => dispose?.();
  }, []);

  return (
    <TooltipProvider>
      <div className="h-screen w-screen p-0" style={{ ["--panel-alpha" as string]: String(opacity) }}>
        {collapsed ? <Pill /> : <Panel />}
      </div>
    </TooltipProvider>
  );
}

function Panel() {
  const clickThrough = useOverlay((s) => s.win?.clickThrough);
  const toggleKey = useSettings((s) => s.settings.shortcuts.toggle_click_through.key);

  return (
    <main className="glass relative flex h-full w-full flex-col overflow-hidden rounded-[14px]" aria-label="Pluely X overlay">
      <TopBar />
      {clickThrough ? (
        <div className="flex shrink-0 items-center gap-2 border-b border-white/[0.07] bg-you/10 px-3 py-1.5 text-[12.5px] text-foreground">
          <MousePointerClick className="size-3.5 text-you" />
          <span className="flex-1">Click-through is on — clicks go to the app below.</span>
          <span className="flex items-center gap-1 text-muted-foreground">
            Turn off <Kbd keys={toggleKey} />
          </span>
        </div>
      ) : null}
      <LiveErrorBanner />
      <ScanPanel />
      <AudioCheckBanner />
      <ShortcutConflictBanner />
      <TranscriptStrip />
      <AnswerStack />
      <AskBox />
      <Toast />
      <ResizeGrip />
    </main>
  );
}

function LiveErrorBanner() {
  const error = useLive((s) => s.error);
  if (!error) return null;
  return (
    <div
      role="alert"
      className={cn(
        "flex shrink-0 items-start gap-2 border-b border-white/[0.07] px-3 py-2 text-[13px]",
        error.fatal ? "bg-danger/[0.12]" : "bg-warn/[0.1]",
      )}
    >
      <AlertTriangle className={cn("mt-0.5 size-4 shrink-0", error.fatal ? "text-danger" : "text-warn")} />
      <div className="min-w-0 flex-1">
        <p className="font-medium text-foreground">{error.title}</p>
        {error.fatal && error.message ? <p className="text-[12.5px] text-muted-foreground">{error.message}</p> : null}
      </div>
      {error.fix === "mic_privacy" ? (
        <button
          type="button"
          onClick={() => void native.audioOpenMicPrivacySettings()}
          className="shrink-0 rounded-md bg-primary px-2 py-0.5 text-[12.5px] font-medium text-primary-foreground"
        >
          Open privacy settings
        </button>
      ) : error.fix === "settings" ? (
        <button
          type="button"
          onClick={() => openDashboard("/dashboard/settings#keys")}
          className="shrink-0 rounded-md bg-primary px-2 py-0.5 text-[12.5px] font-medium text-primary-foreground"
        >
          Open settings
        </button>
      ) : null}
      <IconButton size="sm" label="Dismiss" onClick={() => useLive.setState({ error: null })}>
        <X />
      </IconButton>
    </div>
  );
}

function ShortcutConflictBanner() {
  const failures = useShortcutHealth((s) => s.failures);
  const [dismissed, setDismissed] = useState<string>("");
  const signature = failures.map((f) => `${f.action}:${f.key}`).join(",");
  if (!failures.length || dismissed === signature) return null;
  const list = failures
    .slice(0, 3)
    .map((f) => `${displayAccelerator(f.key)} (${SHORTCUT_LABELS[f.action].label})`)
    .join(", ");
  return (
    <div role="status" className="flex shrink-0 items-start gap-2 border-b border-white/[0.07] bg-warn/[0.09] px-3 py-2 text-[13px]">
      <Keyboard className="mt-0.5 size-4 shrink-0 text-warn" />
      <p className="min-w-0 flex-1 text-foreground">
        {failures.length === 1 ? "A shortcut is" : `${failures.length} shortcuts are`} taken by another app: {list}
        {failures.length > 3 ? "…" : ""}
      </p>
      <button
        type="button"
        onClick={() => openDashboard("/dashboard/settings#shortcuts")}
        className="shrink-0 rounded-md bg-primary px-2 py-0.5 text-[12.5px] font-medium text-primary-foreground"
      >
        Change
      </button>
      <IconButton size="sm" label="Dismiss" onClick={() => setDismissed(signature)}>
        <X />
      </IconButton>
    </div>
  );
}

function Toast() {
  const toast = useOverlay((s) => s.toast);
  if (!toast) return null;
  return (
    <div
      key={toast.id}
      role="status"
      className={cn(
        "card-in pointer-events-none absolute bottom-[104px] left-1/2 max-w-[90%] -translate-x-1/2 rounded-lg border px-3 py-1.5 text-center text-[13px] shadow-lg",
        toast.tone === "error" ? "border-danger/40 bg-popover text-foreground" : "border-white/10 bg-popover text-foreground",
      )}
    >
      {toast.text}
    </div>
  );
}

function ResizeGrip() {
  return (
    <div
      aria-hidden
      title="Drag to resize"
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        void native.startResizeDragging("SouthEast").catch(() => {});
      }}
      className="absolute bottom-0 right-0 z-10 size-3.5 cursor-se-resize"
    >
      <svg viewBox="0 0 14 14" className="size-full text-subtle/60">
        <path d="M13 5 L5 13 M13 9 L9 13" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
      </svg>
    </div>
  );
}

function Pill() {
  const mode = useSettings((s) => s.settings.mode);
  const status = useLive((s) => s.status);
  return (
    <div
      className="glass flex h-full w-full items-center gap-1.5 rounded-full pl-3 pr-1.5"
      onMouseDown={onDragAreaMouseDown}
      onDoubleClick={() => expand()}
    >
      <button
        type="button"
        onClick={() => expand()}
        className="flex min-w-0 items-center gap-1.5 rounded-md text-[13px] font-medium text-foreground"
        aria-label={`Expand overlay (${MODE_META[mode].label} mode)`}
      >
        <ModeIcon mode={mode} className="size-4 text-muted-foreground" />
        <span className="truncate">{MODE_META[mode].label}</span>
      </button>
      <span className="flex-1" />
      {status === "idle" ? <span className="text-[12.5px] text-subtle">Idle</span> : <ListenButton compact />}
      <IconButton size="sm" label="Expand" onClick={() => expand()}>
        <Maximize2 />
      </IconButton>
    </div>
  );
}
