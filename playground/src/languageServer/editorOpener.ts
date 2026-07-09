import type * as Monaco from "monaco-editor";
import type { Pos } from "../compiler/types";
import { requestEditorNavigation } from "../components/editorNavigation";
import { workspacePathFromUri } from "../monaco/paths";
import { useWorkspaceStore } from "../store/workspace";

function targetPos(
  path: string,
  selectionOrPosition: Monaco.IRange | Monaco.IPosition,
): Pos {
  if ("startLineNumber" in selectionOrPosition) {
    const range = selectionOrPosition;
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

  const { lineNumber, column } = selectionOrPosition;
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
        requestEditorNavigation({
          path,
          range: targetPos(path, selectionOrPosition),
        });
      }
      if (store.activePath !== path) {
        store.setActive(path);
      }

      return true;
    },
  });
}
