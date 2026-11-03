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
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const cancelRef = useRef<HTMLButtonElement | null>(null);
  const groupRef = useRef<HTMLSpanElement | null>(null);
  // Was keyboard focus inside the confirm group when it disarmed? Only then
  // do we move focus back to the trigger (don't steal focus otherwise).
  const restoreFocus = useRef(false);
  const mounted = useRef(false);

  // Keep keyboard focus sane across the swap: the focused button unmounts
  // on arm and again on disarm, which dropped focus to <body>.
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    if (armed) {
      cancelRef.current?.focus(); // safe default for a destructive confirm
    } else if (restoreFocus.current) {
      restoreFocus.current = false;
      triggerRef.current?.focus();
    }
  }, [armed]);

  function disarm() {
    restoreFocus.current = !!groupRef.current?.contains(document.activeElement);
    setArmed(false);
  }

  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  function arm() {
    setArmed(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(disarm, timeoutMs);
  }

  function cancel() {
    disarm();
    if (timer.current) clearTimeout(timer.current);
  }

  function confirm() {
    disarm();
    if (timer.current) clearTimeout(timer.current);
    onConfirm();
  }

  if (!armed) {
    return (
      <button
        ref={triggerRef}
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
    <span
      ref={groupRef}
      role="group"
      aria-label={`confirm ${label}`}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          cancel();
        }
      }}
      className={["font-mono text-sm tracking-wide", className ?? ""].join(" ")}
    >
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
        ref={cancelRef}
        type="button"
        onClick={cancel}
        className="text-muted uppercase underline underline-offset-4 ml-2"
      >
        cancel
      </button>
    </span>
  );
}
