/**
 * Off-white masthead block at the top of the settings window.
 *
 * Per shape brief §3, this is the largest type moment in the app:
 * Helvetica Now Mono Black, full-bleed warm off-white. The right-aligned
 * meta is Berkeley Mono.
 */
export function Masthead(): JSX.Element {
  return (
    <header className="bg-paper text-ink h-20 px-8 flex items-center justify-between border-b border-rule">
      <h1 className="font-display font-black text-5xl tracking-tight uppercase leading-none">
        wispertype
      </h1>
      <span className="font-mono text-sm tracking-wide uppercase">settings / 2026</span>
    </header>
  );
}
