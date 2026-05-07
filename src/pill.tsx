import { createRoot } from "react-dom/client";
import "./styles/globals.css";

function Pill() {
  return (
    <div className="px-4 py-3 border border-rule rounded-full bg-ink inline-flex gap-3 items-center">
      <span className="font-mono text-xs text-muted">idle</span>
    </div>
  );
}

const rootEl = document.getElementById("pill-root");
if (!rootEl) throw new Error("#pill-root not found");
createRoot(rootEl).render(<Pill />);
