import { AnimatePresence, motion } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { useEvent } from "@/ipc/events";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import {
  audioLevelSchema,
  dictationStartSchema,
  finalSchema,
  injectedSchema,
  EVENT,
} from "@/schemas/events";
import { Waveform } from "@/pages/pill/Waveform";

// 60 s placeholder cap; Phase 8 makes this engine-aware.
const DURATION_CAP_SECONDS = 60;
const FADE_MS = 120;
// PHASE 2 DEBUG ONLY — how long to flash the raw transcript inside the pill
// after `dictation:final` arrives. Removed in Phase 3 along with the
// subscription itself; the steady-state pill never shows transcript text.
const FINAL_DISPLAY_MS = 1500;

function formatDuration(totalSeconds: number): string {
  const mm = Math.floor(totalSeconds / 60)
    .toString()
    .padStart(2, "0");
  const ss = (totalSeconds % 60).toString().padStart(2, "0");
  return `${mm}:${ss}`;
}

/**
 * The pill overlay. Mounts on `dictation:start`, unmounts ~250 ms after
 * `dictation:injected`. Renders the waveform + duration counter only;
 * never renders transcript text (locked in shape brief §3).
 */
export function Pill() {
  const reducedMotion = useReducedMotion();
  const [active, setActive] = useState<boolean>(false);
  const [level, setLevel] = useState<number>(0);
  const [seconds, setSeconds] = useState<number>(0);
  // PHASE 2 DEBUG ONLY — see top-of-file comment.
  const [finalText, setFinalText] = useState<string | null>(null);

  // Stable callbacks so useEvent doesn't re-subscribe on every render.
  const onStart = useCallback(() => {
    setActive(true);
    setSeconds(0);
    setLevel(0);
    setFinalText(null);
  }, []);
  const onLevel = useCallback((p: { rms: number }) => {
    setLevel(p.rms);
  }, []);
  const onInjected = useCallback(() => {
    setActive(false);
    setLevel(0);
  }, []);
  // PHASE 2 DEBUG ONLY — show the transcript in the pill for FINAL_DISPLAY_MS
  // before the `dictation:injected` event hides the pill. Phase 3 removes
  // this entirely (text goes to the OS clipboard + pasted into the focused
  // app instead).
  const onFinal = useCallback((p: { text: string }) => {
    setFinalText(p.text);
  }, []);

  // Refs let useEvent's stable subscriber call into the latest setter
  // without re-subscribing.
  const onStartRef = useRef(onStart);
  const onLevelRef = useRef(onLevel);
  const onInjectedRef = useRef(onInjected);
  const onFinalRef = useRef(onFinal);
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
    onFinalRef.current = onFinal;
  }, [onFinal]);

  useEvent(EVENT.start, dictationStartSchema, () => onStartRef.current());
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => onLevelRef.current(p));
  useEvent(EVENT.injected, injectedSchema, () => onInjectedRef.current());
  useEvent(EVENT.final, finalSchema, (p) => onFinalRef.current(p));

  // PHASE 2 DEBUG ONLY — auto-clear the flashed transcript so a quick
  // second dictation doesn't show stale text.
  useEffect(() => {
    if (finalText === null) return;
    const id = window.setTimeout(() => setFinalText(null), FINAL_DISPLAY_MS);
    return () => window.clearTimeout(id);
  }, [finalText]);

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
          {finalText !== null ? (
            // PHASE 2 DEBUG ONLY — replaced by clipboard injection in Phase 3.
            <span className="truncate text-text" title={finalText}>
              {finalText.length > 0 ? finalText : "(no speech detected)"}
            </span>
          ) : (
            <>
              <div className="flex h-7 w-[72px] items-center">
                <Waveform level={level} reducedMotion={reducedMotion} />
              </div>
              <span aria-hidden="true" className="opacity-50">
                ·
              </span>
              <span className="tabular-nums">
                {formatDuration(Math.min(seconds, 99 * 60 + 59))} / {DURATION_CAP_SECONDS}
              </span>
            </>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
