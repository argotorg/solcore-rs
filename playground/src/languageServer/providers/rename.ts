import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";

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

export function registerRename(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerRenameProvider(SOLCORE_LANGUAGE_ID, {
    async provideRenameEdits(model, position, newName) {
      try {
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

        const edits: Monaco.languages.IWorkspaceTextEdit[] = [];
        for (const [uri, textEdits] of Object.entries(result.changes)) {
          const resource = monaco.Uri.parse(uri);
          for (const textEdit of textEdits) {
            edits.push({
              resource,
              versionId: undefined,
              textEdit: {
                range: fromLspRange(monaco, textEdit.range),
                text: textEdit.newText,
              },
            });
          }
        }

        return {
          edits,
        };
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
