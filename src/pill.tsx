import { createRoot } from "react-dom/client";
import "./styles/globals.css";
import { Pill } from "@/pages/pill/Pill";

const rootEl = document.getElementById("pill-root");
if (!rootEl) throw new Error("#pill-root not found");
createRoot(rootEl).render(<Pill />);
