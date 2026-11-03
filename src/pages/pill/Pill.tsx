import { motion } from "framer-motion";
import { useRef, useState } from "react";
import { useEvent } from "@/ipc/events";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import {
  audioLevelSchema,
  dictationStartSchema,
  dictationStopSchema,
  errorSchema,
  injectedSchema,
  EVENT,
} from "@/schemas/events";
import { normalizeLevel } from "@/pages/pill/levelMath";
import { Waveform, type WaveMode } from "@/pages/pill/Waveform";

/**
 * Floating dictation pill.
 *
 * The Tauri window (160×64, transparent) is larger than the pill itself so
 * the drop shadow and the spring-in overshoot are never clipped at the
 * window edge — the old 80×44 window == 80×44 pill clipped both.
 *
 * States:
 *  - listening  → red rec dot pulses with voice, bars track live RMS
 *  - processing → hotkey released, engine transcribing: travelling wave
 *  - error      → red bars collapsed + one horizontal shake
 */
export function Pill() {
  const reducedMotion = useReducedMotion();
  const [mode, setMode] = useState<WaveMode>("listening");
  const [level, setLevel] = useState(0);
  // Bumped on every dictation:start so the entrance animation replays even
  // though the window (and this React tree) persists between dictations.
  const [session, setSession] = useState(0);

  const handlers = useRef({
    start: () => {
      setSession((s) => s + 1);
      setMode("listening");
      setLevel(0);
    },
    level: (rms: number) => setLevel(rms),
    stop: () => {
      setMode((m) => (m === "error" ? m : "processing"));
      setLevel(0);
    },
    injected: () => setLevel(0),
    error: () => {
      setMode("error");
      setLevel(0);
    },
  });

  useEvent(EVENT.start, dictationStartSchema, () => handlers.current.start());
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => handlers.current.level(p.rms));
  useEvent(EVENT.stop, dictationStopSchema, () => handlers.current.stop());
  useEvent(EVENT.injected, injectedSchema, () => handlers.current.injected());
  useEvent(EVENT.error, errorSchema, () => handlers.current.error());

  const label =
    mode === "listening" ? "Listening" : mode === "processing" ? "Transcribing" : "Dictation error";

  return (
    <div className="flex h-full w-full items-center justify-center">
      <motion.div
        key={session}
        initial={reducedMotion ? false : { opacity: 0, scale: 0.85, y: 6 }}
        animate={
          mode === "error" && !reducedMotion
            ? { opacity: 1, scale: 1, y: 0, x: [0, -4, 4, -3, 3, 0] }
            : { opacity: 1, scale: 1, y: 0, x: 0 }
        }
        transition={{
          type: "spring",
          stiffness: 520,
          damping: 30,
          mass: 0.7,
          x: { duration: 0.35, ease: "easeInOut" },
        }}
        className="flex h-[38px] items-center gap-[10px] rounded-full px-[14px]"
        style={{
          background: "linear-gradient(180deg, rgba(34,33,36,0.96) 0%, rgba(16,15,17,0.96) 100%)",
          boxShadow:
            "0 0 0 0.5px rgba(255,255,255,0.14) inset, 0 1px 0 rgba(255,255,255,0.06) inset, 0 6px 18px rgba(0,0,0,0.35), 0 0 0 0.5px rgba(0,0,0,0.6)",
        }}
        role="status"
        aria-live="polite"
        aria-label={label}
      >
        <RecDot mode={mode} level={level} reducedMotion={reducedMotion} />
        <Waveform level={level} mode={mode} reducedMotion={reducedMotion} />
      </motion.div>
    </div>
  );
}

function RecDot({
  mode,
  level,
  reducedMotion,
}: {
  mode: WaveMode;
  level: number;
  reducedMotion: boolean;
}) {
  const color = mode === "processing" ? "rgba(244,241,236,0.45)" : "#ef5a4c";
  const pulse = mode === "listening" && !reducedMotion ? 1 + normalizeLevel(level) * 0.45 : 1;
  return (
    <span className="relative flex h-[8px] w-[8px] items-center justify-center" aria-hidden="true">
      {mode === "listening" && !reducedMotion && (
        <span
          className="absolute inset-0 rounded-full"
          style={{
            background: color,
            opacity: 0.35,
            transform: `scale(${1 + normalizeLevel(level) * 1.4})`,
            transition: "transform 90ms ease-out",
          }}
        />
      )}
      <span
        className="relative h-[8px] w-[8px] rounded-full"
        style={{
          background: color,
          transform: `scale(${pulse})`,
          transition: "transform 90ms ease-out, background-color 200ms ease",
          boxShadow: mode === "processing" ? "none" : "0 0 6px rgba(239,90,76,0.55)",
        }}
      />
    </span>
  );
}
