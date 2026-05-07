import { useRef, useState } from "react";
import { DefinitionList, DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";
import { setApiKey, clearApiKey } from "@/ipc/commands";
import { useModelDownloads } from "@/hooks/useModelDownloads";
import { findModel } from "@/models/catalog";
import type { EngineKind } from "@/schemas/settings";

/**
 * Engine picker — two cards (local / scribe v2). Click radio to switch.
 *
 * The Scribe card hosts a live API-key entry row:
 *   - No key stored  → password input + [save] button.
 *   - Key stored     → redacted display + [rotate key] + [clear].
 */
const ENGINES: EngineKind[] = ["local", "scribe"];

// ---------- ApiKeyRow ----------

interface ApiKeyRowProps {
  hasKey: boolean;
}

function ApiKeyRow({ hasKey }: ApiKeyRowProps): JSX.Element {
  const [rotating, setRotating] = useState(false);
  const [inputValue, setInputValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // When a key is present but the user clicks "rotate key" we switch into
  // input mode. On save (button or blur) the new key is persisted.
  const showInput = !hasKey || rotating;

  async function save() {
    const trimmed = inputValue.trim();
    if (!trimmed) return;
    setSaving(true);
    setError(null);
    try {
      await setApiKey(trimmed);
      // settings:updated event will flip has_api_key → true, re-rendering
      // the parent with hasKey=true, which resets this component.
      setInputValue("");
      setRotating(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  async function clear() {
    setError(null);
    try {
      await clearApiKey();
      setRotating(false);
      setInputValue("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="mt-4 pt-4 border-t border-rule space-y-2">
      {showInput ? (
        <div className="flex items-center gap-2">
          <input
            type="password"
            aria-label="elevenlabs api key"
            placeholder="paste your elevenlabs api key"
            value={inputValue}
            onChange={(e) => setInputValue(e.target.value)}
            onBlur={() => {
              if (inputValue.trim()) void save();
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") void save();
              if (e.key === "Escape") {
                setInputValue("");
                setRotating(false);
              }
            }}
            disabled={saving}
            className="bg-transparent border border-rule px-3 py-2 font-mono text-sm focus:border-accent outline-none w-full"
          />
          <button
            type="button"
            onClick={() => void save()}
            disabled={saving || !inputValue.trim()}
            className="shrink-0 font-mono text-sm border border-rule px-3 py-2 text-fg hover:text-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40"
          >
            [save]
          </button>
        </div>
      ) : (
        <div className="flex items-center gap-3 flex-wrap">
          <span className="font-mono text-sm text-muted" aria-label="api key set">
            •••••••••••
          </span>
          <button
            type="button"
            onClick={() => setRotating(true)}
            className="font-mono text-sm border border-rule px-3 py-2 text-fg hover:text-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
          >
            [rotate key]
          </button>
          <button
            type="button"
            onClick={() => void clear()}
            className="font-mono text-xs border border-rule px-2 py-1 text-muted hover:text-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
          >
            [clear]
          </button>
        </div>
      )}
      {error && (
        <p role="alert" className="font-mono text-xs text-accent">
          {error}
        </p>
      )}
    </div>
  );
}

// ---------- Engine tab ----------

export function Engine(): JSX.Element {
  const { settings, update } = useSettingsContext();
  const { installed } = useModelDownloads();
  const isLocal = settings.engine === "local";
  const isScribe = settings.engine === "scribe";
  const localRef = useRef<HTMLButtonElement | null>(null);
  const scribeRef = useRef<HTMLButtonElement | null>(null);

  function roveTo(target: EngineKind) {
    void update({ engine: target });
    // Move focus too, so the radio that owns tabindex=0 also has the
    // visible focus ring. Defer one tick so React paints the new
    // tabindex before we focus.
    requestAnimationFrame(() => {
      const el = target === "local" ? localRef.current : scribeRef.current;
      el?.focus();
    });
  }

  function onArrow(from: EngineKind): (dir: "next" | "prev") => void {
    return (dir) => {
      const idx = ENGINES.indexOf(from);
      const next =
        ENGINES[
          (idx + (dir === "next" ? 1 : -1) + ENGINES.length) % ENGINES.length
        ];
      if (next) roveTo(next);
    };
  }

  return (
    <div role="radiogroup" aria-label="transcription engine" className="space-y-6">
      <DossierCard
        ref={localRef}
        header="local / on-device / private"
        active={isLocal}
        onActivate={() => {
          if (!isLocal) void update({ engine: "local" });
        }}
        onArrowNav={onArrow("local")}
        radioGroup={{ name: "engine" }}
      >
        <DefinitionList
          rows={[
            { term: "mode", value: "local" },
            { term: "inference", value: "on-device" },
            { term: "data", value: "never leaves your device" },
            { term: "requirement", value: "Apple Silicon (M1 or later)" },
            {
              term: "model",
              value:
                installed.length === 0 ? (
                  <span className="text-muted">
                    (none — install one in models tab)
                  </span>
                ) : (
                  <select
                    aria-label="active whisper model"
                    value={settings.local.model}
                    onChange={(e) =>
                      void update({ local: { model: e.target.value } })
                    }
                    className="bg-transparent border-b border-rule font-mono text-sm text-text focus:border-accent outline-none cursor-pointer"
                  >
                    {installed.map((id) => {
                      const entry = findModel(id);
                      return (
                        <option key={id} value={id}>
                          {entry ? entry.displayName.toLowerCase() : id}
                        </option>
                      );
                    })}
                  </select>
                ),
            },
          ]}
        />
      </DossierCard>

      <DossierCard
        ref={scribeRef}
        header="scribe v2 / cloud / 150ms latency / byok"
        active={isScribe}
        onActivate={() => {
          if (!isScribe) void update({ engine: "scribe" });
        }}
        onArrowNav={onArrow("scribe")}
        radioGroup={{ name: "engine" }}
      >
        <DefinitionList
          rows={[
            { term: "mode", value: "cloud" },
            { term: "latency", value: "~150ms" },
            { term: "data", value: "encrypted in transit and at rest" },
            { term: "security", value: "byok" },
            { term: "region", value: "us-east-1, eu-west-1" },
          ]}
        />
        <ApiKeyRow hasKey={settings.scribe.has_api_key} />
      </DossierCard>

      <a
        href="#"
        onClick={(e) => e.preventDefault()}
        className="inline-block font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
      >
        learn more about engine options
      </a>
    </div>
  );
}
