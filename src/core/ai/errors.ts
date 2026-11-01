// Maps LLM / native errors to what an error card shows. Errors are rendered
// as error cards and are never stored as answers.

export type CardErrorAction = "settings" | "retry" | null;

export interface CardError {
  code: string;
  title: string;
  message: string;
  action: CardErrorAction;
  retryable: boolean;
}

interface ErrorLike {
  code: string;
  message?: string;
  retryable?: boolean;
  status?: number;
}

export function mapLlmError(e: ErrorLike): CardError {
  const raw = e.message?.trim() ?? "";
  switch (e.code) {
    case "key_missing":
      return {
        code: e.code,
        title: "Add your Claude key to get answers",
        message: "Pluely X needs an Anthropic API key. It's stored in Windows Credential Manager.",
        action: "settings",
        retryable: false,
      };
    case "key_invalid":
      return {
        code: e.code,
        title: "Your Claude key was rejected",
        message: "Anthropic said the key is invalid or revoked. Paste a new key in Settings.",
        action: "settings",
        retryable: false,
      };
    case "key_no_credit":
      return {
        code: e.code,
        title: "Your Anthropic account is out of credit",
        message: "Add credit in the Anthropic Console (Plans & billing), then ask again.",
        action: null,
        retryable: false,
      };
    case "keyring_error":
      return {
        code: e.code,
        title: "Couldn't read your Claude key",
        message: "Windows Credential Manager didn't return the key. Save it again in Settings.",
        action: "settings",
        retryable: false,
      };
    case "screenshot_missing":
      return {
        code: e.code,
        title: "That screenshot has expired",
        message: "Only the last 10 screenshots are kept. Capture the screen again and re-ask.",
        action: null,
        retryable: false,
      };
    case "rate_limited":
      return {
        code: e.code,
        title: "Claude is rate-limiting requests",
        message: "Too many requests in a short time. Wait a moment and retry.",
        action: "retry",
        retryable: true,
      };
    case "overloaded":
      return {
        code: e.code,
        title: "Claude is overloaded right now",
        message: "Anthropic's servers are busy. Retrying usually works within seconds.",
        action: "retry",
        retryable: true,
      };
    case "server":
      return {
        code: e.code,
        title: "Claude had a server error",
        message: raw || "Anthropic returned an unexpected error. Retry in a moment.",
        action: "retry",
        retryable: true,
      };
    case "network":
      return {
        code: e.code,
        title: "Can't reach Claude",
        message: "Check your internet connection, then retry.",
        action: "retry",
        retryable: true,
      };
    case "bad_request":
      return {
        code: e.code,
        title: "Claude couldn't accept this request",
        message: raw || "The request was rejected as invalid.",
        action: null,
        retryable: false,
      };
    case "stream_parse":
      return {
        code: e.code,
        title: "The answer stream broke off",
        message: "The response arrived garbled. Retry to ask again.",
        action: "retry",
        retryable: true,
      };
    default:
      return {
        code: e.code || "unknown",
        title: "Something went wrong",
        message: raw || "The answer could not be completed.",
        action: e.retryable ? "retry" : null,
        retryable: !!e.retryable,
      };
  }
}

/** Errors that mean "the user stopped it", which never produce an error card. */
export function isCancellation(code: string): boolean {
  return code === "cancelled";
}

/** Session (audio / Deepgram) errors in plain words. */
export function describeSessionError(code: string, fallback: string): { title: string; fix?: "mic_privacy" | "settings" } {
  switch (code) {
    case "mic_permission_denied":
      return { title: "Windows is blocking the microphone", fix: "mic_privacy" };
    case "device_not_found":
      return { title: "That audio device isn't connected" };
    case "device_lost":
      return { title: "Audio device disconnected — reconnecting" };
    case "audio_init_failed":
      return { title: "Couldn't start audio capture" };
    case "key_missing":
      return { title: "Add a Deepgram key to listen", fix: "settings" };
    case "dg_auth":
      return { title: "Your Deepgram key was rejected", fix: "settings" };
    case "dg_payment":
      return { title: "Your Deepgram account is out of credit", fix: "settings" };
    case "dg_rate_limited":
      return { title: "Deepgram is rate-limiting — retrying" };
    case "dg_network":
    case "dg_timeout":
      return { title: "Lost the connection to Deepgram — retrying" };
    case "dg_server":
      return { title: "Deepgram had a server error — retrying" };
    case "dg_bad_request":
      return { title: "Deepgram rejected the session settings", fix: "settings" };
    case "session_active":
      return { title: "Listening is already running (maybe the audio test in Settings)" };
    case "no_session":
    case "session_ended":
      return { title: "The listening session already ended" };
    case "open_failed":
      return { title: "Couldn't open Windows privacy settings" };
    default:
      return { title: fallback || "Listening hit a problem" };
  }
}
