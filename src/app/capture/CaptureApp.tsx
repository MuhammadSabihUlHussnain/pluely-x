// Region selector shown on every monitor (window `capture-overlay-<n>`,
// route /capture?monitor=<name>). Ported from the old components/Overlay.tsx
// selection logic; the rect is sent in CSS px of this window and Rust converts
// it with the monitor's own scale factor (no devicePixelRatio math here).

import { useCallback, useEffect, useRef, useState } from "react";
import { native } from "@/core/native";

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

const MIN_SIZE = 10;

export function CaptureApp() {
  const monitor = new URLSearchParams(window.location.search).get("monitor") ?? "";
  const start = useRef<{ x: number; y: number } | null>(null);
  const [rect, setRect] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);

  const cancel = useCallback(() => {
    start.current = null;
    setRect(null);
    void native.screenshotRegionCancel().catch(() => {});
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [cancel]);

  const toRect = (a: { x: number; y: number }, b: { x: number; y: number }): Rect => ({
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(a.x - b.x),
    height: Math.abs(a.y - b.y),
  });

  return (
    <div
      className="fixed inset-0 select-none"
      style={{ cursor: "crosshair", background: rect ? "transparent" : "oklch(0.15 0.01 255 / 0.32)" }}
      onMouseDown={(e) => {
        if (e.button !== 0 || busy) return;
        start.current = { x: e.clientX, y: e.clientY };
        setRect({ x: e.clientX, y: e.clientY, width: 0, height: 0 });
      }}
      onMouseMove={(e) => {
        if (!start.current) return;
        setRect(toRect(start.current, { x: e.clientX, y: e.clientY }));
      }}
      onMouseUp={(e) => {
        if (!start.current) return;
        const r = toRect(start.current, { x: e.clientX, y: e.clientY });
        start.current = null;
        if (r.width < MIN_SIZE || r.height < MIN_SIZE) {
          setRect(null);
          return;
        }
        setBusy(true);
        void native
          .screenshotRegionCommit(monitor, {
            x: Math.round(r.x),
            y: Math.round(r.y),
            width: Math.round(r.width),
            height: Math.round(r.height),
          })
          .catch(() => setBusy(false));
      }}
    >
      {rect ? (
        <div
          className="pointer-events-none absolute rounded-[3px] border-2 border-white outline outline-1 outline-black/60"
          style={{
            left: rect.x,
            top: rect.y,
            width: rect.width,
            height: rect.height,
            boxShadow: "0 0 0 9999px oklch(0.15 0.01 255 / 0.42)",
          }}
        >
          {rect.width > 60 && rect.height > 24 ? (
            <span className="absolute -top-7 left-0 rounded-md bg-black/75 px-1.5 py-0.5 font-mono text-[12px] text-white">
              {Math.round(rect.width)} × {Math.round(rect.height)}
            </span>
          ) : null}
        </div>
      ) : null}

      <div className="pointer-events-none fixed left-1/2 top-5 -translate-x-1/2 rounded-xl bg-black/75 px-4 py-2 text-[14px] text-white shadow-2xl">
        {busy ? "Capturing…" : "Drag to select the area to ask about · Esc to cancel"}
      </div>
      <button
        type="button"
        onMouseDown={(e) => e.stopPropagation()}
        onClick={cancel}
        className="fixed right-5 top-5 rounded-lg bg-black/75 px-3.5 py-2 text-[14px] font-medium text-white shadow-2xl hover:bg-black/90"
      >
        Cancel
      </button>
    </div>
  );
}
