import { getLoadedProject } from "../project/context";
import { isBrowser } from "./isBrowser";

const parameters = new URLSearchParams(isBrowser() ? window.location.search : "");
export const project = getLoadedProject();
export const projectSource = project?.manifest;
export const embedded = parameters.get("embed") === "1";

// Identity follows the canonical URL, never an id that can collide with a
// bundled example or a project served by a different publisher on this origin.
const workspaceKey = project
  ? `solcore-playground.project.v1.${encodeURIComponent(project.url)}`
  : "solcore-playground.workspace.v1";

export const workspaceStorage = {
  key: workspaceKey,
  backupKey: `${workspaceKey}.backup`,
};
