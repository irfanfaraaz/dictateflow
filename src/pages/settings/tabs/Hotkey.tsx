import { useEffect, useRef, useState } from "react";
import { DefinitionList, DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";
import type { HotkeyMode } from "@/schemas/settings";

/**
 * Hotkey configuration. Mode is hold-vs-toggle (live today; the listener
 * still hardcodes F5 hold, but the persisted value drives Phase 8).
 *
 * Rebind UX: focus the rebind row → press the trigger button → row enters
 * "capture" state (red border, focused) → next keydown becomes the new
 * binding string. Phase 8 wires the result into a re-registered global
 * shortcut; Phase 4 only persists the string.
 */
export function Hotkey(): JSX.Element {
  const { settings, update } = useSettingsContext();
  const [capturing, setCapturing] = useState(false);
  const captureRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!capturing) return;
    captureRef.current?.focus();

    function onKey(e: KeyboardEvent) {
      // Escape exits capture without persisting anything — matches the
      // editorial brief's "every action is reachable by keyboard."
      if (e.key === "Escape") {
        e.preventDefault();
        setCapturing(false);
        return;
      }
      // Ignore plain modifier-only keydowns; wait for the actual chord.
      if (e.key === "Meta" || e.key === "Shift" || e.key === "Alt" || e.key === "Control") {
        return;
      }
      e.preventDefault();
      const parts: string[] = [];
      if (e.metaKey) parts.push("Cmd");
      if (e.ctrlKey) parts.push("Ctrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      // Function keys come through as `F1`..`F19`; otherwise uppercase the
      // single character so `a` and `A` collapse onto one binding string.
      const key = /^F\d{1,2}$/.test(e.key) ? e.key : e.key.toUpperCase();
      parts.push(key);
      const binding = parts.join("+");
      void update({ hotkey: { binding } });
      setCapturing(false);
    }

    function onKeyUp() {
      // Listen on keyup as well so a quick tap is captured even if the
      // browser chrome swallows the keydown for an Fn key.
    }

    window.addEventListener("keydown", onKey, { capture: true });
    window.addEventListener("keyup", onKeyUp, { capture: true });
    return () => {
      window.removeEventListener("keydown", onKey, { capture: true });
      window.removeEventListener("keyup", onKeyUp, { capture: true });
    };
  }, [capturing, update]);

  function setMode(mode: HotkeyMode) {
    if (settings.hotkey.mode === mode) return;
    void update({ hotkey: { mode } });
  }

  return (
    <div className="space-y-6">
      <DossierCard header="binding">
        <DefinitionList
          rows={[
            { term: "current", value: settings.hotkey.binding },
            { term: "mode", value: settings.hotkey.mode },
            {
              term: "cap",
              value: `${settings.limits.local_max_seconds}s (local) / ${settings.limits.scribe_max_seconds}s (scribe)`,
            },
          ]}
        />

        <div className="mt-6 flex items-center gap-3">
          <fieldset className="flex border border-rule" aria-label="hotkey mode">
            <legend className="sr-only">hotkey mode</legend>
            <SegButton
              active={settings.hotkey.mode === "hold"}
              onClick={() => setMode("hold")}
            >
              hold
            </SegButton>
            <SegButton
              active={settings.hotkey.mode === "toggle"}
              onClick={() => setMode("toggle")}
            >
              toggle
            </SegButton>
          </fieldset>

          <div
            ref={captureRef}
            tabIndex={-1}
            className={[
              "px-3 py-1 border font-mono text-sm uppercase tracking-wide",
              capturing ? "border-accent text-accent" : "border-rule text-text",
            ].join(" ")}
          >
            {capturing ? "press chord…" : settings.hotkey.binding}
          </div>

          <button
            type="button"
            onClick={() => setCapturing((c) => !c)}
            className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
          >
            {capturing ? "cancel" : "rebind …"}
          </button>
        </div>

        <p className="mt-4 text-muted text-xs">
          re-registering the live hotkey lands in phase 8 — the string above
          is persisted, but F5 stays the active trigger until then.
        </p>
      </DossierCard>
    </div>
  );
}

function SegButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={[
        "px-3 py-1 font-mono text-sm lowercase tracking-wide",
        "border-r border-rule last:border-r-0",
        active ? "bg-accent/20 text-text" : "text-muted hover:text-text",
      ].join(" ")}
    >
      {children}
    </button>
  );
}
