import { findExample } from "../examples";
import type { WorkspaceState } from "../store/workspace";
import type { ExampleView } from "./exampleLink";

export function workspaceView(state: WorkspaceState): ExampleView {
  const contract = state.contracts.find((c) => c.name === state.callDraft.contract) ?? state.contracts[0];
  const method = contract?.methods.find((m) => m.signature === state.callDraft.signature) ?? contract?.methods[0];
  return {
    selection: state.sourceSelection ?? undefined,
    file: state.activePath !== findExample(state.exampleId)?.entry ? state.activePath : undefined,
    tab: state.outputTab !== "execution" ? state.outputTab : undefined,
    view: state.runActivity !== "calls" || state.viewedTestId ? state.runActivity : undefined,
    contract: contract !== state.contracts[0] ? contract?.name : undefined,
    function: method !== contract?.methods[0] ? method?.signature : undefined,
    test: state.viewedTestId ?? undefined,
  };
}
