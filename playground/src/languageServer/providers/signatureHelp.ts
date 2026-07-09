import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";

interface LspParameterInformation {
  label: string | [number, number];
}

interface LspSignatureInformation {
  label: string;
  parameters?: LspParameterInformation[];
}

interface LspSignatureHelp {
  signatures: LspSignatureInformation[];
  activeSignature?: number;
  activeParameter?: number;
}

function toSignatureHelp(result: LspSignatureHelp): Monaco.languages.SignatureHelp {
  return {
    signatures: result.signatures.map((signature) => ({
      label: signature.label,
      parameters:
        signature.parameters?.map((parameter) => ({
          label: parameter.label,
        })) ?? [],
    })),
    activeSignature: result.activeSignature ?? 0,
    activeParameter: result.activeParameter ?? 0,
  };
}

export function registerSignatureHelp(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerSignatureHelpProvider(SOLCORE_LANGUAGE_ID, {
    signatureHelpTriggerCharacters: ["(", ","],

    async provideSignatureHelp(
      model,
      position,
    ): Promise<Monaco.languages.SignatureHelpResult | null> {
      try {
        const result = await client.request<LspSignatureHelp | null>(
          "textDocument/signatureHelp",
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

        return {
          value: toSignatureHelp(result),
          dispose() {},
        };
      } catch {
        return null;
      }
    },
  });
}
