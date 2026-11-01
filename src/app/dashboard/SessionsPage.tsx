import { Download, Image as ImageIcon, Search, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { CopyButton, InlineMd, Kbd } from "@/components/common";
import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import { parseAnswer } from "@/core/ai/answer";
import { getRepo, type CardRow, type SessionDetail, type SessionListItem } from "@/core/db";
import { downloadText, notesToMarkdown, parseSummary, safeFilename, sessionTitle, sessionToMarkdown, summaryToMarkdown } from "@/core/db/markdown";
import { MODE_META } from "@/core/defaults";
import { native } from "@/core/native";
import { useSettings } from "@/core/settings";
import { formatClock, formatDate, formatDuration } from "@/core/util";
import { cn } from "@/lib/utils";
import { ModeIcon } from "../shared/modes";

export function SessionsPage() {
  const { id } = useParams();
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [list, setList] = useState<SessionListItem[] | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const t = setTimeout(() => {
      void getRepo()
        .listSessions({ query: query.trim() || undefined })
        .then((l) => !cancelled && setList(l))
        .catch(() => !cancelled && setList([]));
    }, query ? 180 : 0);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [query, reloadKey]);

  // Pick up sessions the overlay saved while this window was in the background.
  useEffect(() => {
    const onFocus = () => setReloadKey((k) => k + 1);
    window.addEventListener("focus", onFocus);
    let un: (() => void) | undefined;
    void native.on("session://status", () => setReloadKey((k) => k + 1)).then((u) => (un = u));
    return () => {
      window.removeEventListener("focus", onFocus);
      un?.();
    };
  }, []);

  useEffect(() => {
    if (!id && list && list.length && !query) navigate(`/dashboard/sessions/${list[0].id}`, { replace: true });
  }, [id, list, query, navigate]);

  return (
    <div className="flex h-full">
      <aside className="flex w-[320px] shrink-0 flex-col border-r border-border">
        <div className="p-3">
          <h1 className="mb-3 px-1 text-[20px] font-semibold tracking-[-0.015em]">Sessions</h1>
          <div className="relative">
            <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search transcripts and answers"
              aria-label="Search sessions"
              className="h-10 w-full rounded-lg border border-input bg-transparent pl-9 pr-3 text-[13.5px] outline-none focus-visible:border-ring"
            />
          </div>
        </div>
        <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-3" aria-label="Sessions">
          {list === null ? (
            Array.from({ length: 5 }, (_, i) => (
              <li key={i} className="mx-1 my-1 h-[58px] animate-pulse rounded-lg bg-muted/60" />
            ))
          ) : list.length === 0 ? (
            <li className="px-3 py-8 text-center text-[13.5px] text-muted-foreground">
              {query ? <>No sessions match “{query}”.</> : <EmptyLibrary />}
            </li>
          ) : (
            list.map((s) => (
              <li key={s.id}>
                <button
                  type="button"
                  onClick={() => navigate(`/dashboard/sessions/${s.id}`)}
                  aria-current={s.id === id ? "page" : undefined}
                  className={cn(
                    "flex w-full gap-3 rounded-lg px-3 py-2.5 text-left transition-colors",
                    s.id === id ? "bg-accent" : "hover:bg-accent/60",
                  )}
                >
                  <ModeIcon mode={s.mode} className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[14px] font-medium">{sessionTitle(s)}</span>
                    <span className="block text-[12.5px] text-muted-foreground">
                      {formatDate(s.startedAt)}
                      {s.endedAt ? ` · ${formatDuration(s.endedAt - s.startedAt)}` : ""}
                      {s.cardCount ? ` · ${s.cardCount} answer${s.cardCount === 1 ? "" : "s"}` : ""}
                    </span>
                    {query && s.snippet ? (
                      <span className="mt-0.5 line-clamp-2 block text-[12.5px] text-muted-foreground/90">“{s.snippet}”</span>
                    ) : null}
                  </span>
                </button>
              </li>
            ))
          )}
        </ul>
      </aside>
      <div className="min-w-0 flex-1">
        {id ? (
          <SessionDetailView
            id={id}
            onDeleted={() => {
              setReloadKey((k) => k + 1);
              navigate("/dashboard/sessions", { replace: true });
            }}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-[14px] text-muted-foreground">
            {list?.length ? "Choose a session" : null}
          </div>
        )}
      </div>
    </div>
  );
}

function EmptyLibrary() {
  const sc = useSettings((s) => s.settings.shortcuts);
  return (
    <span className="flex flex-col items-center gap-2">
      <span className="font-medium text-foreground">No sessions yet</span>
      <span>
        Start listening in the overlay with <Kbd keys={sc.toggle_listen.key} /> or ask a question. Every conversation is saved
        here with its transcript, answers and notes.
      </span>
    </span>
  );
}

type Tab = "summary" | "answers" | "transcript" | "notes";

function SessionDetailView({ id, onDeleted }: { id: string; onDeleted: () => void }) {
  const [detail, setDetail] = useState<SessionDetail | null | undefined>(undefined);
  const [tab, setTab] = useState<Tab>("summary");
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setDetail(undefined);
    setConfirm(false);
    void getRepo()
      .getSessionDetail(id)
      .then((d) => {
        setDetail(d);
        if (d) setTab(d.session.summaryJson ? "summary" : d.cards.length ? "answers" : "transcript");
      })
      .catch(() => setDetail(null));
  }, [id]);

  const summary = useMemo(() => parseSummary(detail?.session.summaryJson ?? null), [detail]);

  if (detail === undefined) return <div className="p-8" aria-busy />;
  if (detail === null) return <div className="p-8 text-muted-foreground">This session no longer exists.</div>;

  const s = detail.session;
  const title = summary?.title || sessionTitle(s);
  const tabs: [Tab, string, number | null][] = [
    ["summary", "Summary", null],
    ["answers", "Answers", detail.cards.length],
    ["transcript", "Transcript", detail.transcript.length],
    ["notes", "Notes", detail.notes.length],
  ];

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-border px-8 pb-0 pt-6">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <h2 className="text-[20px] font-semibold tracking-[-0.015em]">{title}</h2>
            <p className="mt-1 flex items-center gap-1.5 text-[13px] text-muted-foreground">
              <ModeIcon mode={s.mode} className="size-3.5" />
              {MODE_META[s.mode].label} · {new Date(s.startedAt).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}
              {s.endedAt ? ` · ${formatDuration(s.endedAt - s.startedAt)}` : ""}
            </p>
          </div>
          <Button variant="outline" size="sm" onClick={() => downloadText(`${safeFilename(title)}.md`, sessionToMarkdown(detail))}>
            <Download /> Export Markdown
          </Button>
          {confirm ? (
            <>
              <Button variant="ghost" size="sm" onClick={() => setConfirm(false)}>
                Cancel
              </Button>
              <Button
                variant="destructive"
                size="sm"
                onClick={async () => {
                  await getRepo().deleteSession(s.id);
                  onDeleted();
                }}
              >
                Delete session
              </Button>
            </>
          ) : (
            <Button variant="ghost" size="sm" aria-label="Delete session" onClick={() => setConfirm(true)}>
              <Trash2 />
            </Button>
          )}
        </div>
        <div role="tablist" aria-label="Session views" className="mt-4 flex gap-1">
          {tabs.map(([t, label, n]) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              onClick={() => setTab(t)}
              className={cn(
                "-mb-px border-b-2 px-3 py-2 text-[13.5px] font-medium transition-colors",
                tab === t ? "border-foreground text-foreground" : "border-transparent text-muted-foreground hover:text-foreground",
              )}
            >
              {label}
              {n ? <span className="ml-1.5 text-[12px] text-muted-foreground tabular">{n}</span> : null}
            </button>
          ))}
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-8 py-6">
        <div className="max-w-[760px]">
          {tab === "summary" ? (
            summary ? (
              <div>
                <Markdown className="text-[14.5px]">{summaryToMarkdown(summary)}</Markdown>
                <div className="mt-4">
                  <CopyButton text={summaryToMarkdown(summary)} label="Copy summary" size="md" />
                </div>
              </div>
            ) : (
              <p className="text-[14px] text-muted-foreground">
                No summary for this session. Summaries are written when you stop listening.
              </p>
            )
          ) : null}
          {tab === "answers" ? (
            detail.cards.length ? (
              <div className="flex flex-col gap-3">
                {detail.cards.map((c) => (
                  <SavedCard key={c.id} card={c} />
                ))}
              </div>
            ) : (
              <p className="text-[14px] text-muted-foreground">No answers were given in this session.</p>
            )
          ) : null}
          {tab === "transcript" ? <TranscriptView detail={detail} /> : null}
          {tab === "notes" ? (
            detail.notes.length ? (
              detail.notes.map((n) => <Markdown key={n.kind}>{notesToMarkdown(n.kind, n.contentJson)}</Markdown>)
            ) : (
              <p className="text-[14px] text-muted-foreground">Rolling notes are kept in Meeting and Lecture mode while listening.</p>
            )
          ) : null}
        </div>
      </div>
    </div>
  );
}

function TranscriptView({ detail }: { detail: SessionDetail }) {
  if (!detail.transcript.length) return <p className="text-[14px] text-muted-foreground">No transcript — this was an ask-only session.</p>;
  return (
    <ol className="flex flex-col gap-2.5">
      {detail.transcript.map((t) => (
        <li key={t.id} className="grid grid-cols-[48px_44px_1fr] gap-2 text-[14px] leading-relaxed">
          <span className="pt-0.5 text-[12px] text-muted-foreground tabular">{formatClock(t.t0Ms)}</span>
          <span className={cn("pt-0.5 text-[12px] font-semibold uppercase tracking-wide", t.speaker === "them" ? "text-them" : "text-you")}>
            {t.speaker === "them" ? "Them" : "You"}
          </span>
          <span>{t.text}</span>
        </li>
      ))}
    </ol>
  );
}

function SavedCard({ card }: { card: CardRow }) {
  const p = useMemo(() => parseAnswer(card.content), [card.content]);
  const [img, setImg] = useState<string | null>(null);
  const [showImg, setShowImg] = useState(false);

  useEffect(() => {
    if (!card.screenshotPath) return;
    void native
      .screenshotLoad(card.screenshotPath)
      .then((r) => setImg(`data:${r.mime};base64,${r.dataBase64}`))
      .catch(() => setImg(null));
  }, [card.screenshotPath]);

  return (
    <article className="rounded-xl border border-border bg-card p-4">
      <header className="mb-2 flex items-start gap-3">
        {img ? (
          <button type="button" onClick={() => setShowImg(!showImg)} className="shrink-0" aria-label="Show screenshot">
            <img src={img} alt="Screenshot" className="h-14 w-auto rounded-md border border-border" />
          </button>
        ) : card.screenshotPath ? (
          <ImageIcon className="size-5 text-muted-foreground" />
        ) : null}
        <div className="min-w-0 flex-1">
          <p className="text-[12.5px] font-semibold text-muted-foreground">
            {card.kind === "auto" ? "Them" : card.kind === "ask_now" ? "What should I say?" : card.kind.startsWith("followup") ? "Follow-up" : card.kind === "screen" ? "Screen" : "You asked"}
            {" · "}
            {new Date(card.createdAt).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}
          </p>
          {card.question ? <p className="text-[13.5px] text-muted-foreground">{card.question}</p> : null}
        </div>
        <CopyButton text={card.content} />
      </header>
      {showImg && img ? <img src={img} alt="Screenshot, full size" className="mb-3 w-full rounded-lg border border-border" /> : null}
      {p.headline ? (
        <p className="text-[16px] font-semibold leading-snug">
          <InlineMd text={p.headline} />
        </p>
      ) : null}
      {p.body ? <Markdown className="mt-1.5">{p.body}</Markdown> : null}
      {p.details ? (
        <details className="mt-2">
          <summary className="text-[13px] font-medium text-muted-foreground hover:text-foreground">Details</summary>
          <Markdown className="mt-1">{p.details}</Markdown>
        </details>
      ) : null}
      {card.status === "truncated" ? <p className="mt-2 text-[12.5px] text-warn">This answer was cut off.</p> : null}
    </article>
  );
}
