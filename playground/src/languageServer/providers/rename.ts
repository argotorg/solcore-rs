import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";
import { workspaceEditTargetsAreCurrent } from "./workspaceEdit.js";

interface LspTextEdit {
  range: LspRange;
  newText: string;
}

interface LspWorkspaceEdit {
  changes?: Record<string, LspTextEdit[]>;
}

interface LspPrepareRenamePlaceholder {
  range: LspRange;
  placeholder: string;
}

type LspPrepareRenameResult = LspRange | LspPrepareRenamePlaceholder | null;

function isPrepareRenamePlaceholder(
  result: Exclude<LspPrepareRenameResult, null>,
): result is LspPrepareRenamePlaceholder {
  return "range" in result;
}

function rejectRenameLocation(
  monaco: typeof Monaco,
  position: Monaco.IPosition,
  reason: string,
): Monaco.languages.RenameLocation & Monaco.languages.Rejection {
  return {
    range: new monaco.Range(
      position.lineNumber,
      position.column,
      position.lineNumber,
      position.column,
    ),
    text: "",
    rejectReason: reason,
  };
}

function applyWorkspaceEdit(
  monaco: typeof Monaco,
  changes: Record<string, LspTextEdit[]>,
  expectedVersions: ReadonlyMap<string, number>,
): boolean {
  if (
    !workspaceEditTargetsAreCurrent(changes, expectedVersions, (uri) =>
      monaco.editor.getModel(monaco.Uri.parse(uri)),
    )
  ) {
    return false;
  }

  let appliedAny = false;

  for (const [uri, textEdits] of Object.entries(changes)) {
    const resource = monaco.Uri.parse(uri);
    const targetModel = monaco.editor.getModel(resource);
    if (!targetModel || textEdits.length === 0) {
      continue;
    }

    targetModel.pushEditOperations(
      [],
      textEdits.map((textEdit) => ({
        range: fromLspRange(monaco, textEdit.range),
        text: textEdit.newText,
        forceMoveMarkers: true,
      })),
      () => null,
    );
    appliedAny = true;
  }

  return appliedAny;
}

export function registerRename(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerRenameProvider(SOLCORE_LANGUAGE_ID, {
    async provideRenameEdits(model, position, newName) {
      try {
        const expectedVersions = new Map<string, number>(
          monaco.editor
            .getModels()
            .map((current) => [current.uri.toString(), current.getVersionId()]),
        );
        const result = await client.request<LspWorkspaceEdit | null>(
          "textDocument/rename",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
            position: toLspPosition(position),
            newName,
          },
        );

        if (!result?.changes) {
          return null;
        }

        return applyWorkspaceEdit(monaco, result.changes, expectedVersions)
          ? { edits: [] }
          : null;
      } catch {
        return null;
      }
    },

    async resolveRenameLocation(model, position) {
      try {
        const result = await client.request<LspPrepareRenameResult>(
          "textDocument/prepareRename",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
            position: toLspPosition(position),
          },
        );

        if (!result) {
          return rejectRenameLocation(
            monaco,
            position,
            "Cannot rename this symbol.",
          );
        }

        if (isPrepareRenamePlaceholder(result)) {
          return {
            range: fromLspRange(monaco, result.range),
            text: result.placeholder,
          };
        }

        return {
          range: fromLspRange(monaco, result),
          text: model.getValueInRange(fromLspRange(monaco, result)),
        };
      } catch {
        return rejectRenameLocation(monaco, position, "Cannot rename this symbol.");
      }
    },
  });
}
