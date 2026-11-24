import type * as Monaco from "monaco-editor";
import { useWorkspaceStore } from "../store/workspace";
import { uriForWorkspacePath } from "./paths";

export function attachReplayLocation(editor: Monaco.editor.IStandaloneCodeEditor): Monaco.IDisposable {
  const marks = editor.createDecorationsCollection();
  let reveal = 0;
  const update = (): void => {
    cancelAnimationFrame(reveal);
    const { replayLocation, files } = useWorkspaceStore.getState();
    const model = editor.getModel();
    if (!model || !replayLocation) {
      marks.clear();
      return;
    }
    const { range, contents } = replayLocation;
    if (model.uri.toString() !== uriForWorkspacePath(range.file) || files[range.file]?.content !== contents) {
      marks.clear();
      return;
    }
    // Monaco can normalize line endings and keeps a leading BOM outside column numbering.
    const modelText = model.getValue(undefined, true);
    if (modelText !== contents && modelText.replace(/\r\n?/g, "\n") !== contents.replace(/\r\n?/g, "\n")) {
      marks.clear();
      return;
    }
    const bom = contents.startsWith("\uFEFF") ? 1 : 0;
    const position = {
      startLineNumber: range.startLine,
      startColumn: Math.max(1, range.startCol - (range.startLine === 1 ? bom : 0)),
      endLineNumber: range.endLine,
      endColumn: Math.max(1, range.endCol - (range.endLine === 1 ? bom : 0)),
    };
    const valid = model.validateRange(position);
    if (!valid.equalsRange(position) || valid.isEmpty()) {
      marks.clear();
      return;
    }
    marks.set([
      { range: valid, options: { inlineClassName: "replay-source-range" } },
      { range: valid.collapseToStart(), options: { linesDecorationsClassName: "replay-source-gutter" } },
    ]);
    // The React wrapper restores the new model's scroll state after switching files.
    reveal = requestAnimationFrame(() => editor.revealRangeInCenterIfOutsideViewport(valid));
  };

  const model = editor.onDidChangeModel(update);
  const content = editor.onDidChangeModelContent(update);
  const unsubscribe = useWorkspaceStore.subscribe((state, previous) => {
    const path = state.replayLocation?.range.file;
    if (state.replayLocation !== previous.replayLocation || (path && state.files[path] !== previous.files[path])) update();
  });
  update();

  return {
    dispose() {
      model.dispose();
      content.dispose();
      unsubscribe();
      cancelAnimationFrame(reveal);
    },
  };
}
