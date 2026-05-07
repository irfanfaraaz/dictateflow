import { MODELS } from "@/models/catalog";
import { useModelDownloads } from "@/hooks/useModelDownloads";
import { DossierCard, DefinitionList, InlineConfirm } from "../components";

// ---------- helpers ----------

function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000) {
    return `${(bytes / 1_000_000).toFixed(1)} MB`;
  }
  return `${(bytes / 1_000).toFixed(0)} KB`;
}

// ---------- Models tab ----------

export function Models(): JSX.Element {
  const { progress, errors, installed, startDownload, cancelDownload, deleteModel } =
    useModelDownloads();

  return (
    <div className="space-y-6">
      {MODELS.map((model) => {
        const isInstalled = installed.includes(model.id);
        const dl = progress[model.id];
        const isDownloading = dl !== undefined;
        const error = errors[model.id];

        const pct =
          dl && dl.bytesTotal > 0
            ? Math.min(100, (dl.bytesDone / dl.bytesTotal) * 100)
            : 0;

        const actionRow = (() => {
          if (error) {
            return (
              <dd className="col-span-2 pt-3 space-y-2">
                <p className="font-mono text-sm text-accent lowercase">
                  error : {error}
                </p>
                <button
                  type="button"
                  onClick={() => startDownload(model.id)}
                  className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
                >
                  retry
                </button>
              </dd>
            );
          }

          if (isDownloading) {
            return (
              <dd className="col-span-2 pt-3 space-y-2">
                {/* Progress bar */}
                <div
                  className="h-px bg-rule w-full relative overflow-hidden"
                  role="progressbar"
                  aria-valuenow={Math.round(pct)}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-label={`downloading ${model.displayName}`}
                >
                  <div
                    className="absolute inset-y-0 left-0 bg-accent transition-all"
                    style={{ width: `${pct}%` }}
                  />
                </div>
                <div className="flex items-center justify-between">
                  <span className="font-mono text-xs text-muted">
                    {dl.bytesTotal > 0
                      ? `${formatBytes(dl.bytesDone)} / ${formatBytes(dl.bytesTotal)}`
                      : `${formatBytes(dl.bytesDone)}…`}
                  </span>
                  <button
                    type="button"
                    onClick={() => cancelDownload(model.id)}
                    className="font-mono text-xs uppercase tracking-wide text-accent underline underline-offset-4"
                  >
                    cancel
                  </button>
                </div>
              </dd>
            );
          }

          if (isInstalled) {
            return (
              <dd className="col-span-2 pt-3 flex items-center gap-4">
                <span className="font-mono text-sm text-muted uppercase tracking-wide">
                  installed
                </span>
                <InlineConfirm label="delete" onConfirm={() => deleteModel(model.id)} />
              </dd>
            );
          }

          // not installed, not downloading, no error
          return (
            <dd className="col-span-2 pt-3">
              <button
                type="button"
                onClick={() => startDownload(model.id)}
                className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
              >
                install
              </button>
            </dd>
          );
        })();

        return (
          <DossierCard key={model.id} header={model.displayName.toLowerCase()}>
            <DefinitionList
              rows={[
                { term: "size", value: `${model.sizeMb} mb` },
                { term: "notes", value: model.description },
                { term: "best for", value: model.recommendedFor },
              ]}
            />
            {/* Action row rendered outside dl so it can span full width */}
            <dl
              className="grid font-mono text-sm mt-0"
              style={{ gridTemplateColumns: "12ch 1fr" }}
            >
              {actionRow}
            </dl>
          </DossierCard>
        );
      })}
    </div>
  );
}
