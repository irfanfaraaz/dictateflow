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

export const EVENT = {
  start: "dictation:start",
  audioLevel: "dictation:audio_level",
  injected: "dictation:injected",
  error: "dictation:error",
} as const;
