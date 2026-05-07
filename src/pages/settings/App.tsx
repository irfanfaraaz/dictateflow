import { useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { Banner, Masthead, NavRail, type NavSection } from "./components";
import { About, Engine, Hotkey, Language, Models, Permissions } from "./tabs";
import { SettingsProvider } from "./SettingsContext";
import { useSettings } from "@/hooks/useSettings";
import { useModelDownloads } from "@/hooks/useModelDownloads";
import { useReducedMotion } from "@/hooks/useReducedMotion";

const BANNER_DISMISSED_KEY = "dictateflow.firstRunBannerDismissed";

const SECTIONS: NavSection[] = [
  { id: "engine", label: "engine" },
  { id: "hotkey", label: "hotkey" },
  { id: "models", label: "models" },
  { id: "language", label: "language" },
  { id: "permissions", label: "permissions" },
  { id: "about", label: "about" },
];

const TABS: Record<string, () => JSX.Element> = {
  engine: Engine,
  hotkey: Hotkey,
  models: Models,
  language: Language,
  permissions: Permissions,
  about: About,
};

/**
 * Settings window root.
 *
 * Layout: outer flex-col so the optional first-run banner can sit above
 * the masthead without breaking the inner 80px-masthead + 220px-rail
 * grid. Active tab swaps via AnimatePresence with an 80 ms cross-fade
 * (per shape brief §6 "section switch"); reduced-motion collapses to an
 * instant swap.
 */
export function SettingsApp(): JSX.Element {
  const { settings, update, ready } = useSettings();
  const { installed } = useModelDownloads();
  const [active, setActive] = useState<string>("engine");
  const reduced = useReducedMotion();
  // Lazy initializer reads from localStorage once on mount.
  const [dismissed, setDismissed] = useState<boolean>(
    () => localStorage.getItem(BANNER_DISMISSED_KEY) === "true",
  );

  if (!ready || !settings) {
    return (
      <main className="min-h-screen flex items-center justify-center bg-ink text-muted font-mono text-sm">
        loading settings…
      </main>
    );
  }

  const Active = TABS[active] ?? Engine;
  const motionDuration = reduced ? 0 : 0.08;

  // Show banner when: no model installed AND engine is local AND not dismissed.
  const showBanner =
    installed.length === 0 && settings.engine === "local" && !dismissed;

  function handleDismiss() {
    setDismissed(true);
    localStorage.setItem(BANNER_DISMISSED_KEY, "true");
  }

  return (
    <SettingsProvider value={{ settings, update }}>
      <div className="min-h-screen flex flex-col bg-ink text-text">
        {showBanner ? (
          <Banner onDismiss={handleDismiss}>
            welcome <span className="text-muted">—</span> set engine + grant
            mic to begin
          </Banner>
        ) : null}

        <div
          className="grid flex-1"
          style={{
            gridTemplateRows: "80px 1fr",
            gridTemplateColumns: "220px 1fr",
            minHeight: "520px",
          }}
        >
          {/* Masthead spans both columns. */}
          <div className="col-span-2">
            <Masthead />
          </div>

          {/* Left rail. */}
          <NavRail sections={SECTIONS} active={active} onSelect={setActive} />

          {/* Right column. */}
          <main className="overflow-y-auto p-8" id="settings-content">
            <AnimatePresence mode="wait" initial={false}>
              <motion.div
                key={active}
                initial={{ opacity: reduced ? 1 : 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: reduced ? 1 : 0 }}
                transition={{ duration: motionDuration }}
                className="space-y-6"
              >
                <SectionHeader id={active} />
                <Active />
              </motion.div>
            </AnimatePresence>
          </main>
        </div>
      </div>
    </SettingsProvider>
  );
}

function SectionHeader({ id }: { id: string }): JSX.Element | null {
  const idx = SECTIONS.findIndex((s) => s.id === id);
  if (idx < 0) return null;
  const numeral = String(idx + 1).padStart(2, "0");
  const label = SECTIONS[idx]?.label ?? id;
  return (
    <div>
      <p className="font-mono text-xs uppercase tracking-widest text-muted">
        {numeral} <span aria-hidden>—</span> {label}
      </p>
      <hr className="border-rule mt-2" />
    </div>
  );
}
