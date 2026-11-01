import { describe, expect, it } from "vitest";
import { parseAnswer, isNullAnswer, speakableText, talkingPoints } from "./answer";
import { describeSessionError, isCancellation, mapLlmError } from "./errors";

describe("error mapping", () => {
  it("sends key problems to settings, never to retry", () => {
    expect(mapLlmError({ code: "key_invalid" })).toMatchObject({
      title: "Your Claude key was rejected",
      action: "settings",
      retryable: false,
    });
    expect(mapLlmError({ code: "key_missing" }).action).toBe("settings");
  });

  it.each(["rate_limited", "overloaded", "network", "server", "stream_parse"])("offers Retry for %s", (code) => {
    const e = mapLlmError({ code, message: "x" });
    expect(e.action).toBe("retry");
    expect(e.retryable).toBe(true);
  });

  it("shows the API message for bad requests and offers no retry", () => {
    const e = mapLlmError({ code: "bad_request", message: "messages: text content blocks must be non-empty" });
    expect(e.message).toContain("must be non-empty");
    expect(e.action).toBeNull();
  });

  it("handles the native-only codes without offering a futile retry", () => {
    expect(mapLlmError({ code: "keyring_error" })).toMatchObject({ action: "settings", retryable: false });
    expect(mapLlmError({ code: "screenshot_missing" })).toMatchObject({ action: null, retryable: false });
    expect(mapLlmError({ code: "key_no_credit" })).toMatchObject({ action: null, retryable: false });
  });

  it("falls back sensibly for unknown codes", () => {
    expect(mapLlmError({ code: "weird", message: "boom", retryable: true })).toMatchObject({ action: "retry", message: "boom" });
    expect(mapLlmError({ code: "weird" })).toMatchObject({ action: null, title: "Something went wrong" });
  });

  it("treats cancellation as not-an-error", () => {
    expect(isCancellation("cancelled")).toBe(true);
    expect(isCancellation("network")).toBe(false);
  });

  it("explains session errors with a fix where one exists", () => {
    expect(describeSessionError("mic_permission_denied", "")).toEqual({ title: "Windows is blocking the microphone", fix: "mic_privacy" });
    expect(describeSessionError("dg_auth", "").fix).toBe("settings");
    expect(describeSessionError("anything", "Raw message").title).toBe("Raw message");
    expect(describeSessionError("session_active", "").title).toContain("already running");
  });
});

describe("answer parsing", () => {
  it("splits headline, bullets and details", () => {
    const p = parseAnswer("I'd use a hash map.\n- O(n) time\n- O(n) space\n\nDetails\n```py\nx = 1\n```");
    expect(p.headline).toBe("I'd use a hash map.");
    expect(p.body).toBe("- O(n) time\n- O(n) space");
    expect(p.details).toBe("```py\nx = 1\n```");
  });

  it("accepts markdown-styled details markers and headings", () => {
    const p = parseAnswer("## **Say this.**\n- a\n**Details:**\nmore");
    expect(p.headline).toBe("Say this.");
    expect(p.details).toBe("more");
  });

  it("moves an unmarked code block into details", () => {
    const p = parseAnswer("Approach.\n- idea\n```python\npass\n```");
    expect(p.body).toBe("- idea");
    expect(p.details).toBe("```python\npass\n```");
  });

  it("keeps a private Note out of the spoken talking points", () => {
    const p = parseAnswer(
      "I'm proud of the pipeline I rebuilt last year.\n- It kept failing overnight.\n- I added checks and alerts.\nNote: Swap in your own project and numbers.",
    );
    expect(p.body).toBe("- It kept failing overnight.\n- I added checks and alerts.");
    expect(p.note).toBe("Swap in your own project and numbers.");
    expect(parseAnswer("Say this.\n- a\n- **Note:** tip").note).toBe("tip");
    expect(parseAnswer("Say this.\n- a").note).toBeNull();
  });

  it("builds the speakable text from the opening line and talking points only", () => {
    const p = parseAnswer(
      "I'd use a **token bucket** per API key.\n- Store counts in `Redis`.\n- Return a 429 with Retry-After.\nNote: mention tiers if asked.\n\nDetails\n```py\nx = 1\n```",
    );
    expect(speakableText(p)).toBe(
      "I'd use a token bucket per API key.\nStore counts in Redis.\nReturn a 429 with Retry-After.",
    );
    expect(talkingPoints("- a\n- b")).toEqual(["a", "b"]);
    expect(talkingPoints("Some prose\n- a")).toBeNull();
  });

  it("handles answers that open with a bullet or code", () => {
    expect(parseAnswer("- just bullets").headline).toBe("");
    expect(parseAnswer("```js\n1\n```").body).toContain("```js");
  });

  it("detects the ∅ sentinel", () => {
    expect(isNullAnswer(" ∅")).toBe(true);
    expect(isNullAnswer("Sure ∅")).toBe(false);
  });
});
