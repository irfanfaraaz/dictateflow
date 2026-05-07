import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./pill.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // editorial-dark palette per shape brief
        ink: "oklch(0.18 0.01 60)",
        paper: "oklch(0.92 0.005 60)",
        rule: "oklch(0.32 0.01 60)",
        muted: "oklch(0.55 0.005 60)",
        text: "oklch(0.92 0.005 60)",
        accent: "oklch(0.62 0.20 25)",
        // Derived urgency token: brighter at ≥80% cap. Not in shape brief base set;
        // defined here so JSX can use `text-accent-warn` in parallel with `var(--accent-warn)` in canvas.
        "accent-warn": "oklch(0.68 0.22 25)",
      },
      fontFamily: {
        mono: ["Berkeley Mono", "SF Mono", "ui-monospace", "monospace"],
        display: ["Helvetica Now Mono", "SF Mono", "ui-monospace", "monospace"],
      },
    },
  },
  plugins: [],
} satisfies Config;
