import { describe, expect, it } from "vitest";
import { countdownLabel, describeError, MAX_PILL_TEXT } from "./pillCopy";

describe("describeError", () => {
  it("uses the backend message when it is short", () => {
    expect(describeError({ code: "audio", message: "No microphone access" })).toEqual({
      text: "No microphone access",
      tone: "error",
    });
  });

  it("falls back to friendly copy for long / technical messages", () => {
    const long = "start audio capture: CoreAudio error -10851 kAudioUnitErr_InvalidPropertyValue at ...";
    const got = describeError({ code: "start_failed", message: long });
    expect(got.text.length).toBeLessThanOrEqual(MAX_PILL_TEXT);
    expect(got.text).not.toContain("CoreAudio");
  });

  it("maps known codes even with an empty message", () => {
    expect(describeError({ code: "missing_api_key", message: "" }).text).toMatch(/API key/i);
  });

  it("treats recoverable outcomes (text is on the clipboard) as notices, not errors", () => {
    expect(describeError({ code: "focus_changed", message: "" }).tone).toBe("notice");
    expect(describeError({ code: "accessibility_denied", message: "" }).tone).toBe("notice");
    expect(describeError({ code: "websocket", message: "" }).tone).toBe("error");
  });

  it("never returns empty text", () => {
    expect(describeError({ code: "???", message: "" }).text.length).toBeGreaterThan(0);
  });
});

describe("countdownLabel", () => {
  it("is hidden until the last 10 seconds", () => {
    expect(countdownLabel(60, 0)).toBeNull();
    expect(countdownLabel(60, 49.9)).toBeNull();
  });

  it("counts down m:ss in the last 10 seconds", () => {
    expect(countdownLabel(60, 50.2)).toBe("0:10");
    expect(countdownLabel(60, 58.5)).toBe("0:02");
  });

  it("disappears at / after the cap and with no cap", () => {
    expect(countdownLabel(60, 60)).toBeNull();
    expect(countdownLabel(60, 75)).toBeNull();
    expect(countdownLabel(0, 5)).toBeNull();
  });
});
