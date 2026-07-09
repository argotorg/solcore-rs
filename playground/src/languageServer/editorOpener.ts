import type * as Monaco from "monaco-editor";
import type { Pos } from "../compiler/types";
import { requestEditorNavigation } from "../components/editorNavigation";
import { workspacePathFromUri } from "../monaco/paths";
import { useWorkspaceStore } from "../store/workspace";

function posAtPosition(path: string, { lineNumber, column }: Monaco.IPosition): Pos {
  return {
    file: path,
    startByte: 0,
    endByte: 0,
    startLine: lineNumber,
    startCol: column,
    endLine: lineNumber,
    endCol: column,
  };
}

function posFromRange(path: string, range: Monaco.IRange): Pos {
  return {
    file: path,
    startByte: 0,
    endByte: 0,
    startLine: range.startLineNumber,
    startCol: range.startColumn,
    endLine: range.endLineNumber,
    endCol: range.endColumn,
  };
}

function isZeroWidthRange(range: Monaco.IRange): boolean {
  return (
    range.startLineNumber === range.endLineNumber &&
    range.startColumn === range.endColumn
  );
}

function posAtWordOrPosition(
  path: string,
  model: Monaco.editor.ITextModel | null,
  position: Monaco.IPosition,
): Pos {
  const word = model?.getWordAtPosition(position);
  if (!word) {
    return posAtPosition(path, position);
  }

  return {
    file: path,
    startByte: 0,
    endByte: 0,
    startLine: position.lineNumber,
    startCol: word.startColumn,
    endLine: position.lineNumber,
    endCol: word.endColumn,
  };
}

function targetPos(
  path: string,
  model: Monaco.editor.ITextModel | null,
  selectionOrPosition: Monaco.IRange | Monaco.IPosition,
): Pos {
  if ("startLineNumber" in selectionOrPosition) {
    const range = selectionOrPosition;
    if (!isZeroWidthRange(range)) {
      return posFromRange(path, range);
    }

    return posAtWordOrPosition(path, model, {
      lineNumber: range.startLineNumber,
      column: range.startColumn,
    });
  }

  return posAtWordOrPosition(path, model, selectionOrPosition);
}

/**
 * Teaches the standalone editor how to follow a definition/reference into
 * another workspace file. Monaco only navigates within the active model on its
 * own; for a target in a different `file:///main/<relpath>` document we switch
 * the active tab and reveal the range through the shared navigation seam.
 * Targets outside the workspace (for example the embedded `/std` library) are
 * left unhandled.
 */
export function registerEditorOpener(monaco: typeof Monaco): Monaco.IDisposable {
  return monaco.editor.registerEditorOpener({
    openCodeEditor(_source, resource, selectionOrPosition) {
      const path = workspacePathFromUri(resource.toString());
      const store = useWorkspaceStore.getState();

      if (!path || !store.files[path]) {
        return false;
      }

      if (selectionOrPosition) {
        const model = monaco.editor.getModel(resource);
        requestEditorNavigation({
          path,
          range: targetPos(path, model, selectionOrPosition),
        });
      }
      if (store.activePath !== path) {
        store.setActive(path);
      }

      return true;
    },
  });
}
