import React from "react";
import ReactDOM from "react-dom/client";
import "./monaco/setup";
import { App } from "./App";

export function mount(): void {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
