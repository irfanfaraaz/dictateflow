import { DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";

const LANGUAGES: { code: string; label: string }[] = [
  { code: "auto", label: "auto-detect" },
  { code: "en", label: "english" },
  { code: "es", label: "spanish" },
  { code: "fr", label: "french" },
  { code: "de", label: "german" },
  { code: "it", label: "italian" },
  { code: "pt", label: "portuguese" },
  { code: "nl", label: "dutch" },
  { code: "ja", label: "japanese" },
  { code: "zh", label: "chinese" },
  { code: "ko", label: "korean" },
  { code: "ar", label: "arabic" },
  { code: "hi", label: "hindi" },
];

/**
 * Manual override for whisper's language hint. `auto` lets whisper detect.
 * The catalog is intentionally small — these are the languages whisper
 * itself reports the highest WER on. Phase 9 may grow it from the full
 * ISO list.
 */
export function Language(): JSX.Element {
  const { settings, update } = useSettingsContext();

  return (
    <DossierCard header="language">
      <div className="flex items-center gap-4">
        <label
          htmlFor="language-select"
          className="font-mono text-sm text-muted lowercase"
        >
          dictation language :
        </label>
        <select
          id="language-select"
          value={settings.language}
          onChange={(e) => {
            void update({ language: e.target.value });
          }}
          className="bg-ink border border-rule px-3 py-1 font-mono text-sm text-text"
        >
          {LANGUAGES.map((l) => (
            <option key={l.code} value={l.code}>
              {l.code} — {l.label}
            </option>
          ))}
        </select>
      </div>
      <p className="mt-4 text-muted text-xs">
        auto-detect uses whisper's own language identifier — pick a specific
        code only if detection misroutes you.
      </p>
    </DossierCard>
  );
}
