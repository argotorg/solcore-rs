import { findExample } from "../examples";
import { projectSource } from "../store/session";
import type { WorkspaceState } from "../store/workspace";
import { buildExampleLink, buildProjectLink, NAVIGATION_FIELDS, type ExampleLocation, type ExampleView } from "./exampleLink";

export function workspaceView(state: WorkspaceState): ExampleView {
  const contract = state.contracts.find((c) => c.name === state.callDraft.contract) ?? state.contracts[0];
  const method = contract?.methods.find((m) => m.signature === state.callDraft.signature) ?? contract?.methods[0];
  return {
    selection: state.sourceSelection ?? undefined,
    file: state.activePath !== (projectSource ?? findExample(state.exampleId))?.entry ? state.activePath : undefined,
    tab: state.outputTab !== "execution" ? state.outputTab : undefined,
    view: state.runActivity !== "calls" || state.viewedTestId ? state.runActivity : undefined,
    contract: contract !== state.contracts[0] ? contract?.name : undefined,
    function: method !== contract?.methods[0] ? method?.signature : undefined,
    test: state.viewedTestId ?? undefined,
    backend: state.options.backend !== "solc" ? state.options.backend : undefined,
  };
}

export function workspaceLink(href: string, id: string, view: ExampleView): string {
  return projectSource ? buildProjectLink(href, view) : buildExampleLink(href, id, view);
}

export function workspaceLocation(state: WorkspaceState): ExampleLocation {
  return { id: state.exampleId, view: workspaceView(state) };
}

export function sameNavigation(a: ExampleLocation, b: ExampleLocation): boolean {
  return a.id === b.id && NAVIGATION_FIELDS.every((field) => a.view[field] === b.view[field]);
}
