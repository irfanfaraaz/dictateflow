import type { ReactNode } from "react";

interface BannerProps {
  children: ReactNode;
  onDismiss: () => void;
}

/**
 * Single-row info banner rendered above the masthead.
 *
 * Background is off-white (`bg-paper`) to invert against the dark settings
 * window. Per shape brief §6 first-run, text is font-mono lowercase.
 */
export function Banner({ children, onDismiss }: BannerProps): JSX.Element {
  return (
    <div
      role="status"
      className="bg-paper text-ink px-8 py-3 flex items-center justify-between border-b border-rule"
    >
      <p className="font-mono text-sm lowercase">{children}</p>
      <button
        type="button"
        onClick={onDismiss}
        className="font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4"
      >
        dismiss
      </button>
    </div>
  );
}
