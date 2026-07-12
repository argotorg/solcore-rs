import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspRange } from "../conversions";
import { LSP_MARKER_OWNER } from "../diagnostics";
import type { LspClient } from "../lspClient";
import {
  DiagnosticSeverity,
  type LspDiagnostic,
  type LspDiagnosticRelatedInformation,
  type LspRange,
} from "../protocol";

interface LspTextEdit {
  range: LspRange;
  newText: string;
}

interface LspTextDocumentEdit {
  textDocument: {
    uri: string;
    version: number | null;
  };
  edits: LspTextEdit[];
}

interface LspResourceOperation {
  kind: "create" | "rename" | "delete";
}

interface LspWorkspaceEdit {
  changes?: Record<string, LspTextEdit[]>;
  documentChanges?: Array<LspTextDocumentEdit | LspResourceOperation>;
}

interface LspCommand {
  title: string;
  command: string;
  arguments?: unknown[];
}

interface LspCodeAction {
  title: string;
  kind?: string;
  diagnostics?: LspDiagnostic[];
  edit?: LspWorkspaceEdit;
  command?: LspCommand;
  isPreferred?: boolean;
  disabled?: {
    reason: string;
  };
}

type LspCodeActionResponse = Array<LspCodeAction | LspCommand> | null;

function markerCode(marker: Monaco.editor.IMarkerData): string | undefined {
  if (marker.code === undefined) {
    return undefined;
  }

  return typeof marker.code === "string" ? marker.code : marker.code.value;
}

function toLspDiagnosticSeverity(
  monaco: typeof Monaco,
  severity: Monaco.MarkerSeverity,
): DiagnosticSeverity {
  switch (severity) {
    case monaco.MarkerSeverity.Warning:
      return DiagnosticSeverity.Warning;
    case monaco.MarkerSeverity.Info:
      return DiagnosticSeverity.Information;
    case monaco.MarkerSeverity.Hint:
      return DiagnosticSeverity.Hint;
    case monaco.MarkerSeverity.Error:
    default:
      return DiagnosticSeverity.Error;
  }
}

function toLspRelatedInformation(
  information: Monaco.editor.IRelatedInformation,
): LspDiagnosticRelatedInformation {
  return {
    location: {
      uri: information.resource.toString(),
      range: toLspRange(information),
    },
    message: information.message,
  };
}

function toLspDiagnostic(
  monaco: typeof Monaco,
  marker: Monaco.editor.IMarkerData,
): LspDiagnostic {
  return {
    range: toLspRange(marker),
    severity: toLspDiagnosticSeverity(monaco, marker.severity),
    code: markerCode(marker),
    source: marker.source,
    message: marker.message,
    relatedInformation: marker.relatedInformation?.map(toLspRelatedInformation),
  };
}

function toMonacoDiagnosticSeverity(
  monaco: typeof Monaco,
  severity: LspDiagnostic["severity"],
): Monaco.MarkerSeverity {
  switch (severity) {
    case DiagnosticSeverity.Warning:
      return monaco.MarkerSeverity.Warning;
    case DiagnosticSeverity.Information:
      return monaco.MarkerSeverity.Info;
    case DiagnosticSeverity.Hint:
      return monaco.MarkerSeverity.Hint;
    case DiagnosticSeverity.Error:
    default:
      return monaco.MarkerSeverity.Error;
  }
}

function fromLspRelatedInformation(
  monaco: typeof Monaco,
  information: LspDiagnosticRelatedInformation,
): Monaco.editor.IRelatedInformation {
  const range = fromLspRange(monaco, information.location.range);
  return {
    resource: monaco.Uri.parse(information.location.uri),
    message: information.message,
    startLineNumber: range.startLineNumber,
    startColumn: range.startColumn,
    endLineNumber: range.endLineNumber,
    endColumn: range.endColumn,
  };
}

function fromLspDiagnostic(
  monaco: typeof Monaco,
  diagnostic: LspDiagnostic,
): Monaco.editor.IMarkerData {
  const range = fromLspRange(monaco, diagnostic.range);
  return {
    startLineNumber: range.startLineNumber,
    startColumn: range.startColumn,
    endLineNumber: range.endLineNumber,
    endColumn: range.endColumn,
    severity: toMonacoDiagnosticSeverity(monaco, diagnostic.severity),
    message: diagnostic.message,
    code: diagnostic.code === undefined ? undefined : String(diagnostic.code),
    source: diagnostic.source,
    relatedInformation: diagnostic.relatedInformation?.map((information) =>
      fromLspRelatedInformation(monaco, information),
    ),
  };
}

function isTextDocumentEdit(
  change: LspTextDocumentEdit | LspResourceOperation,
): change is LspTextDocumentEdit {
  return "textDocument" in change && "edits" in change;
}

function appendTextEdits(
  monaco: typeof Monaco,
  edits: Monaco.languages.IWorkspaceTextEdit[],
  uri: string,
  textEdits: LspTextEdit[],
  versionId: number | undefined,
): void {
  const resource = monaco.Uri.parse(uri);
  for (const textEdit of textEdits) {
    edits.push({
      resource,
      textEdit: {
        range: fromLspRange(monaco, textEdit.range),
        text: textEdit.newText,
      },
      versionId,
    });
  }
}

function requestedModelVersion(
  monaco: typeof Monaco,
  modelVersions: ReadonlyMap<string, number>,
  uri: string,
): number | undefined {
  return modelVersions.get(monaco.Uri.parse(uri).toString());
}

function markerIntersectsRange(
  marker: Monaco.editor.IMarker,
  range: Monaco.IRange,
): boolean {
  const markerEndsBeforeRange =
    marker.endLineNumber < range.startLineNumber ||
    (marker.endLineNumber === range.startLineNumber &&
      marker.endColumn < range.startColumn);
  const rangeEndsBeforeMarker =
    range.endLineNumber < marker.startLineNumber ||
    (range.endLineNumber === marker.startLineNumber &&
      range.endColumn < marker.startColumn);
  return !markerEndsBeforeRange && !rangeEndsBeforeMarker;
}

function fromLspWorkspaceEdit(
  monaco: typeof Monaco,
  workspaceEdit: LspWorkspaceEdit,
  modelVersions: ReadonlyMap<string, number>,
): Monaco.languages.WorkspaceEdit | undefined {
  // The LSP representation requires clients to choose one representation. Do
  // not risk applying the same edit twice if a malformed response contains both.
  if (workspaceEdit.changes && workspaceEdit.documentChanges) {
    return undefined;
  }

  const edits: Monaco.languages.IWorkspaceTextEdit[] = [];
  if (workspaceEdit.changes) {
    for (const [uri, textEdits] of Object.entries(workspaceEdit.changes)) {
      appendTextEdits(
        monaco,
        edits,
        uri,
        textEdits,
        requestedModelVersion(monaco, modelVersions, uri),
      );
    }
  }

  if (workspaceEdit.documentChanges) {
    for (const change of workspaceEdit.documentChanges) {
      // Solcore currently returns `changes`. Supporting TextDocumentEdit here
      // is harmless, while rejecting resource operations avoids partial edits.
      if (!isTextDocumentEdit(change)) {
        return undefined;
      }
      // LSP document versions and Monaco model version ids are independent
      // counters. Reject versioned edits until the client tracks their mapping
      // instead of passing a potentially unrelated value to Monaco.
      if (change.textDocument.version !== null) {
        return undefined;
      }
      appendTextEdits(
        monaco,
        edits,
        change.textDocument.uri,
        change.edits,
        requestedModelVersion(
          monaco,
          modelVersions,
          change.textDocument.uri,
        ),
      );
    }
  }

  return { edits };
}

function isCommand(result: LspCodeAction | LspCommand): result is LspCommand {
  return typeof result.command === "string";
}

function fromLspCommand(command: LspCommand): Monaco.languages.Command {
  return {
    id: command.command,
    title: command.title,
    arguments: command.arguments,
  };
}

function fromLspCodeAction(
  monaco: typeof Monaco,
  action: LspCodeAction,
  modelVersions: ReadonlyMap<string, number>,
): Monaco.languages.CodeAction | null {
  const edit = action.edit
    ? fromLspWorkspaceEdit(monaco, action.edit, modelVersions)
    : undefined;
  if (action.edit && !edit) {
    return null;
  }

  return {
    title: action.title,
    kind: action.kind,
    diagnostics: action.diagnostics?.map((diagnostic) =>
      fromLspDiagnostic(monaco, diagnostic),
    ),
    edit,
    command: action.command ? fromLspCommand(action.command) : undefined,
    isPreferred: action.isPreferred,
    disabled: action.disabled?.reason,
  };
}

export function registerCodeAction(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerCodeActionProvider(
    SOLCORE_LANGUAGE_ID,
    {
      async provideCodeActions(model, range, context, token) {
        if (token.isCancellationRequested) {
          return null;
        }

        try {
          // Bind every returned workspace edit to the model versions for which
          // the request was made. If Monaco does not cancel a request while a
          // model changes, its version guard will still reject the stale edit.
          const modelVersions = new Map<string, number>(
            monaco.editor
              .getModels()
              .map((openModel) => [
                openModel.uri.toString(),
                openModel.getVersionId(),
              ] as const),
          );
          // Monaco's standalone adapter includes markers from every owner in
          // the provider context. Query only diagnostics published by the LSP
          // so compiler-result markers cannot interfere with stale checks.
          const diagnostics = monaco.editor
            .getModelMarkers({
              owner: LSP_MARKER_OWNER,
              resource: model.uri,
            })
            .filter((marker) => markerIntersectsRange(marker, range))
            .map((marker) => toLspDiagnostic(monaco, marker));

          const result = await client.request<LspCodeActionResponse>(
            "textDocument/codeAction",
            {
              textDocument: {
                uri: model.uri.toString(),
              },
              range: toLspRange(range),
              context: {
                diagnostics,
                only: context.only === undefined ? undefined : [context.only],
                triggerKind: context.trigger,
              },
            },
          );

          if (token.isCancellationRequested || !result) {
            return null;
          }

          const actions = result.flatMap((item) => {
            if (isCommand(item)) {
              return [
                {
                  title: item.title,
                  kind: "quickfix",
                  command: fromLspCommand(item),
                } satisfies Monaco.languages.CodeAction,
              ];
            }

            const action = fromLspCodeAction(monaco, item, modelVersions);
            return action ? [action] : [];
          });

          return {
            actions,
            dispose() {},
          };
        } catch {
          return null;
        }
      },
    },
    {
      providedCodeActionKinds: ["quickfix"],
    },
  );
}
