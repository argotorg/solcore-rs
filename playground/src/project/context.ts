import { loadProject, type LoadedProject } from "./loadProject";

let project: LoadedProject | null = null;

// Bootstrap completes this before importing the store or editor modules.
export async function prepareProject(documentUrl: string, fetcher?: typeof fetch): Promise<void> {
  project = await loadProject(documentUrl, fetcher);
}

export function getLoadedProject(): LoadedProject | null {
  return project;
}
