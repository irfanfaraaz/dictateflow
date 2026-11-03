import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  type DeepPartial,
  type Settings,
  SETTINGS_UPDATED_EVENT,
  settingsSchema,
} from "@/schemas/settings";
import { getSettings, updateSettings } from "@/ipc/commands";

interface UseSettings {
  /** Latest snapshot — null until the first `getSettings()` resolves. */
  settings: Settings | null;
  /** Optimistic patcher; rolls back on Rust failure. */
  update: (patch: DeepPartial<Settings>) => Promise<void>;
  /** Initial load complete + cross-window listener wired. */
  ready: boolean;
  /** Set when the initial load failed; `settings` stays null. */
  error: string | null;
  /** Retry the initial load after an error. */
  retry: () => void;
}

/**
 * Loads settings from Rust on mount, mirrors `settings:updated` events from
 * other windows, and provides an optimistic `update()` that reverts on
 * failure.
 *
 * Why optimistic: in the editorial-magazine settings UI a radio click feels
 * broken if it doesn't fill instantly. The Rust round-trip is sub-10 ms in
 * practice, but waiting for it before re-rendering means a perceptible
 * delay on slower Macs.
 *
 * Why we keep the snapshot in React state instead of querying Rust each
 * render: tauri-plugin-store is the source of truth on disk, but every read
 * is an FFI call. Mirroring once + listening for `settings:updated` is
 * cheaper and matches the "Rust authoritative, React cached" pattern.
 */
export function useSettings(): UseSettings {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // We keep the latest known-good snapshot in a ref for rollback. Using a
  // ref (not state) avoids stale closures inside `update`.
  const lastConfirmed = useRef<Settings | null>(null);
  // Latest optimistic snapshot: successive quick updates layer onto this,
  // not onto `lastConfirmed`, so a second click never briefly undoes the
  // first one.
  const optimisticRef = useRef<Settings | null>(null);
  // Only the newest in-flight update may apply its response; an older one
  // resolving late must not overwrite a newer snapshot.
  const seqRef = useRef(0);

  // Initial load (re-runs on retry).
  useEffect(() => {
    let cancelled = false;
    setError(null);
    getSettings()
      .then((s) => {
        if (cancelled) return;
        setSettings(s);
        lastConfirmed.current = s;
        optimisticRef.current = s;
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error("[settings] initial load failed", err);
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, [attempt]);

  const retry = useCallback(() => setAttempt((a) => a + 1), []);

  // Cross-window sync. The other window's `update_settings` call emits
  // `settings:updated`; we mirror it locally so radios stay in lockstep.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<unknown>(SETTINGS_UPDATED_EVENT, (event) => {
      const parsed = settingsSchema.safeParse(event.payload);
      if (!parsed.success) {
        // eslint-disable-next-line no-console
        console.warn("[settings] invalid settings:updated payload", parsed.error.issues);
        return;
      }
      lastConfirmed.current = parsed.data;
      optimisticRef.current = parsed.data;
      setSettings(parsed.data);
    })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error("[settings] listen(settings:updated) failed", err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const update = useCallback(async (patch: DeepPartial<Settings>) => {
    const prev = optimisticRef.current ?? lastConfirmed.current;
    const seq = ++seqRef.current;
    if (!prev) {
      // No baseline yet — defer to the synchronous Rust path; we cannot
      // optimistically apply a patch without something to merge into.
      try {
        const next = await updateSettings(patch);
        lastConfirmed.current = next;
        if (seq === seqRef.current) {
          optimisticRef.current = next;
          setSettings(next);
        }
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("[settings] update failed", err);
        throw err;
      }
      return;
    }

    // Optimistic merge — same shape as the Rust merger: object keys recurse,
    // arrays/scalars replace.
    const optimistic = mergeDeep(prev, patch) as Settings;
    optimisticRef.current = optimistic;
    setSettings(optimistic);
    try {
      const next = await updateSettings(patch);
      lastConfirmed.current = next;
      // Rust applies patches in order, so the newest response is the full
      // truth; older responses are superseded.
      if (seq === seqRef.current) {
        optimisticRef.current = next;
        setSettings(next);
      }
    } catch (err) {
      // Roll back to the last confirmed snapshot (pending optimistic layers
      // are dropped with it).
      optimisticRef.current = lastConfirmed.current;
      setSettings(lastConfirmed.current);
      // eslint-disable-next-line no-console
      console.error("[settings] update failed; rolled back", err);
      throw err;
    }
  }, []);

  return { settings, update, ready, error, retry };
}

/**
 * Recursive object merge that mirrors `merge()` in `settings/store.rs`.
 * Plain-object values recurse; arrays and primitives replace wholesale.
 *
 * Exported for tests; consumers go through `useSettings.update`.
 */
export function mergeDeep<T>(base: T, patch: unknown): T {
  if (!isPlainObject(base) || !isPlainObject(patch)) {
    return (patch === undefined ? base : (patch as T));
  }
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [k, v] of Object.entries(patch as Record<string, unknown>)) {
    out[k] = mergeDeep(out[k], v);
  }
  return out as T;
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
