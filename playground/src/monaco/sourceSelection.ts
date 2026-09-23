import type * as Monaco from "monaco-editor";
import { formatSourceSelection, parseSourceSelection } from "../share/sourceSelection";
import { useWorkspaceStore } from "../store/workspace";
import { uriForWorkspacePath } from "./paths";

type CodeEditor = Monaco.editor.IStandaloneCodeEditor;

export function revealSourceSelection(editor: CodeEditor | null): void {
  const range = parseSourceSelection(useWorkspaceStore.getState().sourceSelection ?? undefined);
  const model = editor?.getModel();
  if (!range || !editor || !model) return;

  const validRange = model.validateRange(range);
  const current = editor.getSelection();
  if (current && formatSourceSelection(current) === formatSourceSelection(validRange)) return;

  editor.setSelection(validRange);
  editor.revealRangeInCenterIfOutsideViewport(validRange);
  editor.focus();
}

export function attachSourceSelection(editor: CodeEditor, monaco: typeof Monaco): Monaco.IDisposable {
  const selectedLines = editor.createDecorationsCollection();
  const updateSelectedLines = (): void => {
    const selections = (editor.getSelections() ?? []).filter((selection) => !selection.isEmpty());
    editor.getDomNode()?.classList.toggle("has-source-selection", selections.length > 0);
    selectedLines.set(selections.map((selection) => {
      // A selection ending at column 1 does not include that line.
      const lastLine = selection.endColumn === 1
        ? selection.endLineNumber - 1 : selection.endLineNumber;
      return {
        range: new monaco.Range(selection.startLineNumber, 1, lastLine, 1),
        options: { lineNumberClassName: "selected-line-number" },
      };
    }));
  };

  updateSelectedLines();
  const modelListener = editor.onDidChangeModel(updateSelectedLines);
  const selectionListener = editor.onDidChangeCursorSelection((event) => {
    updateSelectedLines();
    if (event.source !== "mouse" && event.source !== "keyboard") return;

    const state = useWorkspaceStore.getState();
    if (editor.getModel()?.uri.toString() !== uriForWorkspacePath(state.activePath)) return;
    const selection = formatSourceSelection(event.selection);
    if (selection !== state.sourceSelection) useWorkspaceStore.setState({ sourceSelection: selection });
  });

  return {
    dispose() {
      selectionListener.dispose();
      modelListener.dispose();
    },
  };
}
