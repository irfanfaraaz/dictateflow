import { z } from "zod";

// Mirror of the Rust Settings struct in `src-tauri/src/settings/schema.rs`.
// Field names + enum spellings must stay in sync; serde uses `snake_case`
// renaming on enums (not on struct fields), so the strings here are
// `"local"`/`"scribe"` etc. while object keys stay snake_case Rust-style.

export const engineKindSchema = z.enum(["local", "scribe"]);
export type EngineKind = z.infer<typeof engineKindSchema>;

export const hotkeyModeSchema = z.enum(["hold", "toggle"]);
export type HotkeyMode = z.infer<typeof hotkeyModeSchema>;

export const localSettingsSchema = z.object({
  model: z.string(),
  installed_models: z.array(z.string()),
});
export type LocalSettings = z.infer<typeof localSettingsSchema>;

export const scribeSettingsSchema = z.object({
  has_api_key: z.boolean(),
  streaming: z.boolean(),
});
export type ScribeSettings = z.infer<typeof scribeSettingsSchema>;

export const hotkeySettingsSchema = z.object({
  mode: hotkeyModeSchema,
  binding: z.string(),
});
export type HotkeySettings = z.infer<typeof hotkeySettingsSchema>;

export const limitsSchema = z.object({
  local_max_seconds: z.number().int().nonnegative(),
  scribe_max_seconds: z.number().int().nonnegative(),
});
export type Limits = z.infer<typeof limitsSchema>;

export const pillSettingsSchema = z.object({
  follow_cursor: z.boolean(),
});
export type PillSettings = z.infer<typeof pillSettingsSchema>;

export const soundsSettingsSchema = z.object({
  start_enabled: z.boolean(),
  stop_enabled: z.boolean(),
});
export type SoundsSettings = z.infer<typeof soundsSettingsSchema>;

export const settingsSchema = z.object({
  engine: engineKindSchema,
  local: localSettingsSchema,
  scribe: scribeSettingsSchema,
  hotkey: hotkeySettingsSchema,
  language: z.string(),
  limits: limitsSchema,
  pill: pillSettingsSchema,
  sounds: soundsSettingsSchema,
  telemetry: z.boolean(),
  first_run_completed: z.boolean(),
});
export type Settings = z.infer<typeof settingsSchema>;

// Permission shape mirrors `src-tauri/src/permissions.rs`.
export const permissionStateSchema = z.enum(["granted", "denied", "not_requested"]);
export type PermissionState = z.infer<typeof permissionStateSchema>;

export const permissionKindSchema = z.enum(["microphone", "accessibility", "input_monitoring"]);
export type PermissionKind = z.infer<typeof permissionKindSchema>;

export const permissionsSnapshotSchema = z.object({
  microphone: permissionStateSchema,
  accessibility: permissionStateSchema,
  input_monitoring: permissionStateSchema,
});
export type PermissionsSnapshot = z.infer<typeof permissionsSnapshotSchema>;

/**
 * Settings event name (Rust → frontend). Carries the full new Settings
 * snapshot so other windows can mirror without merging patches themselves.
 */
export const SETTINGS_UPDATED_EVENT = "settings:updated" as const;

/** Recursive partial helper for `update_settings` patches. */
export type DeepPartial<T> = {
  [P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P];
};
