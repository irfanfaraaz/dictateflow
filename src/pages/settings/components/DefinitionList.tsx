import type { ReactNode } from "react";

export interface DefinitionRow {
  term: string;
  value: ReactNode;
}

interface DefinitionListProps {
  rows: DefinitionRow[];
}

/**
 * Two-column `<dl>` with tabular alignment. The 12ch term column matches
 * the longest expected key (`requirement`) so values line up across rows
 * — that vertical rhythm is a load-bearing part of the editorial look.
 */
export function DefinitionList({ rows }: DefinitionListProps): JSX.Element {
  return (
    <dl
      className="grid font-mono text-sm gap-y-1"
      style={{
        gridTemplateColumns: "12ch 1fr",
        fontFeatureSettings: '"tnum" 1',
      }}
    >
      {rows.map((row) => (
        <div key={row.term} className="contents">
          <dt className="text-muted">
            {row.term}
            <span aria-hidden> :</span>
          </dt>
          <dd className="text-text">{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}
