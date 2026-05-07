# 06 — high contrast

verify the AA contrast floor holds when Increase Contrast is on.

## setup

1. System Settings → Accessibility → Display → Increase Contrast → ON.

## steps

1. open settings window. inspect the following for legibility:
   - primary text on ink background (definition list values, section headers).
   - muted text (definition list keys, NavRail inactive items).
   - masthead WISPERTYPE on paper background.
2. use a contrast checker tool (e.g. Colour Contrast Analyser) to spot-check:
   - `oklch(0.98 0 0)` on `oklch(0.18 0.01 60)` — should be ≥ 7:1 (AAA).
   - `oklch(0.75 0.005 60)` on `oklch(0.18 0.01 60)` — should be ≥ 4.5:1 (AA).
3. no UI element should rely on color alone to convey state (permission rows use `●` / `○` glyphs; NavRail uses left-bar + text brightness).

## expected

- all text passes WCAG 2.2 AA (4.5:1 for normal, 3:1 for large).
- high-contrast mode does not break layout.

## cleanup

1. System Settings → Accessibility → Display → Increase Contrast → OFF.
