import { AlertTriangle, CheckCircle2, Loader2, X } from "lucide-react";
import { useEffect } from "react";
import { IconButton } from "@/components/common";
import { describeProblem } from "@/core/live/audioCheck";
import { dismissAudioCheck, runAudioCheckNow, useLive } from "@/core/live/session";

/** Result of the check that runs every time listening starts, plus the mid-session warning. */
export function AudioCheckBanner() {
  const check = useLive((s) => s.audioCheck);

  // A passing check confirms itself briefly, then gets out of the way.
  useEffect(() => {
    if (check.phase !== "ok") return;
    const id = setTimeout(dismissAudioCheck, 4000);
    return () => clearTimeout(id);
  }, [check]);

  if (check.phase === "idle") return null;

  if (check.phase === "checking") {
    return (
      <div role="status" className="flex shrink-0 items-center gap-2 border-b border-white/[0.07] bg-you/10 px-3 py-1.5 text-[12.5px] text-foreground">
        <Loader2 className="size-3.5 animate-spin text-you" />
        <span>Checking that computer audio reaches Pluely X…</span>
      </div>
    );
  }

  if (check.phase === "ok") {
    return (
      <div role="status" className="flex shrink-0 items-center gap-2 border-b border-white/[0.07] bg-them/10 px-3 py-1.5 text-[12.5px] text-foreground">
        <CheckCircle2 className="size-3.5 text-them" />
        <span>
          Computer audio OK
          {check.how === "test_tone" ? " (test beep heard)" : " (call audio heard)"} — ready for the interviewer.
        </span>
      </div>
    );
  }

  const { title, detail } = describeProblem(check.kind);
  return (
    <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-white/[0.07] bg-danger/[0.12] px-3 py-2 text-[13px]">
      <AlertTriangle className="mt-0.5 size-4 shrink-0 text-danger" />
      <div className="min-w-0 flex-1">
        <p className="font-medium text-foreground">{title}</p>
        <p className="text-[12.5px] text-muted-foreground">{detail}</p>
      </div>
      {check.kind !== "not_capturing" ? (
        <button
          type="button"
          onClick={() => void runAudioCheckNow()}
          className="shrink-0 rounded-md bg-primary px-2 py-0.5 text-[12.5px] font-medium text-primary-foreground"
        >
          Re-run check
        </button>
      ) : null}
      <IconButton size="sm" label="Dismiss" onClick={dismissAudioCheck}>
        <X />
      </IconButton>
    </div>
  );
}
