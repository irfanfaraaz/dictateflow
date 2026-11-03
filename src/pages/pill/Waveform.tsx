import { useEffect, useRef, type MutableRefObject } from "react";
import { barTargets, ease, normalizeLevel, processingTargets, smoothLevel } from "./levelMath";

export type WaveMode = "listening" | "processing" | "error";

const BAR_COUNT = 9;
const BAR_WIDTH = 3;
const BAR_GAP = 3;
const MIN_BAR_HEIGHT = 3;
const MAX_BAR_HEIGHT = 22;
/** Fraction of the gap each bar closes per 60 Hz frame (time-corrected). */
const BAR_EASING = 0.35;
/** Clamp dt so a stalled frame (window just shown) doesn't teleport bars. */
const MAX_DT_MS = 64;

// Hex (not oklch CSS vars): canvas fillStyle silently ignores colors it
// can't parse and falls back to black — invisible on the dark pill.
const COLORS: Record<WaveMode, string> = {
  listening: "#f4f1ec",
  processing: "rgba(244, 241, 236, 0.55)",
  error: "#ef5a4c",
};

export const WAVEFORM_WIDTH = BAR_COUNT * BAR_WIDTH + (BAR_COUNT - 1) * BAR_GAP;

interface WaveformProps {
  /** Latest raw RMS from Rust. A ref, not a prop, so 30 Hz level events
   *  never trigger React re-renders — the rAF loop just reads it. */
  levelRef: MutableRefObject<number>;
  mode: WaveMode;
  /** False once the pill has faded out — pauses the rAF loop. */
  active: boolean;
  /** Bars still track the voice level, but without decorative wobble. */
  reducedMotion?: boolean;
  /** Called every frame with the smoothed 0..1 envelope (drives the rec dot). */
  onFrame?: (envelope: number) => void;
}

export function Waveform({ levelRef, mode, active, reducedMotion = false, onFrame }: WaveformProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const modeRef = useRef(mode);
  const reducedRef = useRef(reducedMotion);
  const onFrameRef = useRef(onFrame);
  modeRef.current = mode;
  reducedRef.current = reducedMotion;
  onFrameRef.current = onFrame;

  // Canvas sizing is done once; it survives pause/resume of the loop.
  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(WAVEFORM_WIDTH * dpr);
    canvas.height = Math.round(MAX_BAR_HEIGHT * dpr);
    canvas.style.width = `${WAVEFORM_WIDTH}px`;
    canvas.style.height = `${MAX_BAR_HEIGHT}px`;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }, []);

  useEffect(() => {
    if (!active) return;
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    // Fresh state per session: bars start at rest, no stale frame.
    const heights = new Float32Array(BAR_COUNT).fill(MIN_BAR_HEIGHT);
    let envelope = 0;
    let last = performance.now();
    let raf = 0;

    const tick = (now: number) => {
      const dt = Math.min(MAX_DT_MS, Math.max(0, now - last));
      last = now;
      const m = modeRef.current;
      const reduced = reducedRef.current;

      let targets: number[];
      if (m === "listening") {
        envelope = smoothLevel(envelope, normalizeLevel(levelRef.current), dt);
        targets = barTargets(envelope, reduced ? 0 : now, BAR_COUNT);
      } else {
        envelope = smoothLevel(envelope, 0, dt);
        targets =
          m === "processing"
            ? reduced
              ? new Array<number>(BAR_COUNT).fill(0.15)
              : processingTargets(now, BAR_COUNT)
            : new Array<number>(BAR_COUNT).fill(0);
      }
      onFrameRef.current?.(envelope);

      ctx.clearRect(0, 0, WAVEFORM_WIDTH, MAX_BAR_HEIGHT);
      ctx.fillStyle = COLORS[m];
      for (let i = 0; i < BAR_COUNT; i += 1) {
        const target = MIN_BAR_HEIGHT + targets[i] * (MAX_BAR_HEIGHT - MIN_BAR_HEIGHT);
        heights[i] = ease(heights[i], target, BAR_EASING, dt);
        const h = Math.max(MIN_BAR_HEIGHT, heights[i]);
        const x = i * (BAR_WIDTH + BAR_GAP);
        const y = (MAX_BAR_HEIGHT - h) / 2;
        ctx.beginPath();
        ctx.roundRect(x, y, BAR_WIDTH, h, BAR_WIDTH / 2);
        ctx.fill();
      }
      raf = requestAnimationFrame(tick);
    };

    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [active, levelRef]);

  return <canvas ref={canvasRef} aria-hidden="true" />;
}
