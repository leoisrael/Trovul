import React from "react";
import ReactDOM from "react-dom/client";
// Fonts are bundled: the app must look right offline, on a LAN without internet.
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./styles/theme.css";
import "./styles/ui.css";
import "./styles/shell.css";
import "./styles/pages.css";
import "./styles/flyout.css";
import "./styles/popups.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import { Flyout } from "./flyout/Flyout";
import { Popups } from "./popups/Popups";

// One bundle, three windows: the app, the tray mixer (src-tauri/src/flyout.rs)
// and the pop-ups (src-tauri/src/popups.rs).
const label = (() => {
  try {
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
})();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{label === "flyout" ? <Flyout /> : label === "popups" ? <Popups /> : <App />}</React.StrictMode>,
);
