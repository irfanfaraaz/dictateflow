/**
 * Pure math behind the pill waveform. Kept free of DOM/canvas so it can be
 * unit-tested (see levelMath.test.ts).
 *
 * Why this exists: the Rust side sends *raw* RMS amplitude. Normal speech on
 * a laptop mic sits around 0.02–0.05 RMS, so a linear (or sqrt) mapping only
 * ever used ~15% of the bar height — the waveform looked frozen. Human
 * loudness perception is logarithmic, so we map through decibels instead.
 */

/** Below this (≈ -54 dBFS) we treat input as silence / mic noise floor. */
const DB_FLOOR = -54;
/** At or above this (≈ -18 dBFS) bars are fully extended. */
const DB_CEIL = -18;

/** Map raw RMS amplitude to a perceptual 0..1 level. */
export function normalizeLevel(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0;
  const db = 20 * Math.log10(rms);
  const t = (db - DB_FLOOR) / (DB_CEIL - DB_FLOOR);
  return Math.max(0, Math.min(1, t));
}

const FRAME_60 = 1000 / 60;

/**
 * Frame-rate-independent exponential ease toward `target`.
 * `k60` is the fraction of the gap closed per 60 Hz frame; for any `dtMs`
 * the result equals applying that per-frame step `dtMs / 16.67` times, so
 * motion feels identical on 60 Hz and 120 Hz (ProMotion) displays.
 */
export function ease(prev: number, target: number, k60: number, dtMs: number): number {
  if (dtMs <= 0) return prev;
  const k = 1 - Math.pow(1 - k60, dtMs / FRAME_60);
  return prev + (target - prev) * k;
}

/** Fast attack / slow release envelope — like a VU meter. */
const ATTACK = 0.55;
const RELEASE = 0.12;
export function smoothLevel(prev: number, target: number, dtMs: number = FRAME_60): number {
  return ease(prev, target, target > prev ? ATTACK : RELEASE, dtMs);
}

/** Resting height (fraction of max) and the gentle "breathing" on top of it. */
const IDLE_BASE = 0.07;
const IDLE_BREATH = 0.04;
/** Minimum share of the level that edge bars receive (old code used 0). */
const EDGE_WEIGHT = 0.5;
/** Lowest value of the per-bar wobble multiplier. */
const MOD_FLOOR = 0.45;

/**
 * Per-bar target heights in 0..1.
 *
 * - `level`  — smoothed, normalized level (0..1)
 * - `ms`     — current time in ms (drives independent per-bar motion)
 * - `count`  — number of bars
 */
export function barTargets(level: number, ms: number, count: number): number[] {
  const out = new Array<number>(count);
  const lvl = Math.max(0, Math.min(1, level));
  const center = (count - 1) / 2;
  for (let i = 0; i < count; i += 1) {
    // Bell-shaped weight: 1 at center, EDGE_WEIGHT at the edges.
    const x = center === 0 ? 0 : (i - center) / center; // -1..1
    const bell = 0.5 + 0.5 * Math.cos(Math.PI * x);
    const weight = EDGE_WEIGHT + (1 - EDGE_WEIGHT) * bell;

    // Two incommensurate sines per bar → organic, non-repeating motion,
    // with each bar on its own frequency so they move independently.
    const f1 = 0.011 + 0.0023 * ((i * 7) % 5);
    const f2 = 0.0047 + 0.0019 * ((i * 3) % 4);
    const n = 0.5 + 0.25 * Math.sin(ms * f1 + i * 1.7) + 0.25 * Math.sin(ms * f2 + i * 0.9);
    const mod = MOD_FLOOR + (1 - MOD_FLOOR) * n;

    const idle = IDLE_BASE + IDLE_BREATH * (0.5 + 0.5 * Math.sin(ms / 520 + i * 0.7));
    const v = idle + (1 - idle) * lvl * weight * mod;
    out[i] = Math.max(0, Math.min(1, v));
  }
  return out;
}

/**
 * "Processing" animation (after the hotkey is released, while the engine
 * transcribes): a soft wave that travels left → right across the bars.
 */
export function processingTargets(ms: number, count: number): number[] {
  const out = new Array<number>(count);
  for (let i = 0; i < count; i += 1) {
    const phase = ms / 160 - i * 0.65;
    out[i] = 0.12 + 0.3 * Math.max(0, Math.sin(phase)) ** 2;
  }
  return out;
}
