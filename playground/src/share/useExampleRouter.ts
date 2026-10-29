import { useEffect } from "react";
import { findExample } from "../examples";
import { projectSource } from "../store/session";
import { useWorkspaceStore, type OutputTab } from "../store/workspace";
import { readExampleLocation, readExampleView, readProjectView, readSharedExampleId, type ExampleView } from "./exampleLink";
import { parseSourceSelection } from "./sourceSelection";
import { sameNavigation, workspaceLink, workspaceLocation } from "./exampleView";

const tabs: OutputTab[] = ["execution", "hull", "yul", "sonatina", "abi", "problems"];

export function attachExampleRouter(): () => void {
  let applying = false;
  let pending: { view: ExampleView; version: number } | null = null;
  const applySelection = (view: ExampleView): void => {
    const state = useWorkspaceStore.getState();
    const test = state.testCases.find((test) => test.id === view.test);
    const contract = state.contracts.find((c) => c.name === view.contract) ?? state.contracts[0];
    const method = contract?.methods.find((m) => m.signature === view.function) ?? contract?.methods[0];
    const currentContract = state.contracts.find((c) => c.name === state.callDraft.contract) ?? state.contracts[0];
    const currentMethod = currentContract?.methods.find((m) => m.signature === state.callDraft.signature) ?? currentContract?.methods[0];
    const sameCall = contract === currentContract && method === currentMethod;
    useWorkspaceStore.setState({
      viewedTestId: test?.id ?? null,
      ...(test && !view.view ? { runActivity: "tests" as const } : {}),
      ...(!sameCall ? { selectedTestId: null } : {}),
      callDraft: { ...state.callDraft, contract: contract?.name ?? "", signature: method?.signature ?? "",
        ...(!sameCall ? { arguments: "[]", constructorArguments: "[]" } : {}) },
    });
  };
  const followRoute = (): void => {
    pending = null;
    const projectView = projectSource ? readProjectView(window.location.hash) : null;
    const location = projectSource
      ? (projectView ? { id: projectSource.id, view: projectView } : null)
      : readExampleLocation(window.location.hash);
    if (!location) return;
    const example = projectSource ?? findExample(location.id);
    if (!example) return;
    applying = true;
    try {
      if (!projectSource && location.id !== useWorkspaceStore.getState().exampleId) useWorkspaceStore.getState().loadExample(example.id);
      const state = useWorkspaceStore.getState();
      const view = location.view;
      const tab = view.tab === "run" || (view.tab === "sonatina" && view.backend !== "sonatina") ? "execution" : view.tab;
      state.setBackend(view.backend === "sonatina" ? "sonatina" : "solc");
      const entry = Object.hasOwn(state.files, example.entry) ? example.entry : state.entry;
      state.setActive(view.file && Object.hasOwn(state.files, view.file) ? view.file : entry);
      useWorkspaceStore.setState({ outputTab: tabs.includes(tab as OutputTab) ? tab as OutputTab : "execution",
        sourceSelection: (!view.file || Object.hasOwn(state.files, view.file)) && parseSourceSelection(view.selection) ? view.selection! : null,
        runActivity: view.view === "tests" ? "tests" : "calls", viewedTestId: null });
      if (state.discoveryVersion === state.workspaceVersion) applySelection(view);
      else pending = { view, version: state.workspaceVersion };
    } finally { applying = false; }
  };

  // Keep unknown links intact. Normalize empty and legacy URLs without adding history.
  const legacyId = readSharedExampleId(window.location.search);
  if ((projectSource && !readProjectView(window.location.hash)) || (!window.location.hash && (!legacyId || findExample(legacyId)))) {
    const state = useWorkspaceStore.getState();
    window.history.replaceState(null, "", workspaceLink(window.location.href, state.exampleId, workspaceLocation(state).view));
  }
  followRoute();
  window.addEventListener("hashchange", followRoute);
  const unsubscribe = useWorkspaceStore.subscribe((state, previous) => {
    if (applying) return;
    const current = workspaceLocation(state);
    const before = workspaceLocation(previous);
    const navigationChanged = !sameNavigation(current, before);
    const selectionChanged = current.view.selection !== before.view.selection;
    const changed = navigationChanged || selectionChanged;
    if (selectionChanged && !navigationChanged) {
      const view = readExampleView(window.location.hash);
      window.history.replaceState(null, "", workspaceLink(window.location.href, current.id,
        { ...view, selection: current.view.selection }));
      return;
    }
    if (pending && !changed && pending.version === state.workspaceVersion) {
      if (state.discoveryVersion === state.workspaceVersion) {
        const view = pending.view;
        pending = null;
        applying = true;
        try { applySelection(view); } finally { applying = false; }
      }
      return;
    }
    if (changed || (pending && pending.version !== state.workspaceVersion)) pending = null;
    if (changed) {
      window.history.pushState(null, "", workspaceLink(window.location.href, current.id, current.view));
    }
  });
  return () => { window.removeEventListener("hashchange", followRoute); unsubscribe(); };
}

export function useExampleRouter(): void {
  useEffect(attachExampleRouter, []);
}
