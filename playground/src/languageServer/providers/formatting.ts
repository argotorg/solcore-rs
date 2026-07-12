import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";

interface LspFormattingOptions {
  tabSize: number;
  insertSpaces: boolean;
}

interface LspTextEdit {
  range: LspRange;
  newText: string;
}

function toLspFormattingOptions(
  options: Monaco.languages.FormattingOptions,
): LspFormattingOptions {
  // Monaco exposes only these two standard LSP formatting options. Its
  // `trimAutoWhitespace` model option has different semantics from LSP's
  // `trimTrailingWhitespace`, so it must not be forwarded as a substitute.
  return {
    tabSize: options.tabSize,
    insertSpaces: options.insertSpaces,
  };
}

export function registerFormatting(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerDocumentFormattingEditProvider(
    SOLCORE_LANGUAGE_ID,
    {
      displayName: "Solcore",

      async provideDocumentFormattingEdits(model, options, token) {
        if (token.isCancellationRequested) {
          return null;
        }

        try {
          const result = await client.request<LspTextEdit[] | null>(
            "textDocument/formatting",
            {
              textDocument: {
                uri: model.uri.toString(),
              },
              options: toLspFormattingOptions(options),
            },
          );

          if (token.isCancellationRequested || !result) {
            return null;
          }

          return result.map((edit) => ({
            range: fromLspRange(monaco, edit.range),
            text: edit.newText,
          }));
        } catch {
          return null;
        }
      },
    },
  );
}
