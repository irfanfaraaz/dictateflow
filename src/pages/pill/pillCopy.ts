import type { DictationError } from "@/schemas/events";

/** Longest message the pill shows before falling back to friendly copy. */
export const MAX_PILL_TEXT = 60;

export type PillTone = "error" | "notice";
export interface PillMessage {
  text: string;
  tone: PillTone;
}

/**
 * Friendly copy per backend error code (see `emit_error` / ErrorPayload in
 * src-tauri/src/events.rs). Used when the backend message is missing or too
 * long / technical for a 300 px pill.
 */
const BY_CODE: Record<string, PillMessage> = {
  missing_api_key: { text: "Add your ElevenLabs API key in Settings", tone: "error" },
  model_not_installed: { text: "Download a model in Settings → Models", tone: "error" },
  audio: { text: "Couldn't use the microphone", tone: "error" },
  mic_silent: { text: "No sound from the microphone", tone: "error" },
  device_lost: { text: "Microphone disconnected", tone: "error" },
  websocket: { text: "Couldn't reach ElevenLabs — check internet", tone: "error" },
  inference: { text: "Transcription failed", tone: "error" },
  transcription_timeout: { text: "Transcription timed out", tone: "error" },
  inject_failed: { text: "Couldn't paste — text is on your clipboard", tone: "notice" },
  accessibility_denied: { text: "Re-enable Accessibility to paste — text copied", tone: "notice" },
  focus_changed: { text: "Focus changed — text copied, press ⌘V", tone: "notice" },
  start_failed: { text: "Couldn't start dictation", tone: "error" },
  io: { text: "Something went wrong reading files", tone: "error" },
};

/** Codes where the transcript is safe on the clipboard — not a failure. */
const NOTICE_CODES = new Set(["focus_changed", "accessibility_denied", "inject_failed"]);

export function describeError(e: Pick<DictationError, "code" | "message">): PillMessage {
  const tone: PillTone = NOTICE_CODES.has(e.code) ? "notice" : "error";
  const msg = (e.message ?? "").trim();
  if (msg.length > 0 && msg.length <= MAX_PILL_TEXT && !msg.includes("\n")) {
    return { text: msg, tone };
  }
  return BY_CODE[e.code] ?? { text: "Something went wrong", tone };
}

/** Seconds before the cap at which the countdown appears. */
const COUNTDOWN_FROM_S = 10;

/** "0:09"-style label for the last seconds before the duration cap, else null. */
export function countdownLabel(capSeconds: number, elapsedSeconds: number): string | null {
  if (!(capSeconds > 0)) return null;
  const remaining = capSeconds - elapsedSeconds;
  if (remaining <= 0 || remaining > COUNTDOWN_FROM_S) return null;
  const s = Math.ceil(remaining);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
