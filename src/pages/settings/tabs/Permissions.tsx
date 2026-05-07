import { useEffect, useState } from "react";
import { DossierCard } from "../components";
import { getPermissions, openSystemSettings } from "@/ipc/commands";
import type {
  PermissionKind,
  PermissionState,
  PermissionsSnapshot,
} from "@/schemas/settings";

const POLL_INTERVAL_MS = 2000;

interface Row {
  kind: PermissionKind;
  label: string;
  reason: string;
  state: PermissionState;
}

/**
 * Permissions tab. Polls Rust every 2 s while mounted so the tab reflects
 * grants the user makes mid-session in System Settings (TCC doesn't post
 * any cross-process notification we can subscribe to).
 *
 * Color is never the only signal — each row carries an explicit textual
 * status (`granted` / `denied` / `not requested`) and a CTA when action
 * is required.
 */
export function Permissions(): JSX.Element {
  const [snap, setSnap] = useState<PermissionsSnapshot | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setInterval> | null = null;

    async function tick() {
      try {
        const next = await getPermissions();
        if (!cancelled) setSnap(next);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("[permissions] poll failed", err);
      }
    }

    void tick();
    timer = setInterval(() => void tick(), POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      if (timer) clearInterval(timer);
    };
  }, []);

  const rows: Row[] = [
    {
      kind: "microphone",
      label: "microphone",
      reason: "audio capture during dictation",
      state: snap?.microphone ?? "not_requested",
    },
    {
      kind: "accessibility",
      label: "accessibility",
      reason: "synthesise ⌘V to paste transcript",
      state: snap?.accessibility ?? "not_requested",
    },
    {
      kind: "input_monitoring",
      label: "input monitoring",
      reason: "global hotkey on F-row keys",
      state: snap?.input_monitoring ?? "not_requested",
    },
  ];

  return (
    <DossierCard header="permissions">
      <ul className="divide-y divide-rule">
        {rows.map((r) => (
          <PermissionRow key={r.kind} row={r} />
        ))}
      </ul>
      <p className="mt-4 text-muted text-xs">
        polled every {POLL_INTERVAL_MS / 1000}s while this tab is open —
        grants made in System Settings reflect here within a tick.
      </p>
    </DossierCard>
  );
}

function PermissionRow({ row }: { row: Row }): JSX.Element {
  const granted = row.state === "granted";
  const statusLabel =
    row.state === "granted"
      ? "granted"
      : row.state === "denied"
        ? "denied"
        : "not requested";
  const statusColor =
    row.state === "granted" ? "text-text" : "text-accent";
  // Symbol gives a non-color signal for low-vision users (per PRODUCT.md
  // "color is never the only signal").
  const statusGlyph = granted ? "●" : "○";

  return (
    <li className="py-3 grid grid-cols-[1fr_auto] gap-x-6 items-center">
      <div>
        <p className="font-mono text-sm lowercase text-text">
          <span aria-hidden className={`mr-2 ${statusColor}`}>
            {statusGlyph}
          </span>
          {row.label}
          <span className="text-muted"> : {statusLabel}</span>
        </p>
        <p className="font-mono text-xs text-muted mt-1">{row.reason}</p>
      </div>
      {!granted ? (
        <button
          type="button"
          onClick={() => {
            void openSystemSettings(row.kind);
          }}
          className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
        >
          open system settings →
        </button>
      ) : null}
    </li>
  );
}
