// Runs one llm_stream request and reduces its LlmEvents to a single outcome.
// Transport is injected so the question pipeline can be unit-tested.

import type { LlmEvent, LlmUsage } from "../types";
import type { LlmRequest } from "./requests";
import { parseJsonOutput } from "./schemas";

export interface LlmTransport {
  stream(
    requestId: string,
    body: object,
    betas: string[] | undefined,
    onEvent: (e: LlmEvent) => void,
  ): Promise<void>;
  cancel(requestId: string): Promise<void>;
}

export type StreamOutcome =
  | { kind: "done"; text: string; stopReason: string | null; usage: LlmUsage | null }
  | { kind: "error"; code: string; message: string; retryable: boolean; status?: number; text: string }
  | { kind: "cancelled"; text: string };

export interface StreamCallbacks {
  onStart?(model: string): void;
  onThinking?(): void;
  onText?(delta: string, full: string): void;
}

export async function runStream(
  transport: LlmTransport,
  requestId: string,
  req: LlmRequest,
  cb: StreamCallbacks = {},
): Promise<StreamOutcome> {
  let text = "";
  let outcome: StreamOutcome | null = null;

  const onEvent = (e: LlmEvent) => {
    if (outcome) return; // ignore anything after a terminal event
    switch (e.type) {
      case "start":
        cb.onStart?.(e.model);
        break;
      case "thinking":
        cb.onThinking?.();
        break;
      case "text":
        text += e.delta;
        cb.onText?.(e.delta, text);
        break;
      case "stop":
        outcome = { kind: "done", text, stopReason: e.stopReason, usage: e.usage };
        break;
      case "error":
        outcome =
          e.code === "cancelled"
            ? { kind: "cancelled", text }
            : { kind: "error", code: e.code, message: e.message, retryable: e.retryable, status: e.status, text };
        break;
    }
  };

  try {
    await transport.stream(requestId, req.body, req.betas, onEvent);
  } catch (err) {
    if (!outcome) {
      const e = err as { code?: string; message?: string };
      const code = e?.code ?? "unknown";
      outcome =
        code === "cancelled"
          ? { kind: "cancelled", text }
          : {
              kind: "error",
              code,
              message: e?.message ?? String(err),
              retryable: code === "network" || code === "rate_limited" || code === "overloaded" || code === "server",
              text,
            };
    }
  }
  if (!outcome) {
    // The command resolved without a terminal event: treat the stream as broken.
    outcome = { kind: "error", code: "stream_parse", message: "The stream ended unexpectedly.", retryable: true, text };
  }
  return outcome;
}

/** Runs a structured-output request and parses the JSON result. */
export async function runStructured<T>(
  transport: LlmTransport,
  requestId: string,
  req: LlmRequest,
): Promise<{ ok: true; value: T } | { ok: false; code: string; message: string }> {
  const out = await runStream(transport, requestId, req);
  if (out.kind === "cancelled") return { ok: false, code: "cancelled", message: "Cancelled" };
  if (out.kind === "error") return { ok: false, code: out.code, message: out.message };
  if (out.stopReason === "refusal") return { ok: false, code: "refusal", message: "Claude declined this one" };
  const value = parseJsonOutput<T>(out.text);
  if (!value) return { ok: false, code: "stream_parse", message: "The result wasn't valid JSON." };
  return { ok: true, value };
}
