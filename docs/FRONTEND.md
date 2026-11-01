# Pluely X frontend

React 19 + TypeScript + Vite + Tailwind 4 (+ typography) + shadcn/Radix + Zustand. The product is in `docs/PLAN.md` and every native call is in `docs/CONTRACT.md`.

## Structure

```
src/main.tsx            window router: label "main" → overlay, "dashboard" → dashboard, "capture-overlay-*" → region picker
src/app/overlay/        OverlayApp (panel / pill), TopBar (mode menu, Listen + meters, share/click-through/dashboard/collapse),
                        TranscriptStrip (Transcript | Notes), AnswerStack + AnswerCard, AskBox, controller.ts (shortcuts,
                        screenshots, typing mode, pill sizing), store.ts (overlay UI state)
src/app/dashboard/      DashboardApp (router + sidebar), Onboarding (4 steps), SettingsPage (6 sections),
                        SessionsPage (list, search, detail, export), parts.tsx (key rows, audio test), ShortcutRecorder
src/app/capture/        CaptureApp: drag-a-box region selector (rect in CSS px; Rust converts with the monitor scale)
src/core/native.ts      typed wrappers for every contract command, event and Channel; the only @tauri-apps/api importer
src/core/native.mock.ts browser mock of the whole native layer (see below)
src/core/types.ts       contract types, 1:1 with CONTRACT.md
src/core/settings.ts    settings store (mirrors Rust settings.json), key status, shortcut registration health
src/core/ai/            prompts.ts (base contract + mode prompts, byte-stable), context.ts (chronological Them/You transcript,
                        budgets, keyterms), requests.ts (request bodies typed with @anthropic-ai/sdk beta types), schemas.ts
                        (structured-output JSON schemas), prefilter.ts, answer.ts (headline/body/details split, ∅), errors.ts, run.ts
src/core/live/          transcript.ts (pure assembly), pipeline.ts (question pipeline), cards.ts (card store),
                        session.ts (session engine: events, persistence, rolling notes, summaries)
src/core/db/            Repo interface; sqlRepo (plugin-sql, sqlite:pluelyx.db, FTS5 search); memoryRepo (browser/tests);
                        markdown.ts (export)
src/core/briefs/        PDF (pdfjs-dist), DOCX (mammoth), TXT/MD text extraction, loaded on demand
src/components/         shadcn ui/*, Markdown (Streamdown + shiki + KaTeX), common.tsx (Kbd, Hint tooltips, IconButton, CopyButton, InlineMd)
```

## Stores

- `useSettings` / `useKeys` / `useShortcutHealth` (`core/settings.ts`): Rust-owned settings, key presence (never the key), and shortcut registration failures. They stay in sync across windows through `settings://changed`, `keys://changed` and `shortcut://error` plus `shortcuts_status` on mount.
- `useLive` (`core/live/session.ts`): listening status, per-source state and devices, 10 Hz levels, the transcript, rolling notes, the conversation's session id and brief override.
- `useCards` (`core/live/cards.ts`): answer cards. The `AnswerPipeline` writes into it through a `CardSink`.
- `useOverlay` (`app/overlay/store.ts`): pill/panel, transcript strip, ask box draft, attachment, reply-to, window state, toasts.

## How answers flow

1. `turn` events reach `AnswerPipeline.onTurn`. Only *Them* turns that pass the local pre-filter (or carry the native `question` hint) in modes with auto-answers start a request; a turn id is never answered twice; a newer candidate cancels the in-flight auto-answer.
2. Requests (`core/ai/requests.ts`) put the base contract, mode prompt and brief in `system` with `cache_control` on the last block (byte-stable), then the transcript and question. They always send `output_config.effort`, `max_tokens`, `fallbacks: "default"` with beta `server-side-fallback-2026-07-01`, and never `thinking`, `temperature`, `top_p` or `top_k`.
3. Auto-answers are provisional: a stream that starts with `∅` removes the card and cancels the request. Refusals and `max_tokens` show notes on the card. Errors become error cards and are never persisted.
4. Completed cards are saved (with `screenshot_save` for images); final, non-echo transcript lines are saved as they arrive. Stopping writes `ended_at` and a structured per-mode summary.

## Mocks

When `window.__TAURI_INTERNALS__` is missing (plain `npm run dev` in a browser), `native` is `createMockNative()` and the repo is an in-memory store persisted to localStorage and seeded with four demo sessions and two briefs. The mock simulates:

- a scripted call per mode (interview, meeting, coding, lecture; an audio check before onboarding) with interim→final transcripts for both speakers, turns with question hints, 10 Hz levels, an echo segment that is patched away, and a Deepgram reconnect blip at ~16 s;
- streamed Claude answers that react to the request: scripted answers for scripted questions, `∅` for statements, follow-up chips, screen answers when an image block is present, and JSON for notes and summaries;
- fake screenshots (canvas) with a thumbnail, key tests that pass for keys starting with `sk-ant-` / `dg`, settings, window state and global shortcuts (press Ctrl+Alt+L etc. in the overlay tab).

Routes: `/` overlay (size the viewport 440×560), `/dashboard`, `/capture?monitor=X`. Query flags: `?fresh` resets to a first run (onboarding), `?conflicts` simulates shortcuts owned by another app. In the console, `__pluelyxMock.failNext = "key_invalid"` (or `rate_limited`, `overloaded`, `network`…) fails the next LLM request; a question containing `[mock:overloaded]` does the same. `__pluelyxMock.speed` scales every simulated delay.

## Run and test

```
npm install
npm run dev        # http://localhost:1420 with the mock
npm test           # vitest: transcript assembly, pre-filter, pipeline (∅, cancel, dedupe, errors), prompt builders, errors, repo
npm run build      # tsc + vite build
```

Screenshots of every state are in `docs/screens/`.
