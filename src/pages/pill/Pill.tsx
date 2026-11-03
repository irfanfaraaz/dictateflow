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
  injectedSchema,
  EVENT,
  type DictationError,
} from "@/schemas/events";
import { Waveform, type WaveMode } from "@/pages/pill/Waveform";
import { countdownLabel, describeError, type PillMessage } from "@/pages/pill/pillCopy";

/** Error / notice pill lingers this long, then fades itself out (toggle
 *  mode never gets a release event to dismiss it). */
const MESSAGE_DISMISS_MS = 2600;

type PillMode = "listening" | "processing" | "success" | "message";

const C = {
  rec: "#ef5a4c",
  muted: "rgba(244,241,236,0.45)",
  ok: "#5ccf86",
  warn: "#f2b24c",
  text: "#f4f1ec",
};

/**
 * Floating dictation pill.
 *
 * Smoothness rules:
 *  - Audio levels (30 Hz) land in a ref; React only re-renders on state
 *    changes (start / stop / end / error) — a handful per dictation.
 *  - The rec dot is animated from the canvas rAF loop via direct style
 *    writes, using the same smoothed envelope as the bars.
 *  - One motion node, never remounted; in on `start`, out on `end`; width
 *    changes (message text) animate via framer `layout`.
 *  - The window is larger than the pill so shadow, spring overshoot and the
 *    widened message state are never clipped.
 */
export function Pill() {
  const reducedMotion = useReducedMotion();
  const [visible, setVisible] = useState(false);
  const [running, setRunning] = useState(false); // rAF loop on/off
  const [mode, setMode] = useState<PillMode>("listening");
  const [message, setMessage] = useState<PillMessage | null>(null);
  const [shakeKey, setShakeKey] = useState(0);
  const [countdown, setCountdown] = useState<string | null>(null);

  const levelRef = useRef(0);
  const ringRef = useRef<HTMLSpanElement | null>(null);
  const coreRef = useRef<HTMLSpanElement | null>(null);
  const dismissTimer = useRef<number | null>(null);
  const capRef = useRef<{ startedAt: number; capSeconds: number } | null>(null);

  const clearDismiss = () => {
    if (dismissTimer.current !== null) window.clearTimeout(dismissTimer.current);
    dismissTimer.current = null;
  };

  const handlers = useRef({
    start: (capSeconds: number) => {
      clearDismiss();
      levelRef.current = 0;
      capRef.current = { startedAt: performance.now(), capSeconds };
      setCountdown(null);
      setMessage(null);
      setMode("listening");
      setRunning(true);
      setVisible(true);
    },
    level: (rms: number) => {
      levelRef.current = rms;
    },
    stop: () => {
      levelRef.current = 0;
      capRef.current = null;
      setCountdown(null);
      setMode((m) => (m === "message" ? m : "processing"));
    },
    injected: () => {
      setMode((m) => (m === "message" ? m : "success"));
    },
    end: () => {
      // Messages keep their own dismiss timer so the user can read them.
      if (dismissTimer.current === null) setVisible(false);
    },
    error: (e: DictationError) => {
      clearDismiss();
      levelRef.current = 0;
      capRef.current = null;
      setCountdown(null);
      const msg = describeError(e);
      setMessage(msg);
      setMode("message");
      if (msg.tone === "error") setShakeKey((k) => k + 1);
      setRunning(true);
      setVisible(true);
      dismissTimer.current = window.setTimeout(() => {
        dismissTimer.current = null;
        setVisible(false);
      }, MESSAGE_DISMISS_MS);
    },
  });

  useEffect(() => clearDismiss, []);

  // The waveform (which drives the dot pulse each frame) isn't mounted in
  // message mode, so reset the pulse explicitly when leaving "listening" —
  // otherwise the ring keeps its last frame as a stray halo.
  useEffect(() => {
    if (mode === "listening") return;
    if (ringRef.current) {
      ringRef.current.style.opacity = "0";
      ringRef.current.style.transform = "scale(1)";
    }
    if (coreRef.current) coreRef.current.style.transform = "scale(1)";
  }, [mode]);

  // Countdown in the last seconds before the engine's duration cap. Ticks
  // only while listening; re-renders only when the label changes.
  useEffect(() => {
    if (!visible || mode !== "listening") return;
    const id = window.setInterval(() => {
      const cap = capRef.current;
      if (!cap) return;
      const elapsed = (performance.now() - cap.startedAt) / 1000;
      const next = countdownLabel(cap.capSeconds, elapsed);
      setCountdown((prev) => (prev === next ? prev : next));
    }, 250);
    return () => window.clearInterval(id);
  }, [visible, mode]);

  useEvent(EVENT.start, dictationStartSchema, (p) => handlers.current.start(p.cap_seconds));
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => handlers.current.level(p.rms));
  useEvent(EVENT.stop, dictationStopSchema, () => handlers.current.stop());
  useEvent(EVENT.injected, injectedSchema, () => handlers.current.injected());
  useEvent(EVENT.end, dictationEndSchema, () => handlers.current.end());
  useEvent(EVENT.error, errorSchema, (p) => handlers.current.error(p));

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

  const waveMode: WaveMode =
    mode === "listening" ? "listening" : mode === "processing" ? "processing" : "idle";
  const label =
    mode === "listening"
      ? countdown
        ? `Listening, ${countdown} left`
        : "Listening"
      : mode === "processing"
        ? "Transcribing"
        : mode === "success"
          ? "Pasted"
          : (message?.text ?? "Dictation error");
  const dotColor =
    mode === "processing"
      ? C.muted
      : mode === "success"
        ? C.ok
        : mode === "message"
          ? message?.tone === "notice"
            ? C.warn
            : C.rec
          : C.rec;

  const hidden = reducedMotion ? { opacity: 0, scale: 1, y: 0 } : { opacity: 0, scale: 0.86, y: 8 };

  return (
    <div className="flex h-full w-full items-center justify-center">
      <motion.div
        initial={hidden}
        animate={visible ? { opacity: 1, scale: 1, y: 0 } : hidden}
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
        {/* Inner node carries the error shake and width animation so they
            never fight the enter/exit spring on the outer node. */}
        <motion.div
          key={shakeKey}
          layout={!reducedMotion}
          animate={
            mode === "message" && message?.tone === "error" && !reducedMotion && shakeKey > 0
              ? { x: [0, -4, 4, -3, 3, 0] }
              : { x: 0 }
          }
          transition={{
            duration: 0.35,
            ease: "easeInOut",
            layout: { type: "spring", stiffness: 520, damping: 38 },
          }}
          className="flex h-[38px] max-w-[300px] items-center gap-[10px] overflow-hidden rounded-full px-[14px]"
          style={{
            background:
              "linear-gradient(180deg, rgba(34,33,36,0.96) 0%, rgba(16,15,17,0.96) 100%)",
            boxShadow:
              "0 0 0 0.5px rgba(255,255,255,0.14) inset, 0 1px 0 rgba(255,255,255,0.06) inset, 0 6px 18px rgba(0,0,0,0.35), 0 0 0 0.5px rgba(0,0,0,0.6)",
          }}
        >
          <motion.span
            layout={!reducedMotion ? "position" : false}
            className="relative flex h-[10px] w-[10px] shrink-0 items-center justify-center"
            aria-hidden="true"
          >
            {mode === "success" ? (
              <CheckIcon color={C.ok} animate={!reducedMotion} />
            ) : (
              <>
                <span
                  ref={ringRef}
                  className="absolute inset-[1px] rounded-full"
                  style={{ background: dotColor, opacity: 0, willChange: "transform, opacity" }}
                />
                <span
                  ref={coreRef}
                  className="relative h-[8px] w-[8px] rounded-full"
                  style={{
                    background: dotColor,
                    transition: "background-color 220ms ease, box-shadow 220ms ease",
                    boxShadow:
                      mode === "listening" || (mode === "message" && message?.tone === "error")
                        ? "0 0 6px rgba(239,90,76,0.55)"
                        : "none",
                    willChange: "transform",
                  }}
                />
              </>
            )}
          </motion.span>

          {mode === "message" && message ? (
            <motion.span
              layout={!reducedMotion ? "position" : false}
              initial={reducedMotion ? false : { opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.18, delay: reducedMotion ? 0 : 0.06 }}
              className="truncate whitespace-nowrap text-[12px] font-medium leading-none"
              style={{
                color: C.text,
                fontFamily: "-apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif",
                letterSpacing: "-0.005em",
              }}
              title={message.text}
            >
              {message.text}
            </motion.span>
          ) : (
            <>
              <Waveform
                levelRef={levelRef}
                mode={waveMode}
                active={running}
                reducedMotion={reducedMotion}
                onFrame={onFrame}
              />
              {countdown && mode === "listening" && (
                <span
                  className="text-[11px] font-medium tabular-nums leading-none"
                  style={{
                    color: C.warn,
                    fontFamily:
                      "-apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif",
                  }}
                  aria-hidden="true"
                >
                  {countdown}
                </span>
              )}
            </>
          )}
        </motion.div>
      </motion.div>
    </div>
  );
}

function CheckIcon({ color, animate }: { color: string; animate: boolean }) {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <motion.path
        d="M1.5 5.2 L4 7.6 L8.6 2.6"
        stroke={color}
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        initial={animate ? { pathLength: 0 } : false}
        animate={{ pathLength: 1 }}
        transition={{ duration: 0.22, ease: "easeOut" }}
      />
    </svg>
  );
}
