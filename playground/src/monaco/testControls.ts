import type * as Monaco from "monaco-editor";
import { useWorkspaceStore } from "../store/workspace";
import { workspacePathFromUri } from "./paths";
import { SOLCORE_LANGUAGE_ID } from "./solc-language";

/** Editor-owned controls; disposing the editor removes commands and providers. */
export function attachTestControls(
  editor: Monaco.editor.IStandaloneCodeEditor,
  monaco: typeof Monaco,
): Monaco.IDisposable {
  const listeners = new Set<(provider: Monaco.languages.CodeLensProvider) => void>();
  const decorations = new Map<Monaco.editor.ITextModel, string[]>();
  const resultLenses = new Map<Monaco.editor.ITextModel, Monaco.languages.CodeLens[]>();
  const refresh = (): void => {
    for (const listener of listeners) listener(codeLensProvider);
  };
  const command = monaco.editor.registerCommand("solcore.runTest", (_accessor, id: string) => {
    if (!useWorkspaceStore.getState().compiling) void useWorkspaceStore.getState().runNow(id);
  });
  const codeLensProvider: Monaco.languages.CodeLensProvider = {
    onDidChange: (listener) => {
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
    provideCodeLenses(model) {
      const state = useWorkspaceStore.getState();
      const file = workspacePathFromUri(model.uri.toString());
      return {
        lenses: [...state.testCases.filter((test) => test.file === file).map((test) => ({
          range: new monaco.Range(test.line, 1, test.line, 1),
          command: {
            id: state.compiling ? "" : "solcore.runTest",
            title: state.running ? "Running…" : "▶ Run test",
            tooltip: "Starts fresh and replays earlier setup calls before this test.",
            arguments: [test.id],
          },
        })), ...(resultLenses.get(model) ?? [])],
        dispose() { },
      };
    },
  };
  const provider = monaco.languages.registerCodeLensProvider(SOLCORE_LANGUAGE_ID, codeLensProvider);
  let previousResults = useWorkspaceStore.getState().testResults;
  const render = (): void => {
    const state = useWorkspaceStore.getState();
    const model = editor.getModel();
    if (previousResults !== state.testResults) {
      for (const [model, ids] of decorations) {
        if (!model.isDisposed()) model.deltaDecorations(ids, []);
      }
      decorations.clear();
      resultLenses.clear();
      previousResults = state.testResults;
    }
    if (!model) return;
    const ids = decorations.get(model) ?? [];
    const oldRanges = ids.map((id) => model.getDecorationRange(id));
    const file = workspacePathFromUri(model.uri.toString());
    const stale = state.testResultsVersion !== state.workspaceVersion;
    const visible = state.testResults.filter((test) => test.file === file && test.status !== "ready");
    const lenses: Monaco.languages.CodeLens[] = [];
    decorations.set(model, model.deltaDecorations(ids, visible.map((test, i) => {
      const line = Math.min(oldRanges[i]?.startLineNumber ?? test.line, model.getLineCount());
      const detail = test.message ?? (test.status === "passed"
        ? test.expected?.startsWith("revert") ? "reverted as expected" : test.actual
        : `expected ${test.expected}, got ${test.actual}`);
      const summary = `${test.replayed ? "Setup: " : ""}${detail}${stale ? " (outdated)" : ""}`;
      const text = `${test.status === "passed" ? "✓" : "✗"} ${summary}`;
      const gas = test.gasUsed !== null ? `\n\nGas: ${test.gasUsed}` : "";
      const icon = stale ? "history" : test.status === "passed" ? "pass" : "error";
      // Share CodeLens's reserved row with Run test. Injected result text can
      // wrap source annotations into the next CodeLens's view zone.
      lenses.push({
        range: new monaco.Range(line, 1, line, 1),
        command: {
          id: "",
          title: `$(${icon}) ${summary.replace(/\s+/g, " ")}`,
          tooltip: `${text}\n\n${test.label}${gas}`,
        },
      });
      return {
        range: new monaco.Range(line, 1, line, model.getLineMaxColumn(line)),
        options: {
          hoverMessage: { value: `${test.label}\n\n${text}${gas}` },
          stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
        },
      };
    })));
    resultLenses.set(model, lenses);
    refresh();
  };
  const unsubscribe = useWorkspaceStore.subscribe(render);
  const modelChange = editor.onDidChangeModel(render);
  render();
  return {
    dispose() {
      unsubscribe();
      modelChange.dispose();
      for (const [model, ids] of decorations) {
        if (!model.isDisposed()) model.deltaDecorations(ids, []);
      }
      decorations.clear();
      resultLenses.clear();
      provider.dispose();
      command.dispose();
      listeners.clear();
    },
  };
}
