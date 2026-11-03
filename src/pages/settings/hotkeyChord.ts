/**
 * Build a global-shortcut binding string from a keyboard event.
 *
 * Uses `KeyboardEvent.code` (the physical key), not `.key`: on macOS
 * Option+A reports key "å", Space reports " ", and Shift+3 reports "#",
 * which produced unparseable bindings ("Alt+Å", "Cmd+ ") or let
 * Cmd+Shift+3 slip past the forbidden-chord check as "Cmd+Shift+#".
 */
export function keyFromCode(code: string): string | null {
  if (!code || code === "Unidentified") return null;
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1] ?? null;
  const digit = /^(?:Digit|Numpad)([0-9])$/.exec(code);
  if (digit) return digit[1] ?? null;
  // Modifier keys alone are never a chord.
  if (/^(Meta|Shift|Alt|Control|OS)(Left|Right)?$/.test(code)) return null;
  // F1…F24, Space, Enter, Minus, BracketLeft, ArrowUp, … — the shortcut
  // parser accepts these W3C code names directly.
  return code;
}

export interface ChordInput {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export function chordFromEvent(e: ChordInput): string | null {
  const key = keyFromCode(e.code);
  if (!key) return null;
  const parts: string[] = [];
  if (e.metaKey) parts.push("Cmd");
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  parts.push(key);
  return parts.join("+");
}
