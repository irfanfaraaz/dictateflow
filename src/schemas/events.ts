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

// PHASE 2 DEBUG ONLY — this schema and the corresponding `dictation:final`
// pill subscription will be removed in Phase 3 once clipboard injection
// lands. The shape brief mandates that the pill never renders transcript
// text in the steady state.
export const finalSchema = z.object({
  text: z.string(),
});
export type DictationFinal = z.infer<typeof finalSchema>;

export const EVENT = {
  start: "dictation:start",
  audioLevel: "dictation:audio_level",
  injected: "dictation:injected",
  error: "dictation:error",
  // PHASE 2 DEBUG ONLY — see above.
  final: "dictation:final",
} as const;
