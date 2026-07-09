import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";

interface LspMarkupContent {
  kind: string;
  value: string;
}

interface LspMarkedString {
  language: string;
  value: string;
}

type LspHoverContent = LspMarkedString | LspMarkupContent | string;

interface LspHover {
  contents: LspHoverContent;
  range?: LspRange;
}

function markdownForContent(content: LspHoverContent): Monaco.IMarkdownString {
  if (typeof content === "string") {
    return { value: content };
  }

  if ("language" in content) {
    return { value: `\`\`\`${content.language}\n${content.value}\n\`\`\`` };
  }

  return { value: content.value };
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
          contents: [markdownForContent(result.contents)],
          range: result.range ? fromLspRange(monaco, result.range) : undefined,
        };
      } catch {
        return null;
      }
    },
  });
}
