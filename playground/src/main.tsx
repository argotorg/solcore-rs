import React from "react";
import ReactDOM from "react-dom/client";
import "./monaco/setup";
import { App } from "./App";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/app.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
