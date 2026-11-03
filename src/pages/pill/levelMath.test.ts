import { describe, expect, it } from "vitest";
import { barTargets, normalizeLevel, smoothLevel } from "./levelMath";

describe("normalizeLevel", () => {
  it("maps silence / mic noise floor to 0", () => {
    expect(normalizeLevel(0)).toBe(0);
    expect(normalizeLevel(0.001)).toBe(0);
  });

  it("maps typical conversational speech (rms 0.02–0.05) to a clearly visible range", () => {
    // Regression: raw sqrt(rms) mapping left speech at ~15% of bar range.
    expect(normalizeLevel(0.02)).toBeGreaterThan(0.4);
    expect(normalizeLevel(0.05)).toBeGreaterThan(0.6);
  });

  it("is monotonic and clamped to [0, 1]", () => {
    let prev = -1;
    for (const rms of [0.003, 0.01, 0.03, 0.1, 0.3, 1, 5]) {
      const v = normalizeLevel(rms);
      expect(v).toBeGreaterThanOrEqual(prev);
      expect(v).toBeLessThanOrEqual(1);
      prev = v;
    }
    expect(normalizeLevel(1)).toBe(1);
  });

  it("treats NaN / negative input as silence", () => {
    expect(normalizeLevel(Number.NaN)).toBe(0);
    expect(normalizeLevel(-0.5)).toBe(0);
  });
});

describe("smoothLevel", () => {
  it("attacks faster than it releases", () => {
    const up = smoothLevel(0, 1);
    const down = 1 - smoothLevel(1, 0);
    expect(up).toBeGreaterThan(down);
  });
});

describe("barTargets", () => {
  const COUNT = 9;

  it("keeps every bar near the floor when silent", () => {
    const t = barTargets(0, 1234, COUNT);
    expect(t).toHaveLength(COUNT);
    for (const v of t) expect(v).toBeLessThan(0.2);
  });

  it("moves EVERY bar (including edges) well above the floor when speaking", () => {
    // Regression: edge weights were exactly 0, so the outer bars never moved.
    const t = barTargets(0.8, 500, COUNT);
    for (const v of t) expect(v).toBeGreaterThan(0.25);
    // Center should be the tallest region.
    expect(Math.max(...t)).toBeGreaterThan(0.6);
  });

  it("bars move independently over time (not a static shape)", () => {
    const a = barTargets(0.7, 0, COUNT);
    const b = barTargets(0.7, 250, COUNT);
    const diff = a.reduce((s, v, i) => s + Math.abs(v - b[i]), 0);
    expect(diff).toBeGreaterThan(0.3);
  });

  it("always stays within [0, 1]", () => {
    for (let ms = 0; ms < 5000; ms += 37) {
      for (const v of barTargets(1, ms, COUNT)) {
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThanOrEqual(1);
      }
    }
  });
});
