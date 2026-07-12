import type * as Monaco from "monaco-editor";
import { DiagnosticSeverity, type LspDiagnostic, type PublishDiagnosticsParams } from "./protocol";
import type { LspClient } from "./lspClient";
import { fromLspRange } from "./conversions";

export const LSP_MARKER_OWNER = "solcore-lsp";

function markerSeverity(
  monaco: typeof Monaco,
  severity: DiagnosticSeverity | undefined,
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

function markerCode(code: LspDiagnostic["code"]): string | undefined {
  return code === undefined ? undefined : String(code);
}

function diagnosticToMarker(
  monaco: typeof Monaco,
  diagnostic: LspDiagnostic,
): Monaco.editor.IMarkerData {
  const range = fromLspRange(monaco, diagnostic.range);

  return {
    startLineNumber: range.startLineNumber,
    startColumn: range.startColumn,
    endLineNumber: range.endLineNumber,
    endColumn: range.endColumn,
    severity: markerSeverity(monaco, diagnostic.severity),
    message: diagnostic.message,
    code: markerCode(diagnostic.code),
    source: diagnostic.source,
    relatedInformation: diagnostic.relatedInformation?.map((information) => {
      const relatedRange = fromLspRange(monaco, information.location.range);
      return {
        resource: monaco.Uri.parse(information.location.uri),
        message: information.message,
        startLineNumber: relatedRange.startLineNumber,
        startColumn: relatedRange.startColumn,
        endLineNumber: relatedRange.endLineNumber,
        endColumn: relatedRange.endColumn,
      };
    }),
  };
}

export function registerDiagnostics(monaco: typeof Monaco, client: LspClient): () => void {
  const unsubscribe = client.onNotification(
    "textDocument/publishDiagnostics",
    (params: PublishDiagnosticsParams) => {
      const model = monaco.editor.getModel(monaco.Uri.parse(params.uri));
      if (!model) {
        return;
      }

      monaco.editor.setModelMarkers(
        model,
        LSP_MARKER_OWNER,
        params.diagnostics.map((diagnostic) => diagnosticToMarker(monaco, diagnostic)),
      );
    },
  );

  return () => {
    unsubscribe();
    for (const model of monaco.editor.getModels()) {
      monaco.editor.setModelMarkers(model, LSP_MARKER_OWNER, []);
    }
  };
}
