import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { api } from "./api";
import { bootTextScale } from "./textScale";

// The text size goes on before the first render, so the window never paints
// at 100% and then jumps. `bootTextScale` settles on its own -- read, failed
// (no IPC bridge under `bun run dev` in a browser) or timed out -- and never
// rejects, so the render below always runs.
void bootTextScale(() => api.getSettings()).then(() => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
});
