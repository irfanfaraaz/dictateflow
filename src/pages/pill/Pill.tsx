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

export function Pill() {
  const reducedMotion = useReducedMotion();
  const [active, setActive] = useState<boolean>(false);
  const [level, setLevel] = useState<number>(0);

  const onStart = useCallback(() => {
    setActive(true);
    setLevel(0);
  }, []);
  const onLevel = useCallback((p: { rms: number }) => {
    setLevel(p.rms);
  }, []);
  const onInjected = useCallback(() => {
    setLevel(0);
  }, []);
  const onError = useCallback(() => {
    setActive(true);
  }, []);

  const onStartRef = useRef(onStart);
  const onLevelRef = useRef(onLevel);
  const onInjectedRef = useRef(onInjected);
  const onErrorRef = useRef(onError);
  useEffect(() => { onStartRef.current = onStart; }, [onStart]);
  useEffect(() => { onLevelRef.current = onLevel; }, [onLevel]);
  useEffect(() => { onInjectedRef.current = onInjected; }, [onInjected]);
  useEffect(() => { onErrorRef.current = onError; }, [onError]);

  useEvent(EVENT.start, dictationStartSchema, () => onStartRef.current());
  useEvent(EVENT.audioLevel, audioLevelSchema, (p) => onLevelRef.current(p));
  useEvent(EVENT.injected, injectedSchema, () => onInjectedRef.current());
  useEvent(EVENT.error, errorSchema, () => onErrorRef.current());

  return (
    <AnimatePresence>
      {active && (
        <motion.div
          key="pill"
          initial={{ opacity: reducedMotion ? 1 : 0, scale: reducedMotion ? 1 : 0.9 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.9 }}
          transition={
            reducedMotion
              ? { duration: 0 }
              : { duration: FADE_MS / 1000, scale: { type: "spring", stiffness: 400, damping: 25 } }
          }
          className="flex h-[44px] w-[80px] items-center justify-center rounded-full bg-[#1a1a1a]/90 backdrop-blur-md shadow-lg shadow-black/30"
          role="status"
          aria-live="polite"
          aria-label="dictation in progress"
        >
          <Waveform level={level} reducedMotion={reducedMotion} />
        </motion.div>
      )}
    </AnimatePresence>
  );
}
