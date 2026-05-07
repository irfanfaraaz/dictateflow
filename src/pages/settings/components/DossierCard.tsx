import { forwardRef, type ReactNode } from "react";

interface DossierCardProps {
  /** Header label rendered in font-mono. */
  header: string;
  /** Whether this card is the active radio of its group. */
  active?: boolean;
  /** Optional handler — when present, header row becomes a radio control. */
  onActivate?: () => void;
  /** Optional radiogroup context for ARIA (when used in a group). */
  radioGroup?: { name: string };
  /**
   * Arrow-key navigation handler for radio groups. When present, ↑/← move
   * focus + selection backward, ↓/→ forward. Required by WCAG 2.2 AA for
   * radio groups; consumers wire it in the parent so they can roam focus
   * across siblings via refs.
   */
  onArrowNav?: (dir: "next" | "prev") => void;
  children: ReactNode;
}

/**
 * Bordered rectangle with optional radio + header + body. No drop-shadow,
 * no rounded corners — this is print-style editorial chrome (see shape
 * brief §3, "dossier cards").
 *
 * If `onActivate` is provided, the header row is keyboard-activatable and
 * announces as a radio. Otherwise it's purely informational.
 */
export const DossierCard = forwardRef<HTMLButtonElement, DossierCardProps>(
  function DossierCard(
    { header, active, onActivate, radioGroup, onArrowNav, children },
    radioRef,
  ) {
    const isRadio = typeof onActivate === "function";

    return (
      <section
        className={[
          "border border-rule p-6",
          active ? "bg-rule/10" : "bg-transparent",
        ].join(" ")}
        aria-label={header}
      >
        <header className="flex items-center gap-3 mb-4">
          {isRadio ? (
            <button
              ref={radioRef}
              type="button"
              role="radio"
              aria-checked={active ?? false}
              // Roving tabindex: only the active radio is in the tab
              // sequence; arrow keys move focus + selection within the
              // group. WAI-ARIA Authoring Practices §3.21.
              tabIndex={active ? 0 : -1}
              {...(radioGroup ? { name: radioGroup.name } : {})}
              onClick={onActivate}
              onKeyDown={(e) => {
                if (e.key === " " || e.key === "Enter") {
                  e.preventDefault();
                  onActivate?.();
                  return;
                }
                if (
                  onArrowNav &&
                  (e.key === "ArrowDown" || e.key === "ArrowRight")
                ) {
                  e.preventDefault();
                  onArrowNav("next");
                  return;
                }
                if (
                  onArrowNav &&
                  (e.key === "ArrowUp" || e.key === "ArrowLeft")
                ) {
                  e.preventDefault();
                  onArrowNav("prev");
                }
              }}
              className={[
                "shrink-0 w-4 h-4 rounded-full border flex items-center justify-center",
                active ? "border-accent" : "border-muted",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent",
              ].join(" ")}
              aria-label={active ? `${header} (selected)` : `select ${header}`}
            >
              {active ? (
                <span
                  aria-hidden
                  className="block w-2 h-2 rounded-full bg-accent"
                />
              ) : null}
            </button>
          ) : null}
          <h2 className="font-mono text-sm lowercase tracking-wide text-text">
            {header}
          </h2>
        </header>
        <div className="font-mono text-sm">{children}</div>
      </section>
    );
  },
);
