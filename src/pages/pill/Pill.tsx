import { motion } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { useEvent } from "@/ipc/events";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import {
  audioLevelSchema,
  dictationEndSchema,
  dictationStartSchema,
  dictationStopSchema,
  errorSchema,
  EVENT,
} from "@/schemas/events";
import { Waveform, type WaveMode } from "@/pages/pill/Waveform";

/** Error pill lingers this long, then fades itself out (toggle mode never
 *  gets a release event to dismiss it). */
const ERROR_DISMISS_MS = 2200;

const DOT_LISTEN = "#ef5a4c";
const DOT_PROCESS = "rgba(244,241,236,0.45)";

/**
 * Floating dictation pill.
 *
 * Smoothness rules:
 *  - Audio levels (30 Hz) land in a ref; React only re-renders on state
 *    changes (start / stop / end / error) — a handful per dictation.
 *  - The rec dot is animated from the canvas rAF loop via direct style
 *    writes, using the same smoothed envelope as the bars.
 *  - One motion node, never remounted; in on `start`, out on `end`.
 *  - The window (160×64, transparent) is larger than the pill so the shadow
 *    and spring overshoot are never clipped.
 */
export function Pill() {
  const reducedMotion = useReducedMotion();
  const [visible, setVisible] = useState(false);
  const [running, setRunning] = useState(false); // rAF loop on/off
  const [mode, setMode] = useState<WaveMode>("listening");
  const [shakeKey, setShakeKey] = useState(0);

  const levelRef = useRef(0);
  const ringRef = useRef<HTMLSpanElement | null>(null);
  const coreRef = useRef<HTMLSpanElement | null>(null);
  const dismissTimer = useRef<number | null>(null);

  const clearDismiss = () => {
    if (dismissTimer.current !== null) window.clearTimeout(dismissTimer.current);
    dismissTimer.current = null;
  };

  const handlers = useRef({
    start: () => {
      clearDismiss();
      levelRef.current = 0;
      setMode("listening");
      setRunning(true);
      setVisible(true);
    },
    level: (rms: number) => {
      levelRef.current = rms;
    },
    stop: () => {
      levelRef.current = 0;
      setMode((m) => (m === "error" ? m : "processing"));
    },
    end: () => {
      // Errors keep their own dismiss timer so the user can read them.
      if (dismissTimer.current === null) setVisible(false);
    },
    error: () => {
      clearDismiss();
      levelRef.current = 0;
      setMode("error");
      setShakeKey((k) => k + 1);
      setRunning(true);
      setVisible(true);
      dismissTimer.current = window.setTimeout(() => {
        dismissTimer.current = null;
        setVisible(false);
      }, ERROR_DISMISS_MS);
    },
  });

  useEffect(() => clearDismiss, []);

  useEvent(EVENT.start, dictationStartSchema, () => handlers.current.start());
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => handlers.current.level(p.rms));
  useEvent(EVENT.stop, dictationStopSchema, () => handlers.current.stop());
  useEvent(EVENT.end, dictationEndSchema, () => handlers.current.end());
  useEvent(EVENT.error, errorSchema, () => handlers.current.error());

  // Per-frame rec-dot pulse — direct DOM writes, no React render.
  const onFrame = useCallback(
    (env: number) => {
      const live = mode === "listening" && !reducedMotion;
      if (ringRef.current) {
        ringRef.current.style.transform = `scale(${live ? 1 + env * 1.4 : 1})`;
        ringRef.current.style.opacity = live ? String(0.12 + env * 0.3) : "0";
      }
      if (coreRef.current) {
        coreRef.current.style.transform = `scale(${live ? 1 + env * 0.35 : 1})`;
      }
    },
    [mode, reducedMotion],
  );

  const label =
    mode === "listening" ? "Listening" : mode === "processing" ? "Transcribing" : "Dictation error";
  const dotColor = mode === "processing" ? DOT_PROCESS : DOT_LISTEN;

  const shown = { opacity: 1, scale: 1, y: 0 };
  const hidden = reducedMotion
    ? { opacity: 0, scale: 1, y: 0 }
    : { opacity: 0, scale: 0.86, y: 8 };

  return (
    <div className="flex h-full w-full items-center justify-center">
      <motion.div
        initial={hidden}
        animate={visible ? shown : hidden}
        transition={
          reducedMotion
            ? { duration: 0.12 }
            : visible
              ? { type: "spring", stiffness: 560, damping: 32, mass: 0.7 }
              : { duration: 0.16, ease: [0.4, 0, 1, 1] }
        }
        onAnimationComplete={() => {
          if (!visible) setRunning(false);
        }}
        style={{ willChange: "transform, opacity" }}
        role="status"
        aria-live="polite"
        aria-label={visible ? label : undefined}
      >
        {/* Inner node carries the error shake so it never fights the
            enter/exit spring on the outer node. */}
        <motion.div
          key={shakeKey}
          animate={
            mode === "error" && !reducedMotion && shakeKey > 0
              ? { x: [0, -4, 4, -3, 3, 0] }
              : { x: 0 }
          }
          transition={{ duration: 0.35, ease: "easeInOut" }}
          className="flex h-[38px] items-center gap-[10px] rounded-full px-[14px]"
          style={{
            background:
              "linear-gradient(180deg, rgba(34,33,36,0.96) 0%, rgba(16,15,17,0.96) 100%)",
            boxShadow:
              "0 0 0 0.5px rgba(255,255,255,0.14) inset, 0 1px 0 rgba(255,255,255,0.06) inset, 0 6px 18px rgba(0,0,0,0.35), 0 0 0 0.5px rgba(0,0,0,0.6)",
          }}
        >
          <span
            className="relative flex h-[8px] w-[8px] items-center justify-center"
            aria-hidden="true"
          >
            <span
              ref={ringRef}
              className="absolute inset-0 rounded-full"
              style={{ background: dotColor, opacity: 0, willChange: "transform, opacity" }}
            />
            <span
              ref={coreRef}
              className="relative h-[8px] w-[8px] rounded-full"
              style={{
                background: dotColor,
                transition: "background-color 220ms ease, box-shadow 220ms ease",
                boxShadow: mode === "processing" ? "none" : "0 0 6px rgba(239,90,76,0.55)",
                willChange: "transform",
              }}
            />
          </span>
          <Waveform
            levelRef={levelRef}
            mode={mode}
            active={running}
            reducedMotion={reducedMotion}
            onFrame={onFrame}
          />
        </motion.div>
      </motion.div>
    </div>
  );
}
