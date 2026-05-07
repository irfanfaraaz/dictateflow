import { useEffect, useState } from "react";
import { DefinitionList, DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";
import { getPermissions, openPath, resolveLogPath, openLogDir } from "@/ipc/commands";
import type { PermissionsSnapshot } from "@/schemas/settings";

const APP_VERSION = "0.1.0";
const RELEASES_URL = "https://github.com/irfanfaraaz/dictateflow/releases";

export function About(): JSX.Element {
  const { settings, update } = useSettingsContext();
  const [perms, setPerms] = useState<PermissionsSnapshot | null>(null);
  const [logPath, setLogPath] = useState<string>("");

  useEffect(() => {
    let cancelled = false;
    void getPermissions()
      .then((s) => {
        if (!cancelled) setPerms(s);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error("[about] permissions fetch failed", err);
      });
    void resolveLogPath()
      .then((p) => {
        if (!cancelled) setLogPath(p);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error("[about] log path resolve failed", err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const engineLabel =
    settings.engine === "scribe"
      ? "scribe-v2"
      : `ggml-${settings.local.model}`;

  return (
    <div className="space-y-6">
      <DossierCard header="about">
        <DefinitionList
          rows={[
            { term: "version", value: APP_VERSION },
            { term: "engine", value: engineLabel },
            { term: "mic", value: perms?.microphone ?? "…" },
            { term: "accessibility", value: perms?.accessibility ?? "…" },
            { term: "input mon.", value: perms?.input_monitoring ?? "…" },
            {
              term: "logs",
              value: "~/Library/Logs/dictateflow/ (rotating daily)",
            },
          ]}
        />

        <div className="mt-6 flex flex-wrap gap-x-6 gap-y-2">
          <button
            type="button"
            onClick={() => {
              void openPath(RELEASES_URL).catch((err) => {
                // eslint-disable-next-line no-console
                console.error("[about] open releases failed", err);
              });
            }}
            className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
          >
            check for updates →
          </button>
          <button
            type="button"
            disabled={!logPath}
            onClick={() => {
              void openLogDir().catch((err) => {
                // eslint-disable-next-line no-console
                console.error("[about] open log dir failed", err);
              });
            }}
            className="font-mono text-sm uppercase tracking-wide text-muted underline underline-offset-4 disabled:no-underline"
          >
            open log folder →
          </button>
          <button
            type="button"
            disabled={!logPath}
            onClick={() => {
              if (!logPath) return;
              void openPath(logPath).catch((err) => {
                // eslint-disable-next-line no-console
                console.error("[about] open log file failed", err);
              });
            }}
            className="font-mono text-sm uppercase tracking-wide text-muted underline underline-offset-4 disabled:no-underline"
          >
            open log file →
          </button>
        </div>
      </DossierCard>

      {/* Sounds sub-section */}
      <DossierCard header="sounds">
        <p className="mb-4 text-xs text-muted">
          optional audible feedback using macOS system sounds. off by default.
        </p>
        <div className="space-y-3">
          <label className="flex items-center gap-3 cursor-pointer">
            <input
              type="checkbox"
              checked={settings.sounds.start_enabled}
              onChange={(e) => {
                void update({ sounds: { start_enabled: e.target.checked } }).catch(
                  (err) => {
                    // eslint-disable-next-line no-console
                    console.error("[about] sounds update failed", err);
                  },
                );
              }}
              className="h-4 w-4 rounded border-muted accent-accent"
            />
            <span className="font-mono text-sm">play sound on start</span>
          </label>
          <label className="flex items-center gap-3 cursor-pointer">
            <input
              type="checkbox"
              checked={settings.sounds.stop_enabled}
              onChange={(e) => {
                void update({ sounds: { stop_enabled: e.target.checked } }).catch(
                  (err) => {
                    // eslint-disable-next-line no-console
                    console.error("[about] sounds update failed", err);
                  },
                );
              }}
              className="h-4 w-4 rounded border-muted accent-accent"
            />
            <span className="font-mono text-sm">play sound on stop</span>
          </label>
        </div>
      </DossierCard>
    </div>
  );
}
