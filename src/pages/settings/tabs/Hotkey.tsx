import { useEffect, useRef, useState } from "react";
import { DefinitionList, DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";
import type { HotkeyMode } from "@/schemas/settings";

/**
 * Hotkey configuration tab. Phase 8: rebind is live — the captured chord is
 * validated by Rust and the global shortcut is re-registered immediately.
 *
 * Rebind UX: click "rebind …" → row enters "capture" state (accent border) →
 * next keydown (non-modifier) becomes the new binding → committed via
 * `update_settings`. If Rust rejects the chord (forbidden or parse error) an
 * inline error row appears. Esc cancels without changes.
 */
export function Hotkey(): JSX.Element {
  const { settings, update } = useSettingsContext();
  const [capturing, setCapturing] = useState(false);
  const [bindError, setBindError] = useState<string | null>(null);
  const captureRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!capturing) return;
    captureRef.current?.focus();

    function onKey(e: KeyboardEvent) {
      // Escape exits capture without persisting anything.
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
      // Function keys come through as `F1`..`F19`; otherwise uppercase so
      // `a` and `A` collapse onto one binding string.
      const key = /^F\d{1,2}$/.test(e.key) ? e.key : e.key.toUpperCase();
      parts.push(key);
      const binding = parts.join("+");

      setCapturing(false);
      setBindError(null);

      // update() calls `update_settings` which validates + rebinds on the
      // Rust side. On rejection the promise rejects with the error string.
      update({ hotkey: { binding } }).catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : String(err);
        setBindError(msg);
      });
    }

    function onKeyUp() {
      // No-op; kept so a quick Fn-key tap on keyup can't escape the listener.
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
            onClick={() => {
              setBindError(null);
              setCapturing((c) => !c);
            }}
            className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
          >
            {capturing ? "cancel" : "rebind …"}
          </button>
        </div>

        {bindError !== null && (
          <p className="mt-3 font-mono text-xs text-red-400" role="alert">
            {bindError}
          </p>
        )}
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
