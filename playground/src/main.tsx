import { prepareProject } from "./project/context";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/app.css";

const root = document.getElementById("root") as HTMLElement;

async function start(): Promise<void> {
  const panel = document.createElement("section");
  panel.className = "startup-status";
  panel.setAttribute("aria-live", "polite");
  const heading = document.createElement("h1");
  heading.textContent = "Loading playground…";
  panel.append(heading);
  root.replaceChildren(panel);
  try {
    await prepareProject(window.location.href);
    const { mount } = await import("./mount");
    mount();
  } catch (error) {
    heading.textContent = "Could not open the playground";
    const message = document.createElement("p");
    message.textContent = error instanceof Error ? error.message : "An unexpected loading error occurred.";
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "button button--primary";
    retry.textContent = "Retry";
    retry.addEventListener("click", () => { void start(); });
    panel.append(message, retry);
  }
}

void start();
