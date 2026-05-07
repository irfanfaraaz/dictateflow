import { DefinitionList, DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";

/**
 * Engine picker — two cards (local / scribe v2). Click radio to switch.
 *
 * The Scribe card stays clickable so users can read the dossier even
 * before Phase 7 lands API-key entry; choosing it without a key just
 * surfaces an EngineError on the next dictation. The stub line under
 * the definition list calls out that the key paste field is Phase 7.
 */
export function Engine(): JSX.Element {
  const { settings, update } = useSettingsContext();
  const isLocal = settings.engine === "local";
  const isScribe = settings.engine === "scribe";

  return (
    <div role="radiogroup" aria-label="transcription engine" className="space-y-6">
      <DossierCard
        header="local / on-device / private"
        active={isLocal}
        onActivate={() => {
          if (!isLocal) void update({ engine: "local" });
        }}
        radioGroup={{ name: "engine" }}
      >
        <DefinitionList
          rows={[
            { term: "mode", value: "local" },
            { term: "inference", value: "on-device" },
            { term: "data", value: "never leaves your device" },
            { term: "requirement", value: "Apple Silicon (M1 or later)" },
          ]}
        />
      </DossierCard>

      <DossierCard
        header="scribe v2 / cloud / 150ms latency / byok"
        active={isScribe}
        onActivate={() => {
          if (!isScribe) void update({ engine: "scribe" });
        }}
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
        <p className="mt-4 pt-4 border-t border-rule text-muted text-xs">
          ElevenLabs Scribe v2 lands in Phase 7 — API key paste field will
          appear here once wired.
        </p>
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
