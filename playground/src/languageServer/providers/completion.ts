import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";

interface LspMarkupContent {
  kind: string;
  value: string;
}

interface LspCompletionItem {
  label: string;
  kind?: number;
  detail?: string;
  documentation?: string | LspMarkupContent;
  insertText?: string;
}

interface LspCompletionList {
  items: LspCompletionItem[];
}

type LspCompletionResult = LspCompletionItem[] | LspCompletionList;

function completionKind(
  monaco: typeof Monaco,
  kind: number | undefined,
): Monaco.languages.CompletionItemKind {
  switch (kind) {
    case 1:
      return monaco.languages.CompletionItemKind.Text;
    case 2:
      return monaco.languages.CompletionItemKind.Method;
    case 3:
      return monaco.languages.CompletionItemKind.Function;
    case 4:
      return monaco.languages.CompletionItemKind.Constructor;
    case 5:
      return monaco.languages.CompletionItemKind.Field;
    case 6:
      return monaco.languages.CompletionItemKind.Variable;
    case 7:
      return monaco.languages.CompletionItemKind.Class;
    case 8:
      return monaco.languages.CompletionItemKind.Interface;
    case 9:
      return monaco.languages.CompletionItemKind.Module;
    case 10:
      return monaco.languages.CompletionItemKind.Property;
    case 11:
      return monaco.languages.CompletionItemKind.Unit;
    case 12:
      return monaco.languages.CompletionItemKind.Value;
    case 13:
      return monaco.languages.CompletionItemKind.Enum;
    case 14:
      return monaco.languages.CompletionItemKind.Keyword;
    case 15:
      return monaco.languages.CompletionItemKind.Snippet;
    case 16:
      return monaco.languages.CompletionItemKind.Color;
    case 17:
      return monaco.languages.CompletionItemKind.File;
    case 18:
      return monaco.languages.CompletionItemKind.Reference;
    case 19:
      return monaco.languages.CompletionItemKind.Folder;
    case 20:
      return monaco.languages.CompletionItemKind.EnumMember;
    case 21:
      return monaco.languages.CompletionItemKind.Constant;
    case 22:
      return monaco.languages.CompletionItemKind.Struct;
    case 23:
      return monaco.languages.CompletionItemKind.Event;
    case 24:
      return monaco.languages.CompletionItemKind.Operator;
    case 25:
      return monaco.languages.CompletionItemKind.TypeParameter;
    default:
      return monaco.languages.CompletionItemKind.Text;
  }
}

function completionItems(result: LspCompletionResult | null): LspCompletionItem[] {
  if (!result) {
    return [];
  }

  return Array.isArray(result) ? result : result.items;
}

function documentationForItem(
  documentation: LspCompletionItem["documentation"],
): string | Monaco.IMarkdownString | undefined {
  if (!documentation) {
    return undefined;
  }

  if (typeof documentation === "string") {
    return documentation;
  }

  return { value: documentation.value };
}

export function registerCompletion(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerCompletionItemProvider(SOLCORE_LANGUAGE_ID, {
    triggerCharacters: ["."],

    async provideCompletionItems(model, position): Promise<Monaco.languages.CompletionList | null> {
      try {
        const result = await client.request<LspCompletionResult | null>(
          "textDocument/completion",
          {
            textDocument: {
              uri: model.uri.toString(),
            },
            position: toLspPosition(position),
          },
        );
        const word = model.getWordUntilPosition(position);
        const range = new monaco.Range(
          position.lineNumber,
          word.startColumn,
          position.lineNumber,
          word.endColumn,
        );

        return {
          suggestions: completionItems(result).map((item) => ({
            label: item.label,
            kind: completionKind(monaco, item.kind),
            detail: item.detail,
            documentation: documentationForItem(item.documentation),
            insertText: item.insertText ?? item.label,
            range,
          })),
        };
      } catch {
        return null;
      }
    },
  });
}
