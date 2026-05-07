import { createContext, useContext, type ReactNode } from "react";
import type { DeepPartial, Settings } from "@/schemas/settings";

interface SettingsContextValue {
  settings: Settings;
  update: (patch: DeepPartial<Settings>) => Promise<void>;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({
  value,
  children,
}: {
  value: SettingsContextValue;
  children: ReactNode;
}): JSX.Element {
  return (
    <SettingsContext.Provider value={value}>
      {children}
    </SettingsContext.Provider>
  );
}

/**
 * Tabs read settings via this hook so they don't each re-mount a
 * `useSettings` listener. Throws if used outside the provider — that's a
 * developer bug, not a user-facing failure.
 */
export function useSettingsContext(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) {
    throw new Error("useSettingsContext must be used within SettingsProvider");
  }
  return ctx;
}
