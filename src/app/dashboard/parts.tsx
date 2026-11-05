// Shared dashboard building blocks: sections, rows, key management, audio test.

import { Check, ExternalLink, KeyRound, Loader2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { applyPatch, applyTranscript, emptyTranscript, segments, speakerBlocks } from "@/core/live/transcript";
import { describeSessionError } from "@/core/ai/errors";
import { native } from "@/core/native";
import { useKeys, useSettings } from "@/core/settings";
import type { DeviceInfo, KeyTestResult, Level, Provider, SessionEvent } from "@/core/types";
import { cn } from "@/lib/utils";
import { levelToUnit } from "../overlay/LevelMeter";
import { KEY_STORE, KEY_STORE_SHORT } from "@/core/platform";

export function Section({
  id,
  title,
  description,
  children,
}: {
  id: string;
  title: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className="scroll-mt-6 border-b border-border py-8 first:pt-2 last:border-b-0">
      <h2 className="text-[17px] font-semibold tracking-[-0.01em]">{title}</h2>
      {description ? <p className="mt-1 max-w-[62ch] text-[13.5px] text-muted-foreground">{description}</p> : null}
      <div className="mt-5 flex flex-col gap-5">{children}</div>
    </section>
  );
}

export function Row({
  label,
  description,
  htmlFor,
  children,
  className,
}: {
  label: React.ReactNode;
  description?: React.ReactNode;
  htmlFor?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex items-start justify-between gap-6", className)}>
      <div className="min-w-0 flex-1">
        <label htmlFor={htmlFor} className="text-[14px] font-medium text-foreground">
          {label}
        </label>
        {description ? <p className="mt-0.5 text-[13px] leading-snug text-muted-foreground">{description}</p> : null}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

export function ExternalLinkButton({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={() => void native.openUrl(href)}
      className="inline-flex items-center gap-1 text-[13.5px] font-medium text-you underline-offset-2 hover:underline"
    >
      {children}
      <ExternalLink className="size-3.5" />
    </button>
  );
}

// ---------------------------------------------------------------------------
// Keys

const PROVIDER_COPY: Record<Provider, { name: string; placeholder: string }> = {
  anthropic: { name: "Claude (Anthropic)", placeholder: "sk-ant-…" },
  deepgram: { name: "Deepgram (listening)", placeholder: "Paste your Deepgram API key" },
};

export function friendlyKeyError(provider: Provider, r: KeyTestResult): string {
  switch (r.code) {
    case "key_invalid":
      return provider === "anthropic"
        ? "That key was rejected. Check you copied all of it — it starts with sk-ant-."
        : "Deepgram rejected that key. Copy it again from the Deepgram console.";
    case "key_no_credit":
      return "The key works, but the account has no credit left.";
    case "network":
      return "Couldn't reach the service. Check your connection and try again.";
    case "keyring_error":
      return `${KEY_STORE} refused to store the key.`;
    case "key_missing":
      return "No key is saved yet.";
    case "rate_limited":
      return "The service is rate-limiting right now. Wait a few seconds and test again.";
    case "server":
      return "The service had a server error. Try again in a moment.";
    case "bad_request":
      return r.message || "The service rejected the test request.";
    default:
      return r.message || "The key test failed.";
  }
}

/** Paste → test with the candidate → save. Never displays a stored key. */
export function KeyEntry({
  provider,
  onSaved,
  autoFocus,
}: {
  provider: Provider;
  onSaved?: () => void;
  autoFocus?: boolean;
}) {
  const [value, setValue] = useState("");
  const [state, setState] = useState<"idle" | "testing" | "ok" | "error">("idle");
  const [msg, setMsg] = useState("");
  const refresh = useKeys((s) => s.refresh);

  const submit = async () => {
    const v = value.trim();
    if (!v) return;
    setState("testing");
    setMsg("");
    try {
      const r = await native.keysTest(provider, v);
      if (!r.ok) {
        setState("error");
        setMsg(friendlyKeyError(provider, r));
        return;
      }
      await native.keysSet(provider, v);
      setValue("");
      setState("ok");
      setMsg(`Connected · ${r.latencyMs} ms`);
      await refresh();
      onSaved?.();
    } catch (e) {
      setState("error");
      setMsg((e as { message?: string }).message ?? "Couldn't save the key.");
    }
  };

  return (
    <div>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Input
          type="password"
          autoComplete="off"
          spellCheck={false}
          autoFocus={autoFocus}
          aria-label={`${PROVIDER_COPY[provider].name} API key`}
          placeholder={PROVIDER_COPY[provider].placeholder}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            if (state === "error") setState("idle");
          }}
          className="h-10 flex-1 font-mono text-[13.5px]"
        />
        <Button type="submit" className="h-10" disabled={!value.trim() || state === "testing"}>
          {state === "testing" ? <Loader2 className="animate-spin" /> : null}
          {state === "testing" ? "Testing…" : "Test & save"}
        </Button>
      </form>
      <p
        aria-live="polite"
        className={cn(
          "mt-2 flex min-h-5 items-center gap-1.5 text-[13px]",
          state === "ok" ? "text-ok" : state === "error" ? "text-danger" : "text-muted-foreground",
        )}
      >
        {state === "ok" ? <Check className="size-4" /> : state === "error" ? <X className="size-4" /> : null}
        {msg}
      </p>
    </div>
  );
}

export function KeyStatusRow({ provider }: { provider: Provider }) {
  const status = useKeys((s) => s.status?.[provider]);
  const refresh = useKeys((s) => s.refresh);
  const [replacing, setReplacing] = useState(false);
  const [test, setTest] = useState<{ state: "idle" | "testing" | "done"; result?: KeyTestResult }>({ state: "idle" });
  const [confirmDelete, setConfirmDelete] = useState(false);

  const runTest = async () => {
    setTest({ state: "testing" });
    try {
      setTest({ state: "done", result: await native.keysTest(provider) });
    } catch (e) {
      setTest({ state: "done", result: { ok: false, message: (e as { message?: string }).message, latencyMs: 0 } });
    }
  };

  return (
    <div className="rounded-xl border border-border bg-card p-4">
      <div className="flex items-center gap-3">
        <KeyRound className="size-[18px] text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <p className="text-[14px] font-medium">{PROVIDER_COPY[provider].name}</p>
          <p className="text-[13px] text-muted-foreground">
            {status?.present ? (
              <span className="inline-flex items-center gap-1.5">
                <span className="size-1.5 rounded-full bg-ok" /> Saved in {KEY_STORE_SHORT} · ending {status.hint ?? "…"}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5">
                <span className="size-1.5 rounded-full bg-subtle" /> Not set
                {provider === "deepgram" ? " — listening is off" : " — answers are off"}
              </span>
            )}
          </p>
        </div>
        {status?.present && !replacing ? (
          <>
            <Button variant="outline" size="sm" onClick={() => void runTest()} disabled={test.state === "testing"}>
              {test.state === "testing" ? <Loader2 className="animate-spin" /> : null}
              Test
            </Button>
            <Button variant="outline" size="sm" onClick={() => setReplacing(true)}>
              Replace
            </Button>
            {confirmDelete ? (
              <Button
                variant="destructive"
                size="sm"
                onClick={async () => {
                  await native.keysDelete(provider).catch(() => {});
                  setConfirmDelete(false);
                  setTest({ state: "idle" });
                  await refresh();
                }}
              >
                Confirm delete
              </Button>
            ) : (
              <Button variant="ghost" size="sm" onClick={() => setConfirmDelete(true)}>
                Delete
              </Button>
            )}
          </>
        ) : null}
      </div>
      {test.state === "done" && test.result && !replacing ? (
        <p className={cn("mt-2 text-[13px]", test.result.ok ? "text-ok" : "text-danger")}>
          {test.result.ok ? `Working · ${test.result.latencyMs} ms` : friendlyKeyError(provider, test.result)}
        </p>
      ) : null}
      {!status?.present || replacing ? (
        <div className="mt-3">
          <KeyEntry provider={provider} autoFocus={replacing} onSaved={() => setReplacing(false)} />
          {replacing ? (
            <Button variant="ghost" size="sm" className="-mt-1" onClick={() => setReplacing(false)}>
              Cancel
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Audio test: a short real session with meters and live words.

export function useAudioTest() {
  const [state, setState] = useState<"idle" | "starting" | "running" | "stopping">("idle");
  const [levels, setLevels] = useState<{ me: Level | null; them: Level | null }>({ me: null, them: null });
  const [transcript, setTranscript] = useState(emptyTranscript());
  const [devices, setDevices] = useState<{ me?: DeviceInfo; them?: DeviceInfo }>({});
  const [error, setError] = useState<string | null>(null);
  const running = useRef(false);

  const onEvent = useCallback((ev: SessionEvent) => {
    if (ev.type === "levels") setLevels({ me: ev.me, them: ev.them });
    else if (ev.type === "transcript") setTranscript((t) => applyTranscript(t, ev));
    else if (ev.type === "transcriptPatch") setTranscript((t) => applyPatch(t, ev.segId, ev.echo));
    else if (ev.type === "device")
      setDevices((d) => ({ ...d, [ev.source]: { deviceId: ev.deviceId, name: ev.name, format: ev.format } }));
    else if (ev.type === "error" && ev.fatal) setError(describeSessionError(ev.code, ev.message).title);
  }, []);

  const stop = useCallback(async () => {
    if (!running.current) return;
    running.current = false;
    setState("stopping");
    await native.sessionStop().catch(() => {});
    setState("idle");
    setLevels({ me: null, them: null });
  }, []);

  const start = useCallback(async () => {
    if (running.current) return;
    setError(null);
    setTranscript(emptyTranscript());
    setState("starting");
    const s = useSettings.getState().settings;
    try {
      const info = await native.sessionStart(
        {
          me: { enabled: true, deviceId: s.audio.micDeviceId },
          them: { enabled: true, deviceId: s.audio.outputDeviceId },
          stt: {
            language: s.stt.language,
            keyterms: [],
            endpointingMs: s.stt.endpointingMs,
            utteranceEndMs: s.stt.utteranceEndMs,
            diarizeThem: false,
          },
          aec: s.audio.aec,
          echoDedup: true,
        },
        onEvent,
      );
      running.current = true;
      setDevices({ me: info.me, them: info.them });
      setState("running");
    } catch (e) {
      const err = e as { code?: string; message?: string };
      setError(
        err.code === "session_active"
          ? "Listening is running in the overlay — stop it there first."
          : describeSessionError(err.code ?? "", err.message ?? "").title,
      );
      setState("idle");
    }
  }, [onEvent]);

  useEffect(() => () => void (running.current && native.sessionStop().catch(() => {})), []);

  return { state, levels, transcript, devices, error, start, stop };
}

export function BigMeter({ label, level, tone }: { label: string; level: Level | null; tone: "them" | "you" }) {
  const v = levelToUnit(level);
  return (
    <div className="flex items-center gap-3">
      <span className={cn("w-12 text-[12.5px] font-semibold uppercase tracking-wide", tone === "them" ? "text-them" : "text-you")}>
        {label}
      </span>
      <div
        className="h-2 flex-1 overflow-hidden rounded-full bg-muted"
        role="meter"
        aria-label={`${label} level`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(v * 100)}
      >
        <div
          className={cn("h-full rounded-full", tone === "them" ? "bg-them" : "bg-you")}
          style={{ transform: `scaleX(${v})`, transformOrigin: "left center", transition: "transform 90ms linear" }}
        />
      </div>
    </div>
  );
}

export function LiveWords({ transcript }: { transcript: ReturnType<typeof emptyTranscript> }) {
  const blocks = speakerBlocks(segments(transcript));
  return (
    <div className="min-h-[84px] rounded-lg border border-border bg-muted/40 p-3 text-[13.5px] leading-relaxed">
      {blocks.length === 0 ? (
        <p className="text-muted-foreground">Say something — your words appear here.</p>
      ) : (
        blocks.slice(-4).map((b) => (
          <p key={b.key} className="flex gap-2">
            <span className={cn("w-10 shrink-0 text-[12px] font-semibold uppercase", b.source === "them" ? "text-them" : "text-you")}>
              {b.source === "them" ? "Them" : "You"}
            </span>
            <span>
              {b.parts.map((p) => (
                <span key={p.segId} className={p.isFinal ? undefined : "interim"}>
                  {p.text}{" "}
                </span>
              ))}
            </span>
          </p>
        ))
      )}
    </div>
  );
}
