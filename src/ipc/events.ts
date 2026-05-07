import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";
import type { ZodSchema } from "zod";

/**
 * Subscribe to a Tauri event from React. The payload is validated against the
 * supplied zod schema; invalid payloads are logged and dropped (not thrown,
 * so a single bad event from Rust never crashes the pill).
 *
 * Web analogue: imagine `useEffect(() => { const cb = ev => …; window.addEventListener('foo', cb); return () => window.removeEventListener('foo', cb); }, [])`,
 * but with the listener registered on the OS-wide IPC bus.
 */
export function useEvent<T>(
  name: string,
  schema: ZodSchema<T>,
  handler: (payload: T) => void,
): void {
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    listen<unknown>(name, (event) => {
      const parsed = schema.safeParse(event.payload);
      if (!parsed.success) {
        // eslint-disable-next-line no-console
        console.warn(`[ipc] invalid payload for ${name}`, parsed.error.issues);
        return;
      }
      handler(parsed.data);
    })
      .then((un) => {
        if (cancelled) {
          un();
        } else {
          unlisten = un;
        }
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error(`[ipc] listen(${name}) failed`, err);
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // We intentionally do NOT depend on `handler` — callers should use a
    // ref-stable callback (or wrap with useCallback) to avoid re-subscribing
    // every render. The schema and name are also expected to be stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name]);
}
