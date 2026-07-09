import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";

type LspDocumentHighlightKind = 1 | 2 | 3;

interface LspDocumentHighlight {
  range: LspRange;
  kind?: LspDocumentHighlightKind;
}

function toMonacoDocumentHighlightKind(
  monaco: typeof Monaco,
  kind: LspDocumentHighlightKind | undefined,
): Monaco.languages.DocumentHighlightKind {
  switch (kind) {
    case 2:
      return monaco.languages.DocumentHighlightKind.Read;
    case 3:
      return monaco.languages.DocumentHighlightKind.Write;
    case 1:
    default:
      return monaco.languages.DocumentHighlightKind.Text;
  }
}

export function registerDocumentHighlight(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerDocumentHighlightProvider(SOLCORE_LANGUAGE_ID, {
    async provideDocumentHighlights(model, position) {
      try {
        const result = await client.request<LspDocumentHighlight[] | null>(
          "textDocument/documentHighlight",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
            position: toLspPosition(position),
          },
        );

        if (!result) {
          return null;
        }

        return result.map((highlight) => ({
          range: fromLspRange(monaco, highlight.range),
          kind: toMonacoDocumentHighlightKind(monaco, highlight.kind),
        }));
      } catch {
        return null;
      }
    },
  });
}
