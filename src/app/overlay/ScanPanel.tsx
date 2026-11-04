import { CircleAlert, CircleCheck, CircleMinus, CircleX, Loader2, RotateCw, Wrench, X } from "lucide-react";
import { IconButton } from "@/components/common";
import { scanVerdict, type ScanStatus } from "@/core/live/quickScan";
import { closeQuickScan, listenOnCallDevice, runQuickScanNow, useLive } from "@/core/live/session";
import { cn } from "@/lib/utils";
import { openDashboard } from "./controller";

function StatusIcon({ status }: { status: ScanStatus }) {
  const cls = "mt-0.5 size-3.5 shrink-0";
  switch (status) {
    case "running":
      return <Loader2 className={cn(cls, "animate-spin text-you")} />;
    case "ok":
      return <CircleCheck className={cn(cls, "text-ok")} />;
    case "fixed":
      return <Wrench className={cn(cls, "text-ok")} />;
    case "warn":
      return <CircleAlert className={cn(cls, "text-warn")} />;
    case "fail":
      return <CircleX className={cn(cls, "text-danger")} />;
    default:
      return <CircleMinus className={cn(cls, "text-subtle")} />;
  }
}

/** Results of "Quick scan & fix", shown under the top bar while it runs and until closed. */
export function ScanPanel() {
  const scan = useLive((s) => s.scan);
  if (!scan.running && !scan.steps.length) return null;

  const verdict = scan.running ? null : scanVerdict(scan.steps);
  const needsSettings = !scan.running && scan.steps.some((s) => s.action === "settings" && (s.status === "fail" || s.status === "warn"));
  const audioUntested = scan.steps.some((s) => s.id === "audio" && s.status === "skipped");
  const title = scan.running
    ? "Scanning and fixing…"
    : verdict === "ok"
      ? audioUntested
        ? "Setup looks good — start listening to test the audio"
        : "All good — ready to answer"
      : verdict === "warn"
        ? "Working, with warnings"
        : "Found problems";

  return (
    <section
      aria-label="Quick scan and fix"
      aria-live="polite"
      className={cn(
        "max-h-[45%] shrink-0 overflow-y-auto border-b border-white/[0.07] px-3 py-2 text-[12.5px]",
        verdict === "fail" ? "bg-danger/[0.08]" : verdict === "warn" ? "bg-warn/[0.07]" : "bg-foreground/[0.04]",
      )}
    >
      <div className="mb-1.5 flex items-center gap-2">
        <p className="flex-1 text-[13px] font-medium text-foreground">{title}</p>
        {needsSettings ? (
          <button
            type="button"
            onClick={() => openDashboard("/dashboard/settings#keys")}
            className="shrink-0 rounded-md bg-primary px-2 py-0.5 text-[12.5px] font-medium text-primary-foreground"
          >
            Open settings
          </button>
        ) : null}
        {!scan.running ? (
          <>
            <IconButton size="sm" label="Run the scan again" onClick={() => void runQuickScanNow()}>
              <RotateCw />
            </IconButton>
            <IconButton size="sm" label="Close" onClick={closeQuickScan}>
              <X />
            </IconButton>
          </>
        ) : null}
      </div>
      <ul className="space-y-1">
        {scan.steps.map((s) => (
          <li key={s.id} className="flex items-start gap-2">
            <StatusIcon status={s.status} />
            <div className="min-w-0 flex-1">
              <span className={cn(s.status === "pending" || s.status === "skipped" ? "text-subtle" : "text-foreground")}>
                {s.label}
              </span>
              {s.detail && s.status !== "ok" ? <p className="text-muted-foreground">{s.detail}</p> : null}
              {s.action === "use_call_device" && s.devices && s.status === "warn" ? (
                <button
                  type="button"
                  onClick={() => void listenOnCallDevice(s.devices!)}
                  className="mt-1 rounded-md bg-primary px-2 py-0.5 text-[12.5px] font-medium text-primary-foreground"
                >
                  Listen to {s.devices.call}
                </button>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
