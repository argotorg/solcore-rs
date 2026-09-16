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
        lenses: state.testCases.filter((test) => test.file === file).map((test) => ({
          range: new monaco.Range(test.line, 1, test.line, 1),
          command: {
            id: state.compiling ? "" : "solcore.runTest",
            title: state.running ? "Running…" : "▶ Run test",
            tooltip: "Starts fresh and replays earlier setup calls before this test.",
            arguments: [test.id],
          },
        })),
        dispose() { },
      };
    },
  };
  const provider = monaco.languages.registerCodeLensProvider(SOLCORE_LANGUAGE_ID, codeLensProvider);
  const decorations = new Map<Monaco.editor.ITextModel, string[]>();
  let previousResults = useWorkspaceStore.getState().testResults;
  const render = (): void => {
    const state = useWorkspaceStore.getState();
    const model = editor.getModel();
    if (previousResults !== state.testResults) {
      for (const [model, ids] of decorations) {
        if (!model.isDisposed()) model.deltaDecorations(ids, []);
      }
      decorations.clear();
      previousResults = state.testResults;
    }
    if (!model) return;
    const ids = decorations.get(model) ?? [];
    const oldRanges = ids.map((id) => model.getDecorationRange(id));
    const file = workspacePathFromUri(model.uri.toString());
    const stale = state.testResultsVersion !== state.workspaceVersion;
    const visible = state.testResults.filter((test) => test.file === file && test.status !== "ready");
    decorations.set(model, model.deltaDecorations(ids, visible.map((test, i) => {
      const line = Math.min(oldRanges[i]?.startLineNumber ?? test.line, model.getLineCount());
      const column = model.getLineMaxColumn(line);
      const detail = test.message ?? (test.status === "passed"
        ? test.actual
        : `expected ${test.expected}, got ${test.actual}`);
      const text = `${test.status === "passed" ? "✓" : "✗"} ${test.replayed ? "Setup: " : ""}${detail}${stale ? " (outdated)" : ""}`;
      return {
        range: new monaco.Range(line, column, line, column),
        options: {
          showIfCollapsed: true,
          after: {
            content: `  ${text}`,
            inlineClassName: stale ? "test-result--outdated"
              : test.status === "passed" ? "test-result--passed" : "test-result--failed",
          },
          hoverMessage: { value: `${test.label}\n\n${text}${test.gasUsed !== null ? `\n\nGas: ${test.gasUsed}` : ""}` },
          stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
        },
      };
    })));
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
      provider.dispose();
      command.dispose();
      listeners.clear();
    },
  };
}
