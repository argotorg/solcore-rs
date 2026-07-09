import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspRange } from "../protocol";

interface LspDocumentSymbol {
  name: string;
  detail?: string;
  kind: number;
  range: LspRange;
  selectionRange: LspRange;
  children?: LspDocumentSymbol[];
}

function toSymbolKind(
  monaco: typeof Monaco,
  kind: number,
): Monaco.languages.SymbolKind {
  switch (kind) {
    case 1:
      return monaco.languages.SymbolKind.File;
    case 2:
      return monaco.languages.SymbolKind.Module;
    case 3:
      return monaco.languages.SymbolKind.Namespace;
    case 4:
      return monaco.languages.SymbolKind.Package;
    case 5:
      return monaco.languages.SymbolKind.Class;
    case 6:
      return monaco.languages.SymbolKind.Method;
    case 7:
      return monaco.languages.SymbolKind.Property;
    case 8:
      return monaco.languages.SymbolKind.Field;
    case 9:
      return monaco.languages.SymbolKind.Constructor;
    case 10:
      return monaco.languages.SymbolKind.Enum;
    case 11:
      return monaco.languages.SymbolKind.Interface;
    case 12:
      return monaco.languages.SymbolKind.Function;
    case 13:
      return monaco.languages.SymbolKind.Variable;
    case 14:
      return monaco.languages.SymbolKind.Constant;
    case 15:
      return monaco.languages.SymbolKind.String;
    case 16:
      return monaco.languages.SymbolKind.Number;
    case 17:
      return monaco.languages.SymbolKind.Boolean;
    case 18:
      return monaco.languages.SymbolKind.Array;
    case 19:
      return monaco.languages.SymbolKind.Object;
    case 20:
      return monaco.languages.SymbolKind.Key;
    case 21:
      return monaco.languages.SymbolKind.Null;
    case 22:
      return monaco.languages.SymbolKind.EnumMember;
    case 23:
      return monaco.languages.SymbolKind.Struct;
    case 24:
      return monaco.languages.SymbolKind.Event;
    case 25:
      return monaco.languages.SymbolKind.Operator;
    case 26:
      return monaco.languages.SymbolKind.TypeParameter;
    default:
      return monaco.languages.SymbolKind.Variable;
  }
}

function toDocumentSymbol(
  monaco: typeof Monaco,
  symbol: LspDocumentSymbol,
): Monaco.languages.DocumentSymbol {
  return {
    name: symbol.name,
    detail: symbol.detail ?? "",
    kind: toSymbolKind(monaco, symbol.kind),
    tags: [],
    range: fromLspRange(monaco, symbol.range),
    selectionRange: fromLspRange(monaco, symbol.selectionRange),
    children: symbol.children?.map((child) => toDocumentSymbol(monaco, child)),
  };
}

export function registerDocumentSymbol(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerDocumentSymbolProvider(SOLCORE_LANGUAGE_ID, {
    async provideDocumentSymbols(model) {
      try {
        const result = await client.request<LspDocumentSymbol[] | null>(
          "textDocument/documentSymbol",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
          },
        );

        return result?.map((symbol) => toDocumentSymbol(monaco, symbol)) ?? null;
      } catch {
        return null;
      }
    },
  });
}
