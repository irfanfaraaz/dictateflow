import { AnimatePresence, motion } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { useEvent } from "@/ipc/events";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import {
  audioLevelSchema,
  dictationStartSchema,
  injectedSchema,
  EVENT,
} from "@/schemas/events";
import { Waveform } from "@/pages/pill/Waveform";

// 60 s placeholder cap; Phase 8 makes this engine-aware.
const DURATION_CAP_SECONDS = 60;
const FADE_MS = 120;

function formatDuration(totalSeconds: number): string {
  const mm = Math.floor(totalSeconds / 60)
    .toString()
    .padStart(2, "0");
  const ss = (totalSeconds % 60).toString().padStart(2, "0");
  return `${mm}:${ss}`;
}

/**
 * The pill overlay. Mounts on `dictation:start`. While dictating, shows
 * a live waveform + duration counter. On `dictation:injected`, swaps the
 * counter for a "pasting…" beat while it fades out (~250 ms total).
 *
 * Per shape brief §3, the pill NEVER renders transcript text. The
 * `dictation:final` event still fires from Rust for downstream consumers,
 * but the pill does not subscribe to it.
 */
export function Pill() {
  const reducedMotion = useReducedMotion();
  const [active, setActive] = useState<boolean>(false);
  const [level, setLevel] = useState<number>(0);
  const [seconds, setSeconds] = useState<number>(0);
  const [pasting, setPasting] = useState<boolean>(false);

  // Stable callbacks so useEvent doesn't re-subscribe on every render.
  const onStart = useCallback(() => {
    setActive(true);
    setSeconds(0);
    setLevel(0);
    setPasting(false);
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

  // Refs let useEvent's stable subscriber call into the latest setter
  // without re-subscribing.
  const onStartRef = useRef(onStart);
  const onLevelRef = useRef(onLevel);
  const onInjectedRef = useRef(onInjected);
  useEffect(() => {
    onStartRef.current = onStart;
  }, [onStart]);
  useEffect(() => {
    onLevelRef.current = onLevel;
  }, [onLevel]);
  useEffect(() => {
    onInjectedRef.current = onInjected;
  }, [onInjected]);

  useEvent(EVENT.start, dictationStartSchema, () => onStartRef.current());
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => onLevelRef.current(p));
  useEvent(EVENT.injected, injectedSchema, () => onInjectedRef.current());

  // Duration counter: tick once per second while active.
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => {
      setSeconds((s) => s + 1);
    }, 1000);
    return () => window.clearInterval(id);
  }, [active]);

  return (
    <AnimatePresence>
      {active && (
        <motion.div
          key="pill"
          initial={{ opacity: reducedMotion ? 1 : 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: reducedMotion ? 0 : 0 }}
          transition={{ duration: reducedMotion ? 0 : FADE_MS / 1000 }}
          className="flex h-[56px] w-[280px] items-center gap-3 border border-rule bg-ink px-4 font-mono text-[13px] text-muted"
          role="status"
          aria-live="polite"
          aria-label="dictation in progress"
        >
          <div className="flex h-7 w-[72px] items-center">
            <Waveform level={level} reducedMotion={reducedMotion} />
          </div>
          <span aria-hidden="true" className="opacity-50">
            ·
          </span>
          {pasting ? (
            <span className="tabular-nums">pasting…</span>
          ) : (
            <span className="tabular-nums">
              {formatDuration(Math.min(seconds, 99 * 60 + 59))} / {DURATION_CAP_SECONDS}
            </span>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
