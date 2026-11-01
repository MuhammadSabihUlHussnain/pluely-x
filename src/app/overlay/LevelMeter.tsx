import type { Level, Source } from "@/core/types";
import { cn } from "@/lib/utils";

const BARS = 5;

/** dBFS (-90..0) → 0..1, tuned so normal speech lands around 0.5–0.8. */
export function levelToUnit(level: Level | null): number {
  if (!level) return 0;
  const db = Math.max(level.rmsDb, level.peakDb - 12);
  return Math.min(1, Math.max(0, (db + 58) / 46));
}

export function LevelMeter({
  source,
  level,
  className,
}: {
  source: Source;
  level: Level | null;
  className?: string;
}) {
  const v = levelToUnit(level);
  return (
    <span
      className={cn("inline-flex h-3.5 items-end gap-[2px]", className)}
      role="meter"
      aria-label={source === "them" ? "Their audio level" : "Your microphone level"}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(v * 100)}
    >
      {Array.from({ length: BARS }, (_, i) => {
        const lit = v >= (i + 0.6) / BARS;
        return (
          <span
            key={i}
            className={cn(
              "w-[3px] rounded-[1px] transition-colors duration-100",
              lit ? (source === "them" ? "bg-them" : "bg-you") : "bg-foreground/15",
            )}
            style={{ height: `${36 + i * 16}%` }}
          />
        );
      })}
    </span>
  );
}
