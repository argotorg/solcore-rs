import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import type { LspClient } from "../lspClient";

interface LspSemanticTokens {
  resultId?: string;
  data: number[];
}

export function registerSemanticTokens(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerDocumentSemanticTokensProvider(
    SOLCORE_LANGUAGE_ID,
    {
      getLegend() {
        return {
          tokenTypes: [
            "keyword",
            "function",
            "type",
            "variable",
            "parameter",
            "property",
            "enumMember",
            "namespace",
            "number",
            "string",
            "operator",
            "comment",
          ],
          tokenModifiers: ["declaration", "readonly"],
        };
      },
      async provideDocumentSemanticTokens(model) {
        try {
          const result = await client.request<LspSemanticTokens | null>(
            "textDocument/semanticTokens/full",
            {
              textDocument: {
                uri: model.uri.toString(),
              },
            },
          );

          if (!result) {
            return null;
          }

          return {
            data: new Uint32Array(result.data),
            resultId: result.resultId,
          };
        } catch {
          return null;
        }
      },
      releaseDocumentSemanticTokens() {},
    },
  );
}
