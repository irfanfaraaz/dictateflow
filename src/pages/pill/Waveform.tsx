import { useEffect, useRef } from "react";

const BAR_COUNT = 12;
const BAR_WIDTH = 2;
const BAR_GAP = 4;
const MIN_BAR_HEIGHT = 3;
const MAX_BAR_HEIGHT = 28;
// Per shape brief: Tailwind `accent` token = oklch(0.62 0.20 25).
const BAR_COLOR = "oklch(0.62 0.20 25)";
// Per-frame easing factor for the bar-height interpolation. Lower = smoother
// (more visual lag), higher = snappier. 0.25 lands close to a 60 Hz spring
// without the cost of a real solver.
const EASING = 0.25;

interface WaveformProps {
  /** Current RMS amplitude in [0, ~1]. */
  level: number;
  /** When true, render a static red dot instead of bars. */
  reducedMotion?: boolean;
}

/**
 * Animated waveform: 12 vertical bars whose heights interpolate toward a
 * shared target derived from the latest `level`. The target is shaped via a
 * sqrt curve so quiet input still moves the bars visibly, then jittered per
 * bar so the result reads as "audio energy" instead of "pulse."
 *
 * We draw to a `<canvas>` rather than 12 styled divs because a single rAF
 * loop is cheaper (and stays inside the GPU compositor) than 12 React
 * style updates per frame.
 */
export function Waveform({ level, reducedMotion = false }: WaveformProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const heightsRef = useRef<Float32Array>(new Float32Array(BAR_COUNT));
  const targetsRef = useRef<Float32Array>(new Float32Array(BAR_COUNT));
  const levelRef = useRef<number>(level);
  const rafRef = useRef<number | null>(null);

  // Push the latest level into a ref so the rAF loop can read it without
  // re-subscribing every render.
  useEffect(() => {
    levelRef.current = level;
  }, [level]);

  useEffect(() => {
    if (reducedMotion) return; // static dot path
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const width = (BAR_COUNT - 1) * BAR_GAP + BAR_COUNT * BAR_WIDTH;
    const height = MAX_BAR_HEIGHT;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    ctx.scale(dpr, dpr);

    const tick = () => {
      const lvl = Math.max(0, Math.min(1, levelRef.current));
      // sqrt curve: small lvl values still produce visible heights.
      const shaped = Math.sqrt(lvl);
      const targets = targetsRef.current;
      for (let i = 0; i < BAR_COUNT; i += 1) {
        // Per-bar jitter so the waveform doesn't look like one big bar.
        // Use a deterministic phase offset so reduced-anxiety viewers
        // still see a coherent shape, not noise.
        const phase = (i / BAR_COUNT) * Math.PI * 2;
        const j = 0.6 + 0.4 * Math.abs(Math.sin(phase + performance.now() / 220));
        targets[i] = MIN_BAR_HEIGHT + shaped * j * (MAX_BAR_HEIGHT - MIN_BAR_HEIGHT);
      }

      const heights = heightsRef.current;
      for (let i = 0; i < BAR_COUNT; i += 1) {
        heights[i] += (targets[i] - heights[i]) * EASING;
      }

      ctx.clearRect(0, 0, width, height);
      ctx.fillStyle = BAR_COLOR;
      for (let i = 0; i < BAR_COUNT; i += 1) {
        const h = Math.max(MIN_BAR_HEIGHT, heights[i]);
        const x = i * (BAR_WIDTH + BAR_GAP);
        const y = (height - h) / 2;
        ctx.fillRect(x, y, BAR_WIDTH, h);
      }
      rafRef.current = requestAnimationFrame(tick);
    };

    rafRef.current = requestAnimationFrame(tick);
    return () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    };
  }, [reducedMotion]);

  if (reducedMotion) {
    return (
      <span
        aria-hidden="true"
        className="inline-block h-2 w-2 rounded-full"
        style={{ background: BAR_COLOR }}
      />
    );
  }

  return <canvas ref={canvasRef} aria-hidden="true" />;
}
