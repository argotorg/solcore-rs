import { useEffect } from "react";
import { findExample } from "../examples";
import { useWorkspaceStore, type OutputTab } from "../store/workspace";
import { buildExampleLink, readExampleRoute, readExampleView, readSharedExampleId, type ExampleView } from "./exampleLink";
import { parseSourceSelection } from "./sourceSelection";
import { workspaceView } from "./exampleView";

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
    const id = readExampleRoute(window.location.hash);
    const example = id ? findExample(id) : undefined;
    if (!example) return;
    applying = true;
    try {
      if (id !== useWorkspaceStore.getState().exampleId) useWorkspaceStore.getState().loadExample(example.id);
      const state = useWorkspaceStore.getState();
      const view = readExampleView(window.location.hash);
      const tab = view.tab === "run" ? "execution" : view.tab;
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
  if (!window.location.hash && (!legacyId || findExample(legacyId))) {
    const state = useWorkspaceStore.getState();
    window.history.replaceState(null, "", buildExampleLink(window.location.href, state.exampleId, workspaceView(state)));
  }
  followRoute();
  window.addEventListener("hashchange", followRoute);
  const unsubscribe = useWorkspaceStore.subscribe((state, previous) => {
    if (applying) return;
    const changed = state.exampleId !== previous.exampleId || JSON.stringify(workspaceView(state)) !== JSON.stringify(workspaceView(previous));
    const selectionOnly = state.exampleId === previous.exampleId &&
      JSON.stringify({ ...workspaceView(state), selection: undefined }) === JSON.stringify({ ...workspaceView(previous), selection: undefined });
    if (changed && selectionOnly) {
      const url = new URL(window.location.href);
      const view = readExampleView(url.hash);
      window.history.replaceState(null, "", buildExampleLink(url.href, state.exampleId, { ...view, selection: state.sourceSelection ?? undefined }));
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
      window.history.pushState(null, "", buildExampleLink(window.location.href, state.exampleId, workspaceView(state)));
    }
  });
  return () => { window.removeEventListener("hashchange", followRoute); unsubscribe(); };
}

export function useExampleRouter(): void {
  useEffect(attachExampleRouter, []);
}
