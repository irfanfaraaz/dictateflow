import { useCallback, useState } from "react";

/**
 * Surface failed settings saves instead of letting `update()` rejections go
 * unhandled (the control silently snapped back with no explanation).
 *
 * `guard(promise)` swallows the rejection into visible state; the next
 * successful save clears it.
 */
export function useSaveError(): {
  error: string | null;
  guard: (p: Promise<unknown>) => void;
} {
  const [error, setError] = useState<string | null>(null);
  const guard = useCallback((p: Promise<unknown>) => {
    p.then(
      () => setError(null),
      (e: unknown) => setError(`couldn't save — ${e instanceof Error ? e.message : String(e)}`),
    );
  }, []);
  return { error, guard };
}

/** Same styling as the tabs' existing inline alert rows. */
export function SaveError({ error }: { error: string | null }): JSX.Element | null {
  if (!error) return null;
  return (
    <p role="alert" className="font-mono text-xs text-accent">
      {error}
    </p>
  );
}
