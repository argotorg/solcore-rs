import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";
import {
  markdownForHoverContents,
  type LspHoverContent,
} from "./hoverContent.js";

interface LspHover {
  contents: LspHoverContent | LspHoverContent[];
  range?: LspRange;
}

export function registerHover(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerHoverProvider(SOLCORE_LANGUAGE_ID, {
    async provideHover(model, position): Promise<Monaco.languages.Hover | null> {
      try {
        const result = await client.request<LspHover | null>("textDocument/hover", {
          textDocument: {
            uri: model.uri.toString(),
          },
          position: toLspPosition(position),
        });

        if (!result) {
          return null;
        }

        return {
          contents: markdownForHoverContents(result.contents),
          range: result.range ? fromLspRange(monaco, result.range) : undefined,
        };
      } catch {
        return null;
      }
    },
  });
}
