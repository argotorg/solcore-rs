import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspLocation, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspLocation } from "../protocol";

type LspDefinitionResult = LspLocation | LspLocation[] | null;

export function registerDefinition(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerDefinitionProvider(SOLCORE_LANGUAGE_ID, {
    async provideDefinition(model, position) {
      try {
        const result = await client.request<LspDefinitionResult>(
          "textDocument/definition",
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

        if (Array.isArray(result)) {
          return result.map((location) => fromLspLocation(monaco, location));
        }

        return fromLspLocation(monaco, result);
      } catch {
        return null;
      }
    },
  });
}
