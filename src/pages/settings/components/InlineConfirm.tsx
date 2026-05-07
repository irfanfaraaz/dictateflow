import { useEffect, useRef, useState } from "react";

interface InlineConfirmProps {
  /** Idle label, e.g. `delete`. */
  label: string;
  /** Called when the user clicks `yes` after the confirmation transition. */
  onConfirm: () => void;
  /** Optional className applied to the outer button group. */
  className?: string;
  /** Reset window — defaults to 3 s. */
  timeoutMs?: number;
}

/**
 * One-step inline confirm. Per shape brief §7, destructive actions don't
 * open a modal — the CTA itself transforms into `confirm? yes / cancel`
 * for ~3 s, then snaps back. Cheaper than a dialog and matches the
 * keyboard-first interaction model.
 */
export function InlineConfirm({
  label,
  onConfirm,
  className,
  timeoutMs = 3000,
}: InlineConfirmProps): JSX.Element {
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  function arm() {
    setArmed(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setArmed(false), timeoutMs);
  }

  function cancel() {
    setArmed(false);
    if (timer.current) clearTimeout(timer.current);
  }

  function confirm() {
    setArmed(false);
    if (timer.current) clearTimeout(timer.current);
    onConfirm();
  }

  if (!armed) {
    return (
      <button
        type="button"
        onClick={arm}
        className={[
          "font-mono text-sm uppercase tracking-wide text-accent underline underline-offset-4",
          className ?? "",
        ].join(" ")}
      >
        {label}
      </button>
    );
  }

  return (
    <span className={["font-mono text-sm tracking-wide", className ?? ""].join(" ")}>
      <span className="text-muted lowercase">confirm? </span>
      <button
        type="button"
        onClick={confirm}
        className="text-accent uppercase underline underline-offset-4 mr-2"
      >
        yes
      </button>
      <span className="text-muted">/</span>
      <button
        type="button"
        onClick={cancel}
        className="text-muted uppercase underline underline-offset-4 ml-2"
      >
        cancel
      </button>
    </span>
  );
}
