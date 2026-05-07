import { useEffect, useRef } from "react";

const BAR_COUNT = 5;
const BAR_WIDTH = 3;
const BAR_GAP = 3;
const MIN_BAR_HEIGHT = 4;
const MAX_BAR_HEIGHT = 24;
const EASING = 0.3;

function resolveColor(prop: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(prop).trim() || "#ffffff";
}

interface WaveformProps {
  level: number;
  reducedMotion?: boolean;
}

export function Waveform({ level, reducedMotion = false }: WaveformProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const heightsRef = useRef<Float32Array>(new Float32Array(BAR_COUNT));
  const targetsRef = useRef<Float32Array>(new Float32Array(BAR_COUNT));
  const levelRef = useRef<number>(level);
  const rafRef = useRef<number | null>(null);

  useEffect(() => {
    levelRef.current = level;
  }, [level]);

  useEffect(() => {
    if (reducedMotion) return;
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

    const color = resolveColor("--accent");

    const tick = () => {
      const lvl = Math.max(0, Math.min(1, levelRef.current));
      const shaped = Math.sqrt(lvl);
      const targets = targetsRef.current;
      for (let i = 0; i < BAR_COUNT; i += 1) {
        const center = (BAR_COUNT - 1) / 2;
        const dist = 1 - Math.abs(i - center) / center;
        const phase = (i / BAR_COUNT) * Math.PI * 2;
        const j = 0.5 + 0.5 * Math.abs(Math.sin(phase + performance.now() / 180));
        targets[i] = MIN_BAR_HEIGHT + shaped * j * dist * (MAX_BAR_HEIGHT - MIN_BAR_HEIGHT);
      }

      const heights = heightsRef.current;
      for (let i = 0; i < BAR_COUNT; i += 1) {
        heights[i] += (targets[i] - heights[i]) * EASING;
      }

      ctx.clearRect(0, 0, width, height);
      ctx.fillStyle = color;
      for (let i = 0; i < BAR_COUNT; i += 1) {
        const h = Math.max(MIN_BAR_HEIGHT, heights[i]);
        const w = BAR_WIDTH;
        const x = i * (BAR_WIDTH + BAR_GAP);
        const y = (height - h) / 2;
        const r = w / 2;
        ctx.beginPath();
        ctx.roundRect(x, y, w, h, r);
        ctx.fill();
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
        className="inline-block h-2 w-2 rounded-full bg-white"
      />
    );
  }

  return <canvas ref={canvasRef} aria-hidden="true" />;
}
