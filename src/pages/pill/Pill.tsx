import { AnimatePresence, motion } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { useEvent } from "@/ipc/events";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import {
  audioLevelSchema,
  dictationStartSchema,
  errorSchema,
  injectedSchema,
  EVENT,
} from "@/schemas/events";
import { Waveform } from "@/pages/pill/Waveform";

const FADE_MS = 120;
/** Error chip auto-dismiss after 3 seconds. */
const ERROR_DISMISS_MS = 3000;

function formatDuration(totalSeconds: number): string {
  const mm = Math.floor(totalSeconds / 60)
    .toString()
    .padStart(2, "0");
  const ss = (totalSeconds % 60).toString().padStart(2, "0");
  return `${mm}:${ss}`;
}

/**
 * The pill overlay. Mounts on `dictation:start` OR `dictation:error`.
 *
 * While dictating: live waveform + duration counter.
 * On `dictation:injected`: swap counter for "pasting…" beat while fading.
 * On `dictation:error`: pill auto-shows with error chip for 3 s, then hides.
 *   — Chip is red 1-line text, truncated. Height animates from 56→80 px
 *     (or snaps with reduced-motion). Dismisses automatically after 3 s.
 *
 * Phase 8: `cap_seconds` arrives in the start payload. Duration is displayed
 * as `mm:ss / cap`. At 80% of cap the duration text turns bold + the waveform
 * brightens red. At 100% the Rust cap timer fires `audio_stop`, which ends
 * dictation via the normal stop path — no extra logic needed here.
 *
 * Per shape brief §3, the pill NEVER renders transcript text.
 */
export function Pill() {
  const reducedMotion = useReducedMotion();
  const [active, setActive] = useState<boolean>(false);
  const [level, setLevel] = useState<number>(0);
  const [seconds, setSeconds] = useState<number>(0);
  const [pasting, setPasting] = useState<boolean>(false);
  // Engine-aware cap received from Rust on start. Defaults to 60 s until the
  // first event arrives (matches the local engine default).
  const [capSeconds, setCapSeconds] = useState<number>(60);

  // Error chip state. `errorMsg` is non-null when the chip is visible.
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const errorTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const dismissError = useCallback(() => {
    setErrorMsg(null);
    // If no active dictation was running, also collapse the pill visibility.
    setActive((prev) => prev); // no-op; let Rust hide the window
  }, []);

  const showError = useCallback(
    (msg: string) => {
      // Restart the dismiss timer on each new error.
      if (errorTimerRef.current !== null) {
        clearTimeout(errorTimerRef.current);
      }
      setErrorMsg(msg);
      // Show the pill window even if no dictation is active.
      setActive(true);
      errorTimerRef.current = setTimeout(() => {
        dismissError();
        setActive(false);
      }, ERROR_DISMISS_MS);
    },
    [dismissError],
  );

  // Clean up timer on unmount.
  useEffect(() => {
    return () => {
      if (errorTimerRef.current !== null) {
        clearTimeout(errorTimerRef.current);
      }
    };
  }, []);

  // Stable callbacks so useEvent doesn't re-subscribe on every render.
  const onStart = useCallback((p: { trigger: string; cap_seconds: number }) => {
    setActive(true);
    setSeconds(0);
    setLevel(0);
    setPasting(false);
    setErrorMsg(null);
    setCapSeconds(p.cap_seconds);
  }, []);
  const onLevel = useCallback((p: { rms: number }) => {
    setLevel(p.rms);
  }, []);
  // On `dictation:injected`, swap the counter for a brief "pasting…"
  // beat. We do NOT clear `active` here — Rust hides the pill window
  // ~250 ms later (PILL_HIDE_DELAY in events.rs), which is what visually
  // ends the pill. Keeping `active=true` ensures the swap renders.
  const onInjected = useCallback(() => {
    setPasting(true);
    setLevel(0);
  }, []);

  const onError = useCallback(
    (p: { message: string }) => {
      showError(p.message);
    },
    [showError],
  );

  // Refs let useEvent's stable subscriber call into the latest setter
  // without re-subscribing.
  const onStartRef = useRef(onStart);
  const onLevelRef = useRef(onLevel);
  const onInjectedRef = useRef(onInjected);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onStartRef.current = onStart;
  }, [onStart]);
  useEffect(() => {
    onLevelRef.current = onLevel;
  }, [onLevel]);
  useEffect(() => {
    onInjectedRef.current = onInjected;
  }, [onInjected]);
  useEffect(() => {
    onErrorRef.current = onError;
  }, [onError]);

  useEvent(EVENT.start, dictationStartSchema, (p) => onStartRef.current(p));
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => onLevelRef.current(p));
  useEvent(EVENT.injected, injectedSchema, () => onInjectedRef.current());
  useEvent(EVENT.error, errorSchema, (p) => onErrorRef.current(p));

  // Duration counter: tick once per second while active.
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => {
      setSeconds((s) => s + 1);
    }, 1000);
    return () => window.clearInterval(id);
  }, [active]);

  // 80% warning threshold — computed once per render (cheap).
  const nearCap = capSeconds > 0 && seconds / capSeconds >= 0.8;

  // Pill height: 56px baseline, grows to 80px when error chip is showing.
  // Reduced-motion: snap height, no spring.
  const pillHeight = errorMsg ? 80 : 56;

  return (
    <AnimatePresence>
      {active && (
        <motion.div
          key="pill"
          initial={{ opacity: reducedMotion ? 1 : 0, height: 56 }}
          animate={{ opacity: 1, height: pillHeight }}
          exit={{ opacity: reducedMotion ? 0 : 0 }}
          transition={
            reducedMotion
              ? { duration: 0 }
              : { duration: FADE_MS / 1000, height: { type: "spring", stiffness: 300, damping: 30 } }
          }
          className="flex w-[280px] flex-col justify-center gap-0 overflow-hidden border border-rule bg-ink px-4 font-mono text-[13px] text-muted"
          role="status"
          aria-live="polite"
          aria-label={errorMsg ? `error: ${errorMsg}` : "dictation in progress"}
        >
          {/* Main row: waveform + duration/pasting (hidden in error-only mode) */}
          {!errorMsg && (
            <div className="flex items-center gap-3">
              <div className="flex h-7 w-[72px] items-center">
                <Waveform level={level} reducedMotion={reducedMotion} warn={nearCap} />
              </div>
              <span aria-hidden="true" className="opacity-50">
                ·
              </span>
              {pasting ? (
                <span className="tabular-nums">pasting…</span>
              ) : (
                <span
                  className={[
                    "tabular-nums",
                    nearCap ? "font-bold text-accent-warn" : "",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                >
                  {formatDuration(Math.min(seconds, 99 * 60 + 59))} / {capSeconds}
                </span>
              )}
            </div>
          )}

          {/* Error chip — 1-line, red, auto-dismisses after 3 s */}
          {errorMsg && (
            <div
              className="flex w-full items-center"
              role="alert"
              aria-atomic="true"
            >
              {/* TODO(impeccable-craft): consider a distinct --error token if error vs. warn semantics diverge in craft pass */}
              <span className="w-full truncate text-[12px] text-accent-warn">
                {errorMsg}
              </span>
            </div>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
