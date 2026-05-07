import { invoke } from "@tauri-apps/api/core";
import {
  type DeepPartial,
  type PermissionKind,
  type PermissionsSnapshot,
  type Settings,
  permissionsSnapshotSchema,
  settingsSchema,
} from "@/schemas/settings";

/**
 * Typed wrappers around `invoke()` for every Rust command.
 *
 * Each wrapper validates the response against its zod schema before
 * returning. Invalid responses throw a descriptive error (rather than
 * silently coercing to `unknown`) so callers can render an inline error
 * without `any` leaking into the React tree.
 *
 * Web analogue: think of `invoke('cmd', args)` as
 * `fetch('/api/cmd', { body: args })` against a server you trust to follow
 * the same zod schema you publish.
 */

export async function getSettings(): Promise<Settings> {
  const raw = await invoke<unknown>("get_settings");
  return settingsSchema.parse(raw);
}

export async function updateSettings(
  patch: DeepPartial<Settings>,
): Promise<Settings> {
  const raw = await invoke<unknown>("update_settings", { patch });
  return settingsSchema.parse(raw);
}

export async function getPermissions(): Promise<PermissionsSnapshot> {
  const raw = await invoke<unknown>("get_permissions");
  return permissionsSnapshotSchema.parse(raw);
}

export async function openSystemSettings(kind: PermissionKind): Promise<void> {
  await invoke("open_system_settings", { kind });
}

export async function openPath(path: string): Promise<void> {
  await invoke("open_path", { path });
}

export async function resolveLogPath(): Promise<string> {
  const raw = await invoke<unknown>("resolve_log_path");
  if (typeof raw !== "string") {
    throw new Error(`resolve_log_path: expected string, got ${typeof raw}`);
  }
  return raw;
}

/** Save an ElevenLabs API key to the macOS Keychain. */
export async function setApiKey(key: string): Promise<void> {
  await invoke("set_api_key", { key });
}

/** Remove the ElevenLabs API key from Keychain. */
export async function clearApiKey(): Promise<void> {
  await invoke("clear_api_key");
}

/** Run health_check() on the currently-configured engine. Returns true if healthy. */
export async function testEngine(): Promise<boolean> {
  return await invoke<boolean>("test_engine");
}

// ---------- Model management ----------

/** Start downloading a GGML model by catalog id. Fire-and-forget; progress arrives via events. */
export async function downloadModel(id: string): Promise<void> {
  await invoke("download_model", { id });
}

/** Cancel an in-progress model download. No-op if none is running for the id. */
export async function cancelModelDownload(id: string): Promise<void> {
  await invoke("cancel_model_download", { id });
}

/** Delete an installed model file and sync installed_models in settings. */
export async function deleteModel(id: string): Promise<void> {
  await invoke("delete_model", { id });
}

/** Return the list of installed model ids (disk scan cross-referenced with catalog). */
export async function listInstalledModels(): Promise<string[]> {
  const raw = await invoke<unknown>("list_installed_models");
  if (!Array.isArray(raw) || !raw.every((x) => typeof x === "string")) {
    throw new Error(`list_installed_models: unexpected response ${JSON.stringify(raw)}`);
  }
  return raw as string[];
}
