import { Check, Headphones, Info, Loader2, Mic } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Kbd } from "@/components/common";
import { Button } from "@/components/ui/button";
import { SHORTCUT_LABELS } from "@/core/defaults";
import { native } from "@/core/native";
import { useKeys, useSettings } from "@/core/settings";
import type { ShortcutAction } from "@/core/types";
import { cn } from "@/lib/utils";
import { DeviceSelect } from "./DeviceSelect";
import { BigMeter, ExternalLinkButton, KeyEntry, LiveWords, useAudioTest } from "./parts";
import { KEY_STORE } from "@/core/platform";

const STEPS = ["Claude", "Listening", "Audio check", "Shortcuts"] as const;

export function Onboarding() {
  const [step, setStep] = useState(0);
  const keys = useKeys((s) => s.status);
  const refresh = useKeys((s) => s.refresh);
  const navigate = useNavigate();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const finish = async () => {
    await useSettings.getState().update({ onboarded: true }).catch(() => {});
    navigate("/dashboard/sessions", { replace: true });
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex min-h-full max-w-[600px] flex-col px-8 py-10">
        <div className="mb-8 flex items-center gap-2.5">
          <span className="flex size-8 items-center justify-center rounded-lg bg-primary text-[15px] font-bold text-primary-foreground">
            X
          </span>
          <span className="text-[15px] font-semibold">Set up Pluely X</span>
          <span className="ml-auto text-[13px] text-muted-foreground">About a minute</span>
        </div>

        <ol className="mb-8 grid grid-cols-4 gap-2" aria-label="Setup steps">
          {STEPS.map((s, i) => (
            <li key={s}>
              <button
                type="button"
                onClick={() => i < step && setStep(i)}
                disabled={i > step}
                aria-current={i === step ? "step" : undefined}
                className="w-full text-left disabled:cursor-default"
              >
                <span
                  className={cn(
                    "block h-1 rounded-full transition-colors",
                    i < step ? "bg-ok" : i === step ? "bg-foreground" : "bg-border",
                  )}
                />
                <span
                  className={cn(
                    "mt-2 block text-[12.5px] font-medium",
                    i === step ? "text-foreground" : "text-muted-foreground",
                  )}
                >
                  {i + 1}. {s}
                </span>
              </button>
            </li>
          ))}
        </ol>

        <div className="flex-1">
          {step === 0 && (
            <StepShell
              title="Connect Claude"
              lead={`Pluely X answers with Claude Opus 5.5. Paste an Anthropic API key — it's tested live and stored in ${KEY_STORE}, never in a file.`}
            >
              {keys?.anthropic.present ? (
                <SavedKey hint={keys.anthropic.hint} />
              ) : (
                <KeyEntry provider="anthropic" autoFocus />
              )}
              <p className="text-[13.5px] text-muted-foreground">
                No key yet? <ExternalLinkButton href="https://console.anthropic.com/settings/keys">Create one in the Anthropic Console</ExternalLinkButton>
              </p>
            </StepShell>
          )}

          {step === 1 && (
            <StepShell
              title="Add listening (optional)"
              lead="Live transcription of both sides of a call uses Deepgram. New accounts get $200 of free credit — hundreds of hours of listening. You can skip this and still ask about your screen."
            >
              {keys?.deepgram.present ? <SavedKey hint={keys.deepgram.hint} /> : <KeyEntry provider="deepgram" autoFocus />}
              <p className="text-[13.5px] text-muted-foreground">
                <ExternalLinkButton href="https://console.deepgram.com/signup">Create a free Deepgram account</ExternalLinkButton>
                , then copy an API key from its console.
              </p>
            </StepShell>
          )}

          {step === 2 && <AudioStep hasDeepgram={!!keys?.deepgram.present} />}

          {step === 3 && <ShortcutsStep />}
        </div>

        <div className="mt-10 flex items-center gap-2 border-t border-border pt-5">
          {step > 0 ? (
            <Button variant="ghost" onClick={() => setStep(step - 1)}>
              Back
            </Button>
          ) : null}
          <span className="flex-1" />
          {step === 1 && !keys?.deepgram.present ? (
            <Button variant="ghost" onClick={() => setStep(2)}>
              Skip for now
            </Button>
          ) : null}
          {step < 3 ? (
            <Button onClick={() => setStep(step + 1)} disabled={step === 0 && !keys?.anthropic.present}>
              Continue
            </Button>
          ) : (
            <Button onClick={() => void finish()}>Finish</Button>
          )}
        </div>
      </div>
    </div>
  );
}

function StepShell({ title, lead, children }: { title: string; lead: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-5">
      <div>
        <h1 className="text-[22px] font-semibold tracking-[-0.015em]">{title}</h1>
        <p className="mt-2 max-w-[60ch] text-[14px] leading-relaxed text-muted-foreground">{lead}</p>
      </div>
      {children}
    </div>
  );
}

function SavedKey({ hint }: { hint?: string }) {
  return (
    <div className="flex items-center gap-3 rounded-xl border border-ok/30 bg-ok/[0.08] px-4 py-3">
      <span className="flex size-7 items-center justify-center rounded-full bg-ok text-background">
        <Check className="size-4" strokeWidth={3} />
      </span>
      <div>
        <p className="text-[14px] font-medium">Key saved and working</p>
        <p className="text-[13px] text-muted-foreground">Stored in {KEY_STORE} · ending {hint ?? "…"}</p>
      </div>
    </div>
  );
}

function AudioStep({ hasDeepgram }: { hasDeepgram: boolean }) {
  const audio = useSettings((s) => s.settings.audio);
  const update = useSettings((s) => s.update);
  const test = useAudioTest();
  const running = test.state === "running";

  return (
    <StepShell
      title="Check your audio"
      lead="Pick your microphone and the speakers or headphones your calls play through. Then run a short test: the meters should move and your words should appear."
    >
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-4">
          <label htmlFor="ob-mic" className="flex items-center gap-2 text-[14px] font-medium">
            <Mic className="size-4 text-you" /> Your microphone
          </label>
          <DeviceSelect id="ob-mic" kind="input" value={audio.micDeviceId} onChange={(v) => void update({ audio: { micDeviceId: v } })} />
        </div>
        <div className="flex items-center justify-between gap-4">
          <label htmlFor="ob-out" className="flex items-center gap-2 text-[14px] font-medium">
            <Headphones className="size-4 text-them" /> Call audio (speakers)
          </label>
          <DeviceSelect id="ob-out" kind="output" value={audio.outputDeviceId} onChange={(v) => void update({ audio: { outputDeviceId: v } })} />
        </div>
      </div>

      <div className="rounded-xl border border-border bg-card p-4">
        <div className="mb-3 flex items-center gap-2">
          <p className="flex-1 text-[14px] font-medium">Audio test</p>
          {hasDeepgram ? (
            <Button
              size="sm"
              variant={running ? "outline" : "default"}
              onClick={() => void (running ? test.stop() : test.start())}
              disabled={test.state === "starting" || test.state === "stopping"}
            >
              {test.state === "starting" ? <Loader2 className="animate-spin" /> : null}
              {running ? "Stop test" : test.state === "starting" ? "Starting…" : "Start test"}
            </Button>
          ) : null}
        </div>
        {hasDeepgram ? (
          <div className="flex flex-col gap-2.5">
            <BigMeter label="You" level={test.levels.me} tone="you" />
            <BigMeter label="Them" level={test.levels.them} tone="them" />
            {(test.devices.me || test.devices.them) && (
              <p className="text-[12.5px] text-muted-foreground">
                {test.devices.me ? `Mic: ${test.devices.me.name}` : ""}
                {test.devices.me && test.devices.them ? " · " : ""}
                {test.devices.them ? `Speakers: ${test.devices.them.name}` : ""}
              </p>
            )}
            <LiveWords transcript={test.transcript} />
            {test.error ? <p className="text-[13px] text-danger">{test.error}</p> : null}
          </div>
        ) : (
          <p className="text-[13.5px] text-muted-foreground">
            The live test needs a Deepgram key. You can add one later in Settings → Keys.
          </p>
        )}
      </div>

      <div className="flex flex-col gap-2 text-[13.5px] text-muted-foreground">
        <p className="flex gap-2">
          <Headphones className="mt-0.5 size-4 shrink-0" />
          Headphones are recommended: they stop the other side's voice from leaking into your microphone.
        </p>
        <p className="flex gap-2">
          <Info className="mt-0.5 size-4 shrink-0" />
          Recording others may require their consent. Check the rules where you are, and tell people when you transcribe.
        </p>
      </div>
    </StepShell>
  );
}

const ONBOARD_ACTIONS: ShortcutAction[] = [
  "toggle_overlay",
  "toggle_listen",
  "ask_now",
  "screenshot_full",
  "screenshot_region",
  "focus_input",
  "toggle_click_through",
];

function ShortcutsStep() {
  const sc = useSettings((s) => s.settings.shortcuts);
  return (
    <StepShell
      title="You're set"
      lead="The overlay floats above every app and is hidden from screen sharing. These keys work from anywhere — change them any time in Settings."
    >
      <div className="divide-y divide-border rounded-xl border border-border bg-card">
        {ONBOARD_ACTIONS.map((a) => (
          <div key={a} className="flex items-center gap-4 px-4 py-2.5">
            <div className="min-w-0 flex-1">
              <p className="text-[14px] font-medium">{SHORTCUT_LABELS[a].label}</p>
              <p className="text-[12.5px] text-muted-foreground">{SHORTCUT_LABELS[a].hint}</p>
            </div>
            <Kbd keys={sc[a].key} />
          </div>
        ))}
      </div>
      <Button variant="outline" className="self-start" onClick={() => void native.windowShow().catch(() => {})}>
        Show the overlay
      </Button>
    </StepShell>
  );
}
