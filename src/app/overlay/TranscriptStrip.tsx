import { ChevronDown, ChevronUp, Loader2, RefreshCw } from "lucide-react";
import { useMemo } from "react";
import { IconButton } from "@/components/common";
import type { LectureNotes, MeetingNotes } from "@/core/ai/schemas";
import { MODE_META } from "@/core/defaults";
import { refreshNotes, useLive } from "@/core/live/session";
import { segments, speakerBlocks } from "@/core/live/transcript";
import { useSettings } from "@/core/settings";
import { cn } from "@/lib/utils";
import { useStickToBottom } from "../shared/useStickToBottom";
import { useOverlay } from "./store";

export function TranscriptStrip() {
  const transcript = useLive((s) => s.transcript);
  const status = useLive((s) => s.status);
  const sources = useLive((s) => s.sources);
  const mode = useSettings((s) => s.settings.mode);
  const open = useOverlay((s) => s.transcriptOpen);
  const tab = useOverlay((s) => s.panelTab);
  const hasNotes = MODE_META[mode].rollingNotes;
  const activeTab = hasNotes ? tab : "transcript";

  const blocks = useMemo(() => speakerBlocks(segments(transcript)), [transcript]);
  const lastBlock = blocks[blocks.length - 1];
  const version = `${blocks.length}:${lastBlock?.parts.length}:${lastBlock?.parts[lastBlock.parts.length - 1]?.text.length}:${open}`;
  const { ref, pinned, scrollToBottom } = useStickToBottom<HTMLDivElement>(version);

  if (status === "idle" && blocks.length === 0) return null;

  const reconnecting = sources.them.state === "reconnecting" || sources.me.state === "reconnecting";
  const connecting = status === "starting" || sources.them.state === "connecting";

  return (
    <section aria-label="Live transcript" className="shrink-0 border-b border-white/[0.07]">
      <div className="flex h-8 items-center gap-1 px-2.5">
        <div role="tablist" aria-label="Transcript or notes" className="flex items-center gap-0.5">
          <StripTab
            selected={activeTab === "transcript"}
            onClick={() => useOverlay.setState({ panelTab: "transcript", transcriptOpen: true })}
          >
            Transcript
          </StripTab>
          {hasNotes && (
            <StripTab
              selected={activeTab === "notes"}
              onClick={() => useOverlay.setState({ panelTab: "notes", transcriptOpen: true })}
            >
              Notes
            </StripTab>
          )}
        </div>
        <span className="ml-1.5 min-w-0 flex-1 truncate text-[12px] text-subtle" aria-live="polite">
          {reconnecting
            ? "Reconnecting to speech service…"
            : connecting
              ? "Connecting…"
              : status === "idle"
                ? "Stopped"
                : null}
        </span>
        <IconButton
          size="sm"
          label={open ? "Collapse to two lines" : "Expand transcript"}
          onClick={() => useOverlay.setState({ transcriptOpen: !open })}
        >
          {open ? <ChevronUp /> : <ChevronDown />}
        </IconButton>
      </div>

      {activeTab === "notes" && open ? (
        <NotesView kind={mode === "lecture" ? "lecture" : "meeting"} />
      ) : (
        <div className="relative">
          <div
            ref={ref}
            className={cn(
              "px-3 pb-2 text-[13.5px] leading-[20px] [overflow-anchor:none]",
              open ? "max-h-[132px] overflow-y-auto" : "h-[48px] overflow-hidden",
            )}
            aria-live="off"
          >
            {blocks.length === 0 ? (
              <p className="py-1 text-subtle">{connecting ? "Getting the microphone and speakers ready…" : "Waiting for speech…"}</p>
            ) : (
              blocks.map((b) => (
                <p key={b.key} className="flex gap-2 py-[1px]">
                  <span
                    className={cn(
                      "w-9 shrink-0 pt-px text-[12px] font-semibold uppercase tracking-wide",
                      b.source === "them" ? "text-them" : "text-you",
                    )}
                  >
                    {b.source === "them" ? "Them" : "You"}
                  </span>
                  <span className="min-w-0 flex-1 text-foreground/90">
                    {b.parts.map((p, i) => (
                      <span key={p.segId} className={p.isFinal ? undefined : "interim"}>
                        {i > 0 ? " " : ""}
                        {p.text}
                      </span>
                    ))}
                  </span>
                </p>
              ))
            )}
          </div>
          {open && !pinned && (
            <button
              type="button"
              onClick={scrollToBottom}
              className="absolute bottom-1.5 right-3 rounded-full border border-white/10 bg-popover px-2 py-0.5 text-[12px] text-muted-foreground shadow hover:text-foreground"
            >
              Latest ↓
            </button>
          )}
        </div>
      )}
    </section>
  );
}

function StripTab({ selected, onClick, children }: { selected: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={selected}
      onClick={onClick}
      className={cn(
        "rounded-md px-1.5 py-0.5 text-[12.5px] font-medium transition-colors",
        selected ? "text-foreground" : "text-subtle hover:text-muted-foreground",
      )}
    >
      {children}
    </button>
  );
}

function ago(ts: number | null): string {
  if (!ts) return "";
  const m = Math.round((Date.now() - ts) / 60000);
  return m < 1 ? "just now" : `${m} min ago`;
}

function NotesView({ kind }: { kind: "meeting" | "lecture" }) {
  const notes = useLive((s) => s.notes);
  const busy = useLive((s) => s.notesBusy);
  const updatedAt = useLive((s) => s.notesUpdatedAt);
  const hasTranscript = useLive((s) => s.transcript.order.length > 0);

  return (
    <div className="max-h-[156px] overflow-y-auto px-3 pb-2.5 text-[13.5px] leading-[20px]">
      <div className="mb-1 flex items-center gap-2 text-[12px] text-subtle">
        <span className="flex-1">
          {busy ? "Updating notes…" : updatedAt ? `Updated ${ago(updatedAt)}` : "Notes refresh every few minutes while listening"}
        </span>
        <button
          type="button"
          disabled={busy || !hasTranscript}
          onClick={() => void refreshNotes()}
          className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[12px] text-muted-foreground hover:bg-foreground/[0.08] hover:text-foreground disabled:opacity-40"
        >
          {busy ? <Loader2 className="size-3 animate-spin" /> : <RefreshCw className="size-3" />}
          Update now
        </button>
      </div>
      {!notes ? (
        <p className="text-subtle">
          {kind === "meeting" ? "Decisions and action items will collect here." : "Key points and terms will collect here."}
        </p>
      ) : kind === "meeting" ? (
        <MeetingNotesView n={notes as MeetingNotes} />
      ) : (
        <LectureNotesView n={notes as LectureNotes} />
      )}
    </div>
  );
}

function NoteSection({ title, items }: { title: string; items: React.ReactNode[] }) {
  if (!items.length) return null;
  return (
    <div className="mb-1.5">
      <h4 className="text-[12px] font-semibold text-muted-foreground">{title}</h4>
      <ul className="ml-4 list-disc marker:text-subtle">
        {items.map((it, i) => (
          <li key={i}>{it}</li>
        ))}
      </ul>
    </div>
  );
}

function MeetingNotesView({ n }: { n: MeetingNotes }) {
  return (
    <>
      <NoteSection title="Decisions" items={n.decisions} />
      <NoteSection
        title="Action items"
        items={n.action_items.map((a) => (
          <>
            {a.task}
            {a.owner && <span className="text-muted-foreground"> — {a.owner}</span>}
            {a.due && <span className="text-subtle"> · {a.due}</span>}
          </>
        ))}
      />
      <NoteSection title="Open questions" items={n.open_questions} />
    </>
  );
}

function LectureNotesView({ n }: { n: LectureNotes }) {
  return (
    <>
      <NoteSection title="Key points" items={n.key_points} />
      <NoteSection
        title="Terms"
        items={n.terms.map((t) => (
          <>
            <span className="font-medium">{t.term}</span>
            <span className="text-muted-foreground"> — {t.definition}</span>
          </>
        ))}
      />
    </>
  );
}
