import {
  Check,
  ChevronDown,
  FileText,
  LayoutGrid,
  Loader2,
  Mic,
  Minimize2,
  MousePointerClick,
  ShieldCheck,
  ShieldOff,
} from "lucide-react";
import { useEffect, useState } from "react";
import { Hint, IconButton, Kbd } from "@/components/common";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { MODE_META } from "@/core/defaults";
import { loadBriefs, toggleListening, useLive } from "@/core/live/session";
import { native } from "@/core/native";
import { useSettings } from "@/core/settings";
import { MODES, type Mode } from "@/core/types";
import { formatClock } from "@/core/util";
import { cn } from "@/lib/utils";
import { ModeIcon } from "../shared/modes";
import { collapse, openDashboard, setClickThrough, setContentProtection, setMode } from "./controller";
import { LevelMeter } from "./LevelMeter";
import { useOverlay } from "./store";

/** Mouse-down on empty bar space drags the frameless window. */
export function onDragAreaMouseDown(e: React.MouseEvent) {
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  if (t.closest("button, a, input, textarea, [role='menu'], [data-no-drag]")) return;
  void native.startDragging().catch(() => {});
}

export function TopBar() {
  const mode = useSettings((s) => s.settings.mode);
  const keys = useSettings((s) => s.settings.shortcuts);
  const win = useOverlay((s) => s.win);
  const protectedFromShare = win?.contentProtected ?? true;

  return (
    <header
      className="flex h-12 shrink-0 items-center gap-1.5 border-b border-white/[0.07] px-2"
      onMouseDown={onDragAreaMouseDown}
    >
      <ModeMenu mode={mode} />
      <ListenButton />
      <div className="min-w-2 flex-1 self-stretch" aria-hidden />
      <IconButton
        label={
          protectedFromShare
            ? "Hidden from screen share — click to make it visible"
            : "Visible to screen share — click to hide it"
        }
        tone={protectedFromShare ? "ok" : "warn"}
        onClick={() => void setContentProtection(!protectedFromShare)}
      >
        {protectedFromShare ? <ShieldCheck /> : <ShieldOff />}
      </IconButton>
      <IconButton
        label="Click-through: clicks pass to the app below (turn off with the shortcut)"
        keys={keys.toggle_click_through.key}
        active={win?.clickThrough}
        onClick={() => void setClickThrough(!win?.clickThrough)}
      >
        <MousePointerClick />
      </IconButton>
      <IconButton
        label="Sessions & settings"
        keys={keys.toggle_dashboard.key}
        onClick={() => openDashboard("/dashboard")}
      >
        <LayoutGrid />
      </IconButton>
      <IconButton label="Collapse to a pill" onClick={() => void collapse()}>
        <Minimize2 />
      </IconButton>
    </header>
  );
}

function ModeMenu({ mode }: { mode: Mode }) {
  const briefs = useLive((s) => s.briefs);
  const override = useLive((s) => s.briefOverride);
  const defaultBrief = briefs.find((b) => b.mode === mode) ?? null;
  const activeBriefId = override === undefined ? defaultBrief?.id ?? null : override;

  return (
    <DropdownMenu onOpenChange={(open) => open && void loadBriefs()}>
      <Hint label="Mode — changes how answers are written">
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            className="flex h-8 items-center gap-1.5 rounded-lg pl-2 pr-1.5 text-[13.5px] font-medium text-foreground transition-colors hover:bg-foreground/[0.08] data-[state=open]:bg-foreground/[0.1]"
          >
            <ModeIcon mode={mode} className="size-4 text-muted-foreground" />
            {MODE_META[mode].label}
            <ChevronDown className="size-3.5 text-subtle" />
          </button>
        </DropdownMenuTrigger>
      </Hint>
      <DropdownMenuContent align="start" collisionPadding={6} className="w-[336px] p-1.5">
        <DropdownMenuLabel className="px-2 pb-1 pt-0.5 text-[12px] font-medium text-subtle">Mode</DropdownMenuLabel>
        {MODES.map((m) => (
          <DropdownMenuItem key={m} onSelect={() => void setMode(m)} className="items-start gap-2.5 rounded-lg px-2 py-1.5">
            <ModeIcon mode={m} className="mt-0.5 size-4" />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="text-[13.5px] font-medium">{MODE_META[m].label}</span>
              <span className="text-[12.5px] leading-snug text-muted-foreground">{MODE_META[m].blurb}</span>
            </span>
            {m === mode ? <Check className="mt-0.5 size-4 text-foreground" /> : null}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="px-2 pb-1 pt-0.5 text-[12px] font-medium text-subtle">
          Brief for this conversation
        </DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => useLive.setState({ briefOverride: null })} className="rounded-lg px-2 py-1.5">
          <span className="flex-1 text-[13.5px]">No brief</span>
          {activeBriefId === null ? <Check className="size-4" /> : null}
        </DropdownMenuItem>
        {briefs.map((b) => (
          <DropdownMenuItem
            key={b.id}
            onSelect={() => useLive.setState({ briefOverride: b.id })}
            className="rounded-lg px-2 py-1.5"
          >
            <FileText className="size-4" />
            <span className="min-w-0 flex-1 truncate text-[13.5px]">{b.name}</span>
            {b.mode === mode ? <span className="text-[12px] text-subtle">default</span> : null}
            {activeBriefId === b.id ? <Check className="size-4" /> : null}
          </DropdownMenuItem>
        ))}
        <DropdownMenuItem
          onSelect={() => openDashboard("/dashboard/settings#modes")}
          className="rounded-lg px-2 py-1.5 text-[13px] text-muted-foreground"
        >
          Manage modes and briefs…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

export function ListenButton({ compact = false }: { compact?: boolean }) {
  const status = useLive((s) => s.status);
  const sources = useLive((s) => s.sources);
  const levels = useLive((s) => s.levels);
  const startedAt = useLive((s) => s.startedAtMs);
  const mode = useSettings((s) => s.settings.mode);
  const key = useSettings((s) => s.settings.shortcuts.toggle_listen.key);
  const live = status === "live";
  const now = useNow(live);
  const reconnecting = sources.them.state === "reconnecting" || sources.me.state === "reconnecting";

  if (status === "idle") {
    return (
      <Hint
        label={MODE_META[mode].listens ? "Start listening — transcribes both sides" : "Start listening (switches to Meeting)"}
        keys={key}
      >
        <button
          type="button"
          onClick={() => void toggleListening()}
          className="flex h-8 items-center gap-1.5 rounded-lg border border-white/10 bg-foreground/[0.06] px-2.5 text-[13.5px] font-medium text-foreground transition-colors hover:bg-foreground/[0.11]"
        >
          <Mic className="size-4" />
          Listen
        </button>
      </Hint>
    );
  }

  if (status === "starting" || status === "stopping") {
    return (
      <span className="flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[13px] text-muted-foreground">
        <Loader2 className="size-4 animate-spin" />
        {status === "starting" ? "Starting…" : "Stopping…"}
      </span>
    );
  }

  const devices = [
    sources.them.device ? `Them: ${sources.them.device.name}` : null,
    sources.me.device ? `You: ${sources.me.device.name}` : null,
  ].filter(Boolean);

  return (
    <Hint
      label={
        <span className="flex flex-col gap-0.5">
          <span>Stop listening and write the summary</span>
          {devices.map((d) => (
            <span key={d} className="text-[12px] text-muted-foreground">
              {d}
            </span>
          ))}
        </span>
      }
      keys={key}
    >
      <button
        type="button"
        onClick={() => void toggleListening()}
        aria-label="Stop listening"
        className={cn(
          "group flex h-8 items-center gap-2 rounded-lg border px-2.5 text-[13px] font-medium transition-colors",
          reconnecting
            ? "border-warn/40 bg-warn/10 text-warn"
            : "border-live/35 bg-live/[0.13] text-foreground hover:bg-live/20",
        )}
      >
        <span className={cn("size-2 rounded-full", reconnecting ? "bg-warn" : "live-dot bg-live")} />
        <span className="tabular">{reconnecting ? "Reconnecting" : formatClock(now - (startedAt ?? now))}</span>
        {!compact && (
          <span className="flex items-center gap-1.5 border-l border-white/10 pl-2">
            <LevelMeter source="them" level={levels.them} />
            <LevelMeter source="me" level={levels.me} />
          </span>
        )}
      </button>
    </Hint>
  );
}

export function ShortcutLine({ label, keys }: { label: string; keys: string }) {
  return (
    <span className="flex items-center justify-between gap-3 text-[13px] text-muted-foreground">
      <span>{label}</span>
      <Kbd keys={keys} />
    </span>
  );
}
