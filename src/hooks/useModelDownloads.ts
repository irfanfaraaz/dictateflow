/**
 * Manages model download state: progress, errors, installed list.
 *
 * Subscribes to the three `model:*` Tauri events and reconciles them into a
 * single state object the Models tab and Engine tab can consume.
 *
 * Web analogue: like a polling SWR hook, but driven by server-sent events
 * instead of polling — the Rust side pushes progress, we just accumulate it.
 */

import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  cancelModelDownload,
  deleteModel,
  downloadModel,
  listInstalledModels,
} from "@/ipc/commands";
import {
  MODEL_EVENT,
  modelDownloadCompleteSchema,
  modelDownloadErrorSchema,
  modelDownloadProgressSchema,
} from "@/schemas/events";

export interface DownloadProgress {
  bytesDone: number;
  bytesTotal: number;
}

export interface UseModelDownloads {
  /** Map of model id → download progress (only present while downloading). */
  progress: Record<string, DownloadProgress>;
  /** Map of model id → last error message. Cleared on next download attempt. */
  errors: Record<string, string>;
  /** List of installed model ids (disk-verified). */
  installed: string[];
  /** Re-query `list_installed_models` from Rust. */
  refresh: () => void;
  /** Start a download for the given model id. */
  startDownload: (id: string) => void;
  /** Cancel an in-progress download. */
  cancelDownload: (id: string) => void;
  /** Delete an installed model (with settings sync). */
  deleteModel: (id: string) => void;
}

export function useModelDownloads(): UseModelDownloads {
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [installed, setInstalled] = useState<string[]>([]);

  const refresh = useCallback(() => {
    listInstalledModels()
      .then((ids) => setInstalled(ids))
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error("[useModelDownloads] list_installed_models failed", err);
      });
  }, []);

  // Initial load.
  useEffect(() => {
    refresh();
  }, [refresh]);

  // Subscribe to progress events.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    listen<unknown>(MODEL_EVENT.progress, (event) => {
      const parsed = modelDownloadProgressSchema.safeParse(event.payload);
      if (!parsed.success) {
        console.warn("[useModelDownloads] invalid progress payload", parsed.error.issues);
        return;
      }
      const { id, bytes_done, bytes_total } = parsed.data;
      setProgress((prev) => ({
        ...prev,
        [id]: { bytesDone: bytes_done, bytesTotal: bytes_total },
      }));
    })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch((err) => console.error("[useModelDownloads] listen progress failed", err));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Subscribe to complete events.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    listen<unknown>(MODEL_EVENT.complete, (event) => {
      const parsed = modelDownloadCompleteSchema.safeParse(event.payload);
      if (!parsed.success) {
        console.warn("[useModelDownloads] invalid complete payload", parsed.error.issues);
        return;
      }
      const { id } = parsed.data;
      // Remove from progress, refresh installed list.
      setProgress((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setErrors((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      // Re-query disk so the list is authoritative.
      listInstalledModels()
        .then((ids) => setInstalled(ids))
        .catch((err) => console.error("[useModelDownloads] refresh after complete failed", err));
    })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch((err) => console.error("[useModelDownloads] listen complete failed", err));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Subscribe to error events.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    listen<unknown>(MODEL_EVENT.error, (event) => {
      const parsed = modelDownloadErrorSchema.safeParse(event.payload);
      if (!parsed.success) {
        console.warn("[useModelDownloads] invalid error payload", parsed.error.issues);
        return;
      }
      const { id, message } = parsed.data;
      setProgress((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setErrors((prev) => ({ ...prev, [id]: message }));
    })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch((err) => console.error("[useModelDownloads] listen error failed", err));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const startDownload = useCallback((id: string) => {
    // Clear any previous error for this id.
    setErrors((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    downloadModel(id).catch((err) => {
      setErrors((prev) => ({
        ...prev,
        [id]: err instanceof Error ? err.message : String(err),
      }));
    });
  }, []);

  const cancel = useCallback((id: string) => {
    cancelModelDownload(id).catch((err) =>
      console.error("[useModelDownloads] cancel failed", err),
    );
  }, []);

  const remove = useCallback(
    (id: string) => {
      deleteModel(id)
        .then(() => refresh())
        .catch((err) =>
          setErrors((prev) => ({
            ...prev,
            [id]: err instanceof Error ? err.message : String(err),
          })),
        );
    },
    [refresh],
  );

  return {
    progress,
    errors,
    installed,
    refresh,
    startDownload,
    cancelDownload: cancel,
    deleteModel: remove,
  };
}
