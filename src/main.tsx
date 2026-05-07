import { createRoot } from "react-dom/client";
import "./styles/globals.css";

function App() {
  return (
    <main className="min-h-screen flex items-center justify-center">
      <div className="text-center">
        <h1 className="font-display text-5xl uppercase tracking-tight">wispertype</h1>
        <p className="font-mono text-muted mt-4">settings — coming soon</p>
      </div>
    </main>
  );
}

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("#root not found");
createRoot(rootEl).render(<App />);
