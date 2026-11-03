import { describe, expect, it } from "vitest";
import { chordFromEvent, keyFromCode } from "./hotkeyChord";

const ev = (code: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) => ({
  code,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe("chordFromEvent", () => {
  it("uses the physical key, not the layout character (Option+A ≠ å)", () => {
    expect(chordFromEvent(ev("KeyA", { altKey: true }))).toBe("Alt+A");
  });

  it("keeps Cmd+Shift+3 recognisable so the forbidden-chord check sees it", () => {
    expect(chordFromEvent(ev("Digit3", { metaKey: true, shiftKey: true }))).toBe("Cmd+Shift+3");
  });

  it("names Space instead of a literal space", () => {
    expect(chordFromEvent(ev("Space", { metaKey: true }))).toBe("Cmd+Space");
  });

  it("passes function keys through", () => {
    expect(chordFromEvent(ev("F5"))).toBe("F5");
  });

  it("orders modifiers Cmd, Ctrl, Alt, Shift", () => {
    expect(
      chordFromEvent(ev("KeyD", { shiftKey: true, altKey: true, ctrlKey: true, metaKey: true })),
    ).toBe("Cmd+Ctrl+Alt+Shift+D");
  });

  it("ignores lone modifier keys", () => {
    expect(chordFromEvent(ev("MetaLeft", { metaKey: true }))).toBeNull();
    expect(keyFromCode("ShiftRight")).toBeNull();
    expect(keyFromCode("")).toBeNull();
  });
});
