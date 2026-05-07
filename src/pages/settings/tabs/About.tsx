import { useEffect, useState } from "react";
import { DefinitionList, DossierCard } from "../components";
import { useSettingsContext } from "../SettingsContext";
import { getPermissions, openPath, resolveLogPath } from "@/ipc/commands";
import type { PermissionsSnapshot } from "@/schemas/settings";

const APP_VERSION = "0.1.0";

export function About(): JSX.Element {
  const { settings } = useSettingsContext();
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
              value: logPath || "~/Library/Logs/wispertype/wispertype.log",
            },
          ]}
        />

        <div className="mt-6 flex flex-wrap gap-x-6 gap-y-2">
          <button
            type="button"
            disabled={!logPath}
            onClick={() => {
              if (!logPath) return;
              void openPath(logPath);
            }}
            className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4 disabled:text-muted disabled:no-underline"
          >
            open log file →
          </button>
        </div>

        <p className="mt-4 text-muted text-xs">
          log file rotation lands in phase 9 — the path exists today even
          if the file is empty.
        </p>
      </DossierCard>
    </div>
  );
}
