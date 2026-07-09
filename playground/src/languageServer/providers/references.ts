import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspLocation, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspLocation } from "../protocol";

export function registerReferences(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerReferenceProvider(SOLCORE_LANGUAGE_ID, {
    async provideReferences(model, position, context) {
      try {
        const result = await client.request<LspLocation[] | null>(
          "textDocument/references",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
            position: toLspPosition(position),
            context: {
              includeDeclaration: context.includeDeclaration,
            },
          },
        );

        if (!result) {
          return null;
        }

        return result.map((location) => fromLspLocation(monaco, location));
      } catch {
        return null;
      }
    },
  });
}
