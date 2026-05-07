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
