import { useEffect, useRef } from "react";
import { barTargets, normalizeLevel, processingTargets, smoothLevel } from "./levelMath";

export type WaveMode = "listening" | "processing" | "error";

const BAR_COUNT = 9;
const BAR_WIDTH = 3;
const BAR_GAP = 3;
const MIN_BAR_HEIGHT = 3;
const MAX_BAR_HEIGHT = 22;
/** Per-frame easing of each bar toward its target height. */
const BAR_EASING = 0.35;

// Hex (not oklch CSS vars): canvas fillStyle silently ignores colors it
// can't parse and falls back to black — invisible on the dark pill.
const COLORS: Record<WaveMode, string> = {
  listening: "#f4f1ec",
  processing: "rgba(244, 241, 236, 0.55)",
  error: "#ef5a4c",
};

export const WAVEFORM_WIDTH = BAR_COUNT * BAR_WIDTH + (BAR_COUNT - 1) * BAR_GAP;

interface WaveformProps {
  /** Raw RMS amplitude from Rust (unnormalized). */
  level: number;
  mode: WaveMode;
  /** When true, bars still track the voice level but without decorative wobble. */
  reducedMotion?: boolean;
}

export function Waveform({ level, mode, reducedMotion = false }: WaveformProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const levelRef = useRef(level);
  const modeRef = useRef(mode);
  const reducedRef = useRef(reducedMotion);

  useEffect(() => { levelRef.current = level; }, [level]);
  useEffect(() => { modeRef.current = mode; }, [mode]);
  useEffect(() => { reducedRef.current = reducedMotion; }, [reducedMotion]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const width = WAVEFORM_WIDTH;
    const height = MAX_BAR_HEIGHT;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    const heights = new Float32Array(BAR_COUNT).fill(MIN_BAR_HEIGHT);
    let envelope = 0;
    let raf = 0;

    const tick = (now: number) => {
      const m = modeRef.current;
      const t = reducedRef.current ? 0 : now;

      let targets: number[];
      if (m === "listening") {
        envelope = smoothLevel(envelope, normalizeLevel(levelRef.current));
        targets = barTargets(envelope, t, BAR_COUNT);
      } else if (m === "processing") {
        envelope = 0;
        targets = reducedRef.current
          ? new Array<number>(BAR_COUNT).fill(0.15)
          : processingTargets(now, BAR_COUNT);
      } else {
        envelope = 0;
        targets = new Array<number>(BAR_COUNT).fill(0);
      }

      ctx.clearRect(0, 0, width, height);
      ctx.fillStyle = COLORS[m];
      for (let i = 0; i < BAR_COUNT; i += 1) {
        const target = MIN_BAR_HEIGHT + targets[i] * (MAX_BAR_HEIGHT - MIN_BAR_HEIGHT);
        heights[i] += (target - heights[i]) * BAR_EASING;
        const h = Math.max(MIN_BAR_HEIGHT, heights[i]);
        const x = i * (BAR_WIDTH + BAR_GAP);
        const y = (height - h) / 2;
        ctx.beginPath();
        ctx.roundRect(x, y, BAR_WIDTH, h, BAR_WIDTH / 2);
        ctx.fill();
      }
      raf = requestAnimationFrame(tick);
    };

    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  return <canvas ref={canvasRef} aria-hidden="true" />;
}
