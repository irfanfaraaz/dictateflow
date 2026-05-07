import { createRoot } from "react-dom/client";
import "./styles/globals.css";
import { SettingsApp } from "./pages/settings/App";

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("#root not found");
createRoot(rootEl).render(<SettingsApp />);
