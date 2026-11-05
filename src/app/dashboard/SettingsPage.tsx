import { AlertCircle, FileText, FileUp, Loader2, Plus, RotateCcw, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router-dom";
import { Kbd } from "@/components/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { DEFAULT_MODE_PROMPTS } from "@/core/ai/prompts";
import { BRIEF_ACCEPT, extractBriefText } from "@/core/briefs/extract";
import {
  approxTokens,
  BRIEF_MAX_CHARS,
  composeBrief,
  parseSources,
  serializeSources,
  sourceNames,
  type BriefDoc,
} from "@/core/briefs/sources";
import { getRepo, type BriefRow } from "@/core/db";
import { downloadText, sessionToMarkdown } from "@/core/db/markdown";
import { DEFAULT_SHORTCUTS, MODE_META, MODEL_OPTIONS, SHORTCUT_LABELS } from "@/core/defaults";
import { native } from "@/core/native";
import { duplicateActions, knownConflict } from "@/core/shortcuts";
import { useSettings, useShortcutHealth } from "@/core/settings";
import { MODES, type Effort, type Mode, type ShortcutAction } from "@/core/types";
import { newId } from "@/core/util";
import { cn } from "@/lib/utils";
import { ModeIcon } from "../shared/modes";
import { DeviceSelect } from "./DeviceSelect";
import { BigMeter, KeyStatusRow, LiveWords, Row, Section, useAudioTest } from "./parts";
import { ShortcutRecorder } from "./ShortcutRecorder";
import { IS_MAC, KEY_STORE } from "@/core/platform";

const TOC = [
  ["keys", "Keys & models"],
  ["audio", "Audio"],
  ["shortcuts", "Shortcuts"],
  ["overlay", "Overlay"],
  ["modes", "Modes & briefs"],
  ["data", "Data"],
] as const;

export function SettingsPage() {
  const location = useLocation();
  const scroller = useRef<HTMLDivElement>(null);
  const [current, setCurrent] = useState<string>("keys");

  useEffect(() => {
    const id = location.hash.replace("#", "");
    if (!id) return;
    document.getElementById(id)?.scrollIntoView({ block: "start" });
    // Let the scroll settle, then mark the requested section as current.
    const t = setTimeout(() => setCurrent(id), 60);
    return () => clearTimeout(t);
  }, [location.hash]);

  useEffect(() => {
    const root = scroller.current;
    if (!root) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onScroll = () => {
      clearTimeout(timer);
      timer = setTimeout(compute, 30);
    };
    const compute = () => {
      const top = root.getBoundingClientRect().top;
      let cur: string = TOC[0][0];
      for (const [id] of TOC) {
        const el = document.getElementById(id);
        if (el && el.getBoundingClientRect().top - top <= 96) cur = id;
      }
      // At the very bottom, the last section wins even if it's short.
      if (root.scrollTop + root.clientHeight >= root.scrollHeight - 4) cur = TOC[TOC.length - 1][0];
      setCurrent(cur);
    };
    onScroll();
    root.addEventListener("scroll", onScroll, { passive: true });
    // Sections above can grow after their data loads; re-check when they do.
    const ro = new ResizeObserver(onScroll);
    if (root.firstElementChild) ro.observe(root.firstElementChild);
    return () => {
      clearTimeout(timer);
      ro.disconnect();
      root.removeEventListener("scroll", onScroll);
    };
  }, []);

  return (
    <div ref={scroller} className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-[960px] gap-10 px-8 py-8">
        <nav aria-label="Settings sections" className="sticky top-8 hidden h-fit w-[160px] shrink-0 lg:block">
          <h1 className="mb-4 text-[20px] font-semibold tracking-[-0.015em]">Settings</h1>
          <ul className="flex flex-col gap-0.5">
            {TOC.map(([id, label]) => (
              <li key={id}>
                <a
                  href={`#${id}`}
                  onClick={(e) => {
                    e.preventDefault();
                    document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
                    setTimeout(() => setCurrent(id), 450);
                  }}
                  className={cn(
                    "block rounded-md px-2 py-1 text-[13.5px] transition-colors",
                    current === id ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <div className="min-w-0 max-w-[680px] flex-1">
          <h1 className="mb-2 text-[20px] font-semibold lg:hidden">Settings</h1>
          <SaveError />
          <KeysSection />
          <AudioSection />
          <ShortcutsSection />
          <OverlaySection />
          <ModesSection />
          <DataSection />
        </div>
      </div>
    </div>
  );
}

function SaveError() {
  const error = useSettings((s) => s.error);
  if (!error) return null;
  return (
    <p role="alert" className="mb-2 rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-[13.5px]">
      That change wasn't saved: {error}
      <button type="button" className="ml-2 underline underline-offset-2" onClick={() => useSettings.setState({ error: null })}>
        Dismiss
      </button>
    </p>
  );
}

// ---------------------------------------------------------------------------

const EFFORT_LABEL: Record<Effort, string> = { low: "Low — fastest", medium: "Medium", high: "High — most thorough" };

function EffortSelect({ id, value, onChange }: { id: string; value: Effort; onChange: (e: Effort) => void }) {
  return (
    <Select value={value} onValueChange={(v) => onChange(v as Effort)}>
      <SelectTrigger id={id} className="h-10 w-[220px] text-[13.5px]">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {(["low", "medium", "high"] as Effort[]).map((e) => (
          <SelectItem key={e} value={e}>
            {EFFORT_LABEL[e]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function KeysSection() {
  const ai = useSettings((s) => s.settings.ai);
  const update = useSettings((s) => s.update);
  const [maxTokens, setMaxTokens] = useState(String(ai.maxTokens));
  useEffect(() => setMaxTokens(String(ai.maxTokens)), [ai.maxTokens]);

  return (
    <Section
      id="keys"
      title="Keys & models"
      description={`Keys live in ${KEY_STORE}. They never reach this window and are never shown again after saving.`}
    >
      <KeyStatusRow provider="anthropic" />
      <KeyStatusRow provider="deepgram" />
      <Row
        label="Model"
        description="Used for every answer, note and summary. Opus 5.5 answers best; Sonnet and Haiku are faster and cheaper."
        htmlFor="model"
      >
        <Select value={ai.model} onValueChange={(v) => void update({ ai: { model: v } })}>
          <SelectTrigger id="model" className="h-10 w-[220px] text-[13.5px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {MODEL_OPTIONS.map((m) => (
              <SelectItem key={m.id} value={m.id}>
                {m.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row
        label="Live answers effort"
        description="Auto-answers and “What should I say?”. Low keeps answers arriving within about a second."
        htmlFor="live-effort"
      >
        <EffortSelect id="live-effort" value={ai.liveEffort} onChange={(e) => void update({ ai: { liveEffort: e } })} />
      </Row>
      <Row label="Deep work effort" description="Solving the screen in Coding mode, rolling notes and session summaries." htmlFor="deep-effort">
        <EffortSelect id="deep-effort" value={ai.deepEffort} onChange={(e) => void update({ ai: { deepEffort: e } })} />
      </Row>
      <Row label="Longest answer" description="Maximum tokens per answer. 16,000 keeps long code answers from being cut off." htmlFor="max-tokens">
        <Input
          id="max-tokens"
          type="number"
          min={256}
          max={128000}
          step={1000}
          value={maxTokens}
          onChange={(e) => setMaxTokens(e.target.value)}
          onBlur={() => {
            const n = Math.min(128000, Math.max(256, Math.round(Number(maxTokens) || 16000)));
            setMaxTokens(String(n));
            if (n !== ai.maxTokens) void update({ ai: { maxTokens: n } });
          }}
          className="h-10 w-[120px] text-[13.5px] tabular"
        />
      </Row>
    </Section>
  );
}

const LANGUAGES: [string, string][] = [
  ["en", "English"],
  ["en-US", "English (US)"],
  ["en-GB", "English (UK)"],
  ["en-IN", "English (India)"],
  ["multi", "Multilingual (auto)"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["de", "German"],
  ["hi", "Hindi"],
  ["pt", "Portuguese"],
  ["it", "Italian"],
  ["nl", "Dutch"],
  ["ja", "Japanese"],
];

function AudioSection() {
  const s = useSettings((st) => st.settings);
  const update = useSettings((st) => st.update);
  const test = useAudioTest();
  const running = test.state === "running";

  return (
    <Section id="audio" title="Audio" description="Pluely X hears your microphone as “You” and whatever your computer plays as “Them”.">
      <Row label="Microphone" htmlFor="mic">
        <DeviceSelect id="mic" kind="input" value={s.audio.micDeviceId} onChange={(v) => void update({ audio: { micDeviceId: v } })} />
      </Row>
      <Row label="Call audio" description="The speakers or headphones your calls play through." htmlFor="out">
        <DeviceSelect id="out" kind="output" value={s.audio.outputDeviceId} onChange={(v) => void update({ audio: { outputDeviceId: v } })} />
      </Row>
      <Row
        label="Echo cancellation"
        description={
          IS_MAC
            ? "Not available on macOS yet; your own voice is de-duplicated from the transcript either way."
            : "Windows echo cancellation on the mic. Often unsupported; your own voice is de-duplicated from the transcript either way."
        }
        htmlFor="aec"
      >
        <Select value={s.audio.aec} onValueChange={(v) => void update({ audio: { aec: v as "auto" | "on" | "off" } })}>
          <SelectTrigger id="aec" className="h-10 w-[140px] text-[13.5px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">Auto</SelectItem>
            <SelectItem value="on">On</SelectItem>
            <SelectItem value="off">Off</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="Language" htmlFor="lang">
        <Select value={s.stt.language} onValueChange={(v) => void update({ stt: { language: v } })}>
          <SelectTrigger id="lang" className="h-10 w-[200px] text-[13.5px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {LANGUAGES.map(([v, l]) => (
              <SelectItem key={v} value={v}>
                {l}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row
        label="End of sentence after"
        description="Silence before a phrase counts as finished. Shorter is snappier; longer keeps a question asked with pauses in one piece. 700 ms is the default."
      >
        <div className="flex w-[240px] items-center gap-3">
          <Slider
            aria-label="End of sentence after (milliseconds)"
            min={100}
            max={1000}
            step={50}
            value={[s.stt.endpointingMs]}
            onValueChange={([v]) => useSettings.setState((st) => ({ settings: { ...st.settings, stt: { ...st.settings.stt, endpointingMs: v } } }))}
            onValueCommit={([v]) => void update({ stt: { endpointingMs: v } })}
          />
          <span className="w-14 text-right text-[13px] text-muted-foreground tabular">{s.stt.endpointingMs} ms</span>
        </div>
      </Row>
      <Row label="End of turn after" description="Silence before the other person's turn is treated as finished and checked for a question.">
        <div className="flex w-[240px] items-center gap-3">
          <Slider
            aria-label="End of turn after (milliseconds)"
            min={1000}
            max={3000}
            step={100}
            value={[s.stt.utteranceEndMs]}
            onValueChange={([v]) => useSettings.setState((st) => ({ settings: { ...st.settings, stt: { ...st.settings.stt, utteranceEndMs: v } } }))}
            onValueCommit={([v]) => void update({ stt: { utteranceEndMs: v } })}
          />
          <span className="w-14 text-right text-[13px] text-muted-foreground tabular">{(s.stt.utteranceEndMs / 1000).toFixed(1)} s</span>
        </div>
      </Row>
      <Row label="Tell speakers apart" description="Label different voices on the other side (useful for panels and group calls)." htmlFor="diarize">
        <Switch id="diarize" checked={s.stt.diarizeThem} onCheckedChange={(v) => void update({ stt: { diarizeThem: v } })} />
      </Row>
      <div className="rounded-xl border border-border bg-card p-4">
        <div className="mb-3 flex items-center gap-2">
          <div className="flex-1">
            <p className="text-[14px] font-medium">Audio test</p>
            <p className="text-[13px] text-muted-foreground">Runs a short session with these devices.</p>
          </div>
          <Button
            size="sm"
            variant={running ? "outline" : "default"}
            onClick={() => void (running ? test.stop() : test.start())}
            disabled={test.state === "starting" || test.state === "stopping"}
          >
            {test.state === "starting" ? <Loader2 className="animate-spin" /> : null}
            {running ? "Stop test" : "Start test"}
          </Button>
        </div>
        {running || test.transcript.order.length ? (
          <div className="flex flex-col gap-2.5">
            <BigMeter label="You" level={test.levels.me} tone="you" />
            <BigMeter label="Them" level={test.levels.them} tone="them" />
            <LiveWords transcript={test.transcript} />
          </div>
        ) : null}
        {test.error ? <p className="text-[13px] text-danger">{test.error}</p> : null}
      </div>
    </Section>
  );
}

const SHORTCUT_ORDER: ShortcutAction[] = [
  "toggle_overlay",
  "toggle_listen",
  "ask_now",
  "screenshot_full",
  "screenshot_region",
  "focus_input",
  "toggle_click_through",
  "toggle_dashboard",
  "move_up",
  "move_down",
  "move_left",
  "move_right",
];

function ShortcutsSection() {
  const shortcuts = useSettings((s) => s.settings.shortcuts);
  const update = useSettings((s) => s.update);
  const [editing, setEditing] = useState<ShortcutAction | null>(null);
  const failures = useShortcutHealth((s) => s.failures);
  const refreshHealth = useShortcutHealth((s) => s.refresh);

  useEffect(() => {
    void refreshHealth();
  }, [refreshHealth]);

  const save = async (action: ShortcutAction, patch: Partial<{ key: string; enabled: boolean }>) => {
    await update({ shortcuts: { [action]: { ...shortcuts[action], ...patch } } }).catch(() => {});
    // shortcut://error only reaches the overlay; ask Rust what registered.
    await refreshHealth();
  };

  return (
    <Section
      id="shortcuts"
      title="Shortcuts"
      description="These work from any app. The defaults avoid Teams, Zoom, Office and editor keys."
    >
      <div className="divide-y divide-border rounded-xl border border-border bg-card">
        {SHORTCUT_ORDER.map((a) => {
          const b = shortcuts[a];
          const failure = failures.find((f) => f.action === a);
          const dupes = b.enabled ? duplicateActions(shortcuts, b.key, a) : [];
          const taken = b.enabled ? knownConflict(b.key) : null;
          return (
            <div key={a} className="px-4 py-3">
              <div className="flex items-center gap-3">
                <div className="min-w-0 flex-1">
                  <p className={cn("text-[14px] font-medium", !b.enabled && "text-muted-foreground")}>{SHORTCUT_LABELS[a].label}</p>
                  <p className="text-[12.5px] text-muted-foreground">{SHORTCUT_LABELS[a].hint}</p>
                </div>
                <Kbd keys={b.key} className={cn(!b.enabled && "opacity-50")} />
                <Button variant="ghost" size="sm" onClick={() => setEditing(editing === a ? null : a)}>
                  Change
                </Button>
                <Switch
                  aria-label={`${SHORTCUT_LABELS[a].label} enabled`}
                  checked={b.enabled}
                  onCheckedChange={(v) => void save(a, { enabled: v })}
                />
              </div>
              {failure ? (
                <p className="mt-1.5 text-[13px] text-danger">
                  Another app already uses these keys, so this shortcut doesn't work. Choose different keys.
                </p>
              ) : dupes.length ? (
                <p className="mt-1.5 text-[13px] text-warn">Same keys as “{SHORTCUT_LABELS[dupes[0]].label}” — change one of them.</p>
              ) : taken ? (
                <p className="mt-1.5 text-[13px] text-warn">Often taken by {taken}.</p>
              ) : null}
              {editing === a ? (
                <ShortcutRecorder
                  action={a}
                  shortcuts={shortcuts}
                  onCancel={() => setEditing(null)}
                  onSave={(key) => {
                    setEditing(null);
                    void save(a, { key, enabled: true });
                  }}
                />
              ) : null}
            </div>
          );
        })}
      </div>
      <Button
        variant="outline"
        className="self-start"
        onClick={async () => {
          await update({ shortcuts: DEFAULT_SHORTCUTS }).catch(() => {});
          await refreshHealth();
        }}
      >
        <RotateCcw /> Reset to defaults
      </Button>
    </Section>
  );
}

function OverlaySection() {
  const o = useSettings((s) => s.settings.overlay);
  const update = useSettings((s) => s.update);
  return (
    <Section id="overlay" title="Overlay">
      <Row label="Background opacity" description="Text stays fully opaque; only the panel behind it changes.">
        <div className="flex w-[240px] items-center gap-3">
          <Slider
            aria-label="Background opacity"
            min={30}
            max={100}
            step={1}
            value={[Math.round(o.opacity * 100)]}
            onValueChange={([v]) =>
              useSettings.setState((st) => ({ settings: { ...st.settings, overlay: { ...st.settings.overlay, opacity: v / 100 } } }))
            }
            onValueCommit={([v]) => void update({ overlay: { opacity: v / 100 } })}
          />
          <span className="w-10 text-right text-[13px] text-muted-foreground tabular">{Math.round(o.opacity * 100)}%</span>
        </div>
      </Row>
      <Row label="Hide from screen share" description="Teams, Zoom, OBS and screenshots can't see the overlay. Recommended." htmlFor="protect">
        <Switch id="protect" checked={o.contentProtection} onCheckedChange={(v) => void update({ overlay: { contentProtection: v } })} />
      </Row>
      <Row label="Always on top" htmlFor="ontop">
        <Switch id="ontop" checked={o.alwaysOnTop} onCheckedChange={(v) => void update({ overlay: { alwaysOnTop: v } })} />
      </Row>
      <Row
        label="Click-through"
        description={
          <>
            Clicks pass to the app below. Turn it off with the shortcut or the tray menu.
          </>
        }
        htmlFor="clickthrough"
      >
        <Switch id="clickthrough" checked={o.clickThrough} onCheckedChange={(v) => void update({ overlay: { clickThrough: v } })} />
      </Row>
      <Row label="Launch at startup" description={IS_MAC ? "Start Pluely X in the menu bar when you log in." : "Start Pluely X hidden in the tray when Windows starts."} htmlFor="autostart">
        <Switch id="autostart" checked={o.launchAtStartup} onCheckedChange={(v) => void update({ overlay: { launchAtStartup: v } })} />
      </Row>
      <Row label="Overlay position" description="Move it back to the top centre of your main screen.">
        <Button variant="outline" size="sm" onClick={() => void native.windowResetPosition().catch(() => {})}>
          Reset position
        </Button>
      </Row>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Modes & briefs

function ModesSection() {
  const [briefs, setBriefs] = useState<BriefRow[]>([]);
  const reload = async () => setBriefs(await getRepo().listBriefs().catch(() => []));
  useEffect(() => {
    void reload();
  }, []);

  return (
    <Section
      id="modes"
      title="Modes & briefs"
      description="Each mode is a prompt you can tune. Briefs (résumé, job description, agenda, slides) are sent with every answer as a cached prefix, so long briefs stay fast."
    >
      <ModePromptEditor />
      <div>
        <h3 className="text-[14px] font-semibold">Default brief per mode</h3>
        <p className="mt-0.5 text-[13px] text-muted-foreground">You can still switch briefs per conversation from the overlay's mode menu.</p>
        <div className="mt-3 grid grid-cols-2 gap-x-6 gap-y-3">
          {MODES.filter((m) => m !== "ask").map((m) => {
            const current = briefs.find((b) => b.mode === m);
            return (
              <div key={m} className="flex items-center gap-3">
                <span className="flex w-[92px] items-center gap-1.5 text-[13.5px]">
                  <ModeIcon mode={m} className="size-4 text-muted-foreground" />
                  {MODE_META[m].label}
                </span>
                <Select
                  value={current?.id ?? "none"}
                  onValueChange={async (v) => {
                    await getRepo().setDefaultBrief(m, v === "none" ? null : v);
                    await reload();
                  }}
                >
                  <SelectTrigger aria-label={`Default brief for ${MODE_META[m].label}`} className="h-9 min-w-0 flex-1 text-[13px]">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="none">No brief</SelectItem>
                    {briefs.map((b) => (
                      <SelectItem key={b.id} value={b.id}>
                        {b.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            );
          })}
        </div>
      </div>
      <BriefManager briefs={briefs} onChange={reload} />
    </Section>
  );
}

function ModePromptEditor() {
  const [mode, setMode] = useState<Mode>("interview");
  const [custom, setCustom] = useState<Partial<Record<Mode, string>>>({});
  const [text, setText] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    void getRepo()
      .getModePrompts()
      .then(setCustom)
      .catch(() => {});
  }, []);
  useEffect(() => {
    setText(custom[mode] ?? DEFAULT_MODE_PROMPTS[mode]);
    setSaved(false);
  }, [mode, custom]);

  const isCustom = custom[mode] !== undefined;
  const dirty = text !== (custom[mode] ?? DEFAULT_MODE_PROMPTS[mode]);

  return (
    <div>
      <h3 className="text-[14px] font-semibold">Mode prompts</h3>
      <div role="tablist" aria-label="Mode" className="mt-3 flex flex-wrap gap-1">
        {MODES.map((m) => (
          <button
            key={m}
            role="tab"
            type="button"
            aria-selected={m === mode}
            onClick={() => setMode(m)}
            className={cn(
              "flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[13px] font-medium transition-colors",
              m === mode ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
            )}
          >
            <ModeIcon mode={m} className="size-4" />
            {MODE_META[m].label}
            {custom[m] !== undefined ? <span className="size-1.5 rounded-full bg-you" title="Customised" /> : null}
          </button>
        ))}
      </div>
      <Textarea
        aria-label={`${MODE_META[mode].label} prompt`}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setSaved(false);
        }}
        className="mt-3 min-h-[180px] text-[13.5px] leading-relaxed"
      />
      <div className="mt-2 flex items-center gap-2">
        <Button
          size="sm"
          disabled={!dirty || !text.trim()}
          onClick={async () => {
            await getRepo().setModePrompt(mode, text);
            setCustom((c) => ({ ...c, [mode]: text }));
            setSaved(true);
          }}
        >
          Save prompt
        </Button>
        {isCustom ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={async () => {
              await getRepo().resetModePrompt(mode);
              setCustom((c) => {
                const n = { ...c };
                delete n[mode];
                return n;
              });
            }}
          >
            <RotateCcw /> Reset to default
          </Button>
        ) : null}
        <span className="text-[13px] text-muted-foreground" aria-live="polite">
          {saved ? "Saved — new answers use it." : isCustom ? "Customised" : "Default prompt"}
        </span>
      </div>
    </div>
  );
}

function BriefManager({ briefs, onChange }: { briefs: BriefRow[]; onChange: () => Promise<void> }) {
  const [editing, setEditing] = useState<BriefRow | null>(null);

  const startNew = () => {
    const now = Date.now();
    setEditing({
      id: newId("brief"),
      name: "",
      mode: null,
      content: "",
      filesJson: serializeSources({ notes: "", docs: [] }),
      createdAt: now,
      updatedAt: now,
    });
  };

  return (
    <div>
      <div className="flex items-center gap-2">
        <h3 className="flex-1 text-[14px] font-semibold">Briefs</h3>
        <Button size="sm" variant="outline" onClick={startNew}>
          <Plus /> New brief
        </Button>
      </div>
      {editing ? (
        <BriefEditor
          key={editing.id}
          brief={editing}
          onCancel={() => setEditing(null)}
          onSaved={async () => {
            setEditing(null);
            await onChange();
          }}
        />
      ) : null}
      {briefs.length === 0 && !editing ? (
        <p className="mt-3 rounded-xl border border-dashed border-border px-4 py-6 text-center text-[13.5px] text-muted-foreground">
          No briefs yet. Add your résumé and the job description before an interview, or an agenda and slides before a meeting.
        </p>
      ) : (
        <ul className="mt-3 divide-y divide-border rounded-xl border border-border bg-card">
          {briefs.map((b) => {
            const names = sourceNames(b);
            return (
              <li key={b.id} className="flex items-center gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[14px] font-medium">{b.name}</p>
                  <p className="truncate text-[12.5px] text-muted-foreground">
                    {names.length ? `${names.length} file${names.length === 1 ? "" : "s"}: ${names.join(", ")}` : "Pasted text"}
                    {` · ~${approxTokens(b.content.length).toLocaleString()} tokens`}
                    {b.mode ? ` · default for ${MODE_META[b.mode].label}` : ""}
                  </p>
                </div>
                <Button size="sm" variant="ghost" onClick={() => setEditing(b)}>
                  Edit
                </Button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

interface PendingFile {
  key: string;
  name: string;
  state: "reading" | "error";
  error?: string;
}

const fmtSize = (bytes: number) =>
  bytes >= 1_048_576 ? `${(bytes / 1_048_576).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;

function BriefEditor({ brief, onCancel, onSaved }: { brief: BriefRow; onCancel: () => void; onSaved: () => Promise<void> }) {
  const initial = useMemo(() => parseSources(brief), [brief]);
  const [name, setName] = useState(brief.name);
  const [notes, setNotes] = useState(initial.notes);
  const [docs, setDocs] = useState<BriefDoc[]>(initial.docs);
  const [pending, setPending] = useState<PendingFile[]>([]);
  const [preview, setPreview] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const isNew = useMemo(() => !brief.name && !brief.content, [brief]);

  const composed = useMemo(() => composeBrief({ notes, docs }), [notes, docs]);
  const reading = pending.some((p) => p.state === "reading");

  // Files are read one after another (so the list keeps the order you picked them in),
  // each on its own: one bad file never stops the others.
  const addFiles = async (list: FileList | File[] | null) => {
    const files = Array.from(list ?? []);
    if (fileInput.current) fileInput.current.value = "";
    if (!files.length) return;
    if (!name.trim() && files.length === 1) setName(files[0].name.replace(/\.[^.]+$/, ""));
    const queued = files.map((f) => ({ key: newId("file"), file: f }));
    setPending((p) => [...p, ...queued.map(({ key, file }) => ({ key, name: file.name, state: "reading" as const }))]);
    for (const { key, file } of queued) {
      try {
        const r = await extractBriefText(file);
        setDocs((ds) => [
          // Re-adding the same file replaces the old copy.
          ...ds.filter((d) => !(d.name === r.name && d.size === r.size)),
          { id: newId("doc"), name: r.name, size: r.size, pages: r.pages, text: r.text },
        ]);
        setPending((p) => p.filter((x) => x.key !== key));
      } catch (e) {
        setPending((p) => p.map((x) => (x.key === key ? { ...x, state: "error", error: (e as Error).message } : x)));
      }
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      await getRepo().saveBrief({
        ...brief,
        name: name.trim(),
        content: composed.content,
        filesJson: serializeSources({ notes, docs }),
        updatedAt: Date.now(),
      });
      await onSaved();
    } finally {
      setSaving(false);
    }
  };

  const trimmedParts = composed.trimmed.docs.filter((n) => n > 0).length + (composed.trimmed.notes > 0 ? 1 : 0);

  return (
    <div
      className={cn("mt-3 rounded-xl border bg-card p-4 transition-colors", dragging ? "border-you bg-you/5" : "border-border")}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("Files")) return;
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        void addFiles(e.dataTransfer.files);
      }}
    >
      <div className="flex flex-col gap-4">
        <div>
          <label htmlFor="brief-name" className="text-[13px] font-medium">
            Name
          </label>
          <Input
            id="brief-name"
            value={name}
            placeholder="e.g. Résumé + Stripe platform JD"
            onChange={(e) => setName(e.target.value)}
            className="mt-1 h-10 text-[13.5px]"
          />
        </div>

        <div>
          <div className="flex items-center gap-2">
            <p className="flex-1 text-[13px] font-medium">Documents</p>
            <input
              ref={fileInput}
              type="file"
              accept={BRIEF_ACCEPT}
              multiple
              className="sr-only"
              id="brief-file"
              onChange={(e) => void addFiles(e.target.files)}
            />
            <Button size="sm" variant="outline" onClick={() => fileInput.current?.click()}>
              <FileUp /> Add files
            </Button>
          </div>
          <button
            type="button"
            onClick={() => fileInput.current?.click()}
            className="mt-2 flex w-full flex-col items-center justify-center rounded-lg border border-dashed border-border px-4 py-4 text-center text-[13px] text-muted-foreground transition-colors hover:border-foreground/30 hover:text-foreground"
          >
            <span className="font-medium text-foreground">Drop PDFs here or click to choose</span>
            <span className="mt-0.5 text-[12.5px]">
              Several at once is fine: résumé, job description, slides, notes. PDF, DOCX, TXT or MD.
            </span>
          </button>

          {docs.length || pending.length ? (
            <ul className="mt-2 divide-y divide-border rounded-lg border border-border">
              {docs.map((d, i) => {
                const cut = composed.trimmed.docs[i];
                return (
                  <li key={d.id} className="px-3 py-2">
                    <div className="flex items-center gap-2.5">
                      <FileText className="size-4 shrink-0 text-muted-foreground" />
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-[13.5px] font-medium">{d.name}</p>
                        <p className="text-[12px] text-muted-foreground">
                          {d.pages ? `${d.pages} page${d.pages === 1 ? "" : "s"} · ` : ""}
                          {fmtSize(d.size)} · ~{approxTokens(d.text.length).toLocaleString()} tokens
                          {cut ? (
                            <span className="text-warn"> · last {Math.max(1, Math.round((cut / d.text.length) * 100))}% not included</span>
                          ) : null}
                        </p>
                      </div>
                      <Button size="sm" variant="ghost" onClick={() => setPreview(preview === d.id ? null : d.id)}>
                        {preview === d.id ? "Hide text" : "View text"}
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        className="size-8"
                        aria-label={`Remove ${d.name}`}
                        title={`Remove ${d.name}`}
                        onClick={() => setDocs((ds) => ds.filter((x) => x.id !== d.id))}
                      >
                        <X />
                      </Button>
                    </div>
                    {preview === d.id ? (
                      <pre className="mt-2 max-h-[220px] overflow-auto whitespace-pre-wrap rounded-md bg-raised p-2.5 font-sans text-[12.5px] leading-relaxed text-muted-foreground">
                        {d.text.length > 6000
                          ? `${d.text.slice(0, 6000)}\n\n… (${(d.text.length - 6000).toLocaleString()} more characters)`
                          : d.text}
                      </pre>
                    ) : null}
                  </li>
                );
              })}
              {pending.map((p) => (
                <li key={p.key} className="flex items-center gap-2.5 px-3 py-2">
                  {p.state === "reading" ? (
                    <Loader2 className="size-4 shrink-0 animate-spin text-muted-foreground" />
                  ) : (
                    <AlertCircle className="size-4 shrink-0 text-danger" />
                  )}
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-[13.5px] font-medium">{p.name}</p>
                    <p className={cn("text-[12px]", p.state === "error" ? "text-danger" : "text-muted-foreground")}>
                      {p.state === "reading" ? "Reading…" : p.error}
                    </p>
                  </div>
                  {p.state === "error" ? (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="size-8"
                      aria-label={`Dismiss ${p.name}`}
                      onClick={() => setPending((xs) => xs.filter((x) => x.key !== p.key))}
                    >
                      <X />
                    </Button>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
        </div>

        <div>
          <label htmlFor="brief-content" className="text-[13px] font-medium">
            Notes <span className="font-normal text-muted-foreground">(optional)</span>
          </label>
          <Textarea
            id="brief-content"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            placeholder="Paste anything else Claude should know: talking points, the company, who you're meeting."
            className="mt-1 max-h-[260px] min-h-[96px] text-[13px] leading-relaxed"
          />
        </div>

        <div className="rounded-lg bg-raised px-3 py-2 text-[12.5px] text-muted-foreground">
          {docs.length} document{docs.length === 1 ? "" : "s"}
          {notes.trim() ? " + notes" : ""} · ~{approxTokens(Math.min(composed.totalChars, BRIEF_MAX_CHARS)).toLocaleString()} tokens
          sent with each answer
          {trimmedParts ? (
            <span className="text-warn">
              {" "}
              · over the ~{approxTokens(BRIEF_MAX_CHARS).toLocaleString()}-token limit, so the longest{" "}
              {trimmedParts === 1 ? "part is" : `${trimmedParts} parts are`} shortened. Every document still gets a fair share.
            </span>
          ) : null}
        </div>

        <div className="flex items-center gap-2">
          <Button size="sm" disabled={!name.trim() || !composed.content.trim() || reading || saving} onClick={() => void save()}>
            {reading ? "Reading files…" : "Save brief"}
          </Button>
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <span className="flex-1" />
          {!isNew ? (
            confirmDelete ? (
              <Button
                size="sm"
                variant="destructive"
                onClick={async () => {
                  await getRepo().deleteBrief(brief.id);
                  await onSaved();
                }}
              >
                Delete “{brief.name}”
              </Button>
            ) : (
              <Button size="sm" variant="ghost" className="text-danger" onClick={() => setConfirmDelete(true)}>
                <Trash2 /> Delete
              </Button>
            )
          ) : null}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------

function DataSection() {
  const [count, setCount] = useState<number | null>(null);
  const [exporting, setExporting] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [version, setVersion] = useState("");

  const refresh = () =>
    void getRepo()
      .listSessions({ limit: 100000 })
      .then((l) => setCount(l.length))
      .catch(() => setCount(null));
  useEffect(() => {
    refresh();
    void native.appVersion().then(setVersion).catch(() => {});
  }, []);

  const exportAll = async () => {
    setExporting(true);
    try {
      const repo = getRepo();
      const list = await repo.listSessions({ limit: 100000 });
      const parts: string[] = [];
      for (const s of list) {
        const d = await repo.getSessionDetail(s.id);
        if (d) parts.push(sessionToMarkdown(d));
      }
      downloadText(`pluely-x-sessions-${new Date().toISOString().slice(0, 10)}.md`, parts.join("\n\n---\n\n"));
    } finally {
      setExporting(false);
    }
  };

  return (
    <Section id="data" title="Data" description="Everything is stored on this PC in the app's data folder. Nothing is uploaded except the requests sent to Claude and Deepgram.">
      <Row label="Export everything" description={count === null ? "All sessions as one Markdown file." : `${count} sessions as one Markdown file.`}>
        <Button variant="outline" size="sm" onClick={() => void exportAll()} disabled={exporting || !count}>
          {exporting ? <Loader2 className="animate-spin" /> : null}
          Export as Markdown
        </Button>
      </Row>
      <Row label="Delete all sessions" description="Transcripts, answers, notes and summaries. Briefs and settings stay. This can't be undone.">
        {confirm ? (
          <>
            <Button variant="ghost" size="sm" onClick={() => setConfirm(false)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              size="sm"
              onClick={async () => {
                await getRepo().deleteAll();
                setConfirm(false);
                refresh();
              }}
            >
              Delete {count ?? ""} sessions
            </Button>
          </>
        ) : (
          <Button variant="outline" size="sm" className="text-danger" onClick={() => setConfirm(true)} disabled={!count}>
            <Trash2 /> Delete all
          </Button>
        )}
      </Row>
      <Row label="Logs" description={`Pluely X ${version}. Logs never contain keys or transcripts.`}>
        <Button variant="outline" size="sm" onClick={() => void native.appOpenLogs().catch(() => {})}>
          Open logs folder
        </Button>
      </Row>
    </Section>
  );
}
