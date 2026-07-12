import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import type { LspClient } from "../lspClient";

interface LspFoldingRange {
  startLine: number;
  startCharacter?: number;
  endLine: number;
  endCharacter?: number;
  kind?: string;
  collapsedText?: string;
}

function toMonacoFoldingRangeKind(
  monaco: typeof Monaco,
  kind: string | undefined,
): Monaco.languages.FoldingRangeKind | undefined {
  return kind === undefined
    ? undefined
    : monaco.languages.FoldingRangeKind.fromValue(kind);
}

function toMonacoFoldingRange(
  monaco: typeof Monaco,
  range: LspFoldingRange,
): Monaco.languages.FoldingRange {
  return {
    // LSP line positions are zero-based, while Monaco folding ranges are
    // one-based. Monaco's API is line-only, so character fields are omitted.
    start: range.startLine + 1,
    end: range.endLine + 1,
    kind: toMonacoFoldingRangeKind(monaco, range.kind),
  };
}

export function registerFolding(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerFoldingRangeProvider(SOLCORE_LANGUAGE_ID, {
    async provideFoldingRanges(model, _context, token) {
      if (token.isCancellationRequested) {
        return null;
      }

      try {
        const result = await client.request<LspFoldingRange[] | null>(
          "textDocument/foldingRange",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
          },
        );

        if (token.isCancellationRequested || !result) {
          return null;
        }

        return result.map((range) => toMonacoFoldingRange(monaco, range));
      } catch {
        return null;
      }
    },
  });
}
