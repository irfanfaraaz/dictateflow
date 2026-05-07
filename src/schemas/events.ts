import { z } from "zod";

// Mirror of the Rust payload structs in `src-tauri/src/events.rs`.
// Keeping them in sync is manual for now; Phase 7 introduces codegen.

export const dictationStartSchema = z.object({
  trigger: z.enum(["hold", "toggle"]),
});
export type DictationStart = z.infer<typeof dictationStartSchema>;

export const audioLevelSchema = z.object({
  rms: z.number(),
});
export type AudioLevel = z.infer<typeof audioLevelSchema>;

export const injectedSchema = z.object({
  text: z.string(),
  restored_clipboard: z.boolean(),
});
export type Injected = z.infer<typeof injectedSchema>;

export const errorSchema = z.object({
  code: z.string(),
  message: z.string(),
  recovery_hint: z.string().nullable().optional(),
});
export type DictationError = z.infer<typeof errorSchema>;

// `dictation:final` still fires from Rust — Phase 9 may use it for a
// tray-history surface — but per shape brief §3 the pill never renders
// transcript text. The schema is kept here for downstream consumers.
export const finalSchema = z.object({
  text: z.string(),
});
export type DictationFinal = z.infer<typeof finalSchema>;

export const EVENT = {
  start: "dictation:start",
  audioLevel: "dictation:audio_level",
  injected: "dictation:injected",
  error: "dictation:error",
  final: "dictation:final",
} as const;

// ---------- Model download event schemas (Phase 5) ----------

export const modelDownloadProgressSchema = z.object({
  id: z.string(),
  bytes_done: z.number(),
  bytes_total: z.number(),
});
export type ModelDownloadProgress = z.infer<typeof modelDownloadProgressSchema>;

export const modelDownloadCompleteSchema = z.object({
  id: z.string(),
});
export type ModelDownloadComplete = z.infer<typeof modelDownloadCompleteSchema>;

export const modelDownloadErrorSchema = z.object({
  id: z.string(),
  message: z.string(),
});
export type ModelDownloadError = z.infer<typeof modelDownloadErrorSchema>;

export const MODEL_EVENT = {
  progress: "model:download_progress",
  complete: "model:download_complete",
  error: "model:download_error",
} as const;
