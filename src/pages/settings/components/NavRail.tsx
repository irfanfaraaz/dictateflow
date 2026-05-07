import { useRef } from "react";

export interface NavSection {
  id: string;
  label: string;
}

interface NavRailProps {
  sections: NavSection[];
  active: string;
  onSelect: (id: string) => void;
}

/**
 * 220px left rail of numbered TOC items. Active row gets a 4px red
 * left-edge bar + brighter text; inactive rows are muted.
 *
 * Keyboard: arrow keys move the focused row; Enter/Space activates.
 * `aria-current="page"` marks the active section. We use a `<nav>` with
 * an inner `role="tablist"` so screen readers announce both the
 * navigation landmark and the section structure.
 */
export function NavRail({ sections, active, onSelect }: NavRailProps): JSX.Element {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  function onKeyDown(idx: number) {
    return (e: React.KeyboardEvent<HTMLButtonElement>) => {
      if (e.key === "ArrowDown" || e.key === "ArrowRight") {
        e.preventDefault();
        const next = (idx + 1) % sections.length;
        const el = refs.current[next];
        if (el) {
          el.focus();
          // Eager activation: matches the editorial-magazine "see content
          // change as you arrow through" rhythm; same as VS Code's tabs.
          const target = sections[next];
          if (target) onSelect(target.id);
        }
      } else if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
        e.preventDefault();
        const next = (idx - 1 + sections.length) % sections.length;
        const el = refs.current[next];
        if (el) {
          el.focus();
          const target = sections[next];
          if (target) onSelect(target.id);
        }
      }
    };
  }

  return (
    <nav
      aria-label="settings sections"
      className="border-r border-rule h-full flex flex-col py-4"
    >
      <ul role="tablist" aria-orientation="vertical" className="flex flex-col">
        {sections.map((s, idx) => {
          const isActive = s.id === active;
          const numeral = String(idx + 1).padStart(2, "0");
          return (
            <li key={s.id} role="presentation">
              <button
                ref={(el) => {
                  refs.current[idx] = el;
                }}
                type="button"
                role="tab"
                aria-current={isActive ? "page" : undefined}
                aria-selected={isActive}
                tabIndex={isActive ? 0 : -1}
                onClick={() => onSelect(s.id)}
                onKeyDown={onKeyDown(idx)}
                className={[
                  "w-full h-14 text-left pl-6 pr-4 font-mono text-sm lowercase",
                  "border-l-4 transition-colors duration-75",
                  "focus:outline-none focus-visible:bg-rule/20",
                  isActive
                    ? "border-l-accent text-text"
                    : "border-l-transparent text-muted hover:text-text",
                ].join(" ")}
              >
                <span className="tabular-nums">{numeral}</span>
                <span className="text-muted/70 mx-1">/</span>
                <span>{s.label}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
