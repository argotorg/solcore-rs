import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspPosition, toLspRange } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspPosition } from "../protocol";

interface LspInlayHintLabelPart {
  value: string;
}

interface LspInlayHint {
  position: LspPosition;
  label: string | LspInlayHintLabelPart[];
  kind?: 1 | 2;
}

function toMonacoInlayHintKind(
  monaco: typeof Monaco,
  kind: LspInlayHint["kind"],
): Monaco.languages.InlayHintKind | undefined {
  switch (kind) {
    case 1:
      return monaco.languages.InlayHintKind.Type;
    case 2:
      return monaco.languages.InlayHintKind.Parameter;
    default:
      return undefined;
  }
}

function toMonacoInlayHintLabel(
  label: LspInlayHint["label"],
): Monaco.languages.InlayHint["label"] {
  if (typeof label === "string") {
    return label;
  }

  return label.map((part) => ({
    label: part.value,
  }));
}

export function registerInlayHints(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerInlayHintsProvider(SOLCORE_LANGUAGE_ID, {
    async provideInlayHints(model, range) {
      try {
        const result = await client.request<LspInlayHint[] | null>(
          "textDocument/inlayHint",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
            range: toLspRange(range),
          },
        );

        if (!result) {
          return null;
        }

        return {
          hints: result.map((hint) => ({
            label: toMonacoInlayHintLabel(hint.label),
            position: fromLspPosition(monaco, hint.position),
            kind: toMonacoInlayHintKind(monaco, hint.kind),
          })),
          dispose() {},
        };
      } catch {
        return null;
      }
    },
  });
}
