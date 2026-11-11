import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { startDebugBridge } from "./lib/debugBridge";

// Does nothing unless the backend runs the debug bridge (debug builds).
void startDebugBridge();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
