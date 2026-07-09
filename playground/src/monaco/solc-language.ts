import type * as Monaco from "monaco-editor";
import languageConfigurationSource from "../../../editors/vscode-solcore/language-configuration.json?raw";
import type { ThemeMode } from "../store/workspace";
import { createSolcoreTextMateTokensProvider } from "./textmate";

export const SOLCORE_LANGUAGE_ID = "solcore";
export const SOLCORE_LIGHT_THEME = "solcore-light";
export const SOLCORE_DARK_THEME = "solcore-dark";

interface Pair {
  open: string;
  close: string;
}

interface VscodeLanguageConfiguration {
  comments: {
    lineComment: string;
    blockComment: [string, string];
  };
  brackets: [string, string][];
  autoClosingPairs: Array<Pair & { notIn?: string[] }>;
  surroundingPairs: Pair[];
  indentationRules: {
    increaseIndentPattern: string;
    decreaseIndentPattern: string;
  };
  wordPattern: string;
}

const languageConfiguration = JSON.parse(
  languageConfigurationSource,
) as VscodeLanguageConfiguration;

let registered = false;

export function monacoThemeFor(theme: ThemeMode): string {
  return theme === "dark" ? SOLCORE_DARK_THEME : SOLCORE_LIGHT_THEME;
}

function solcoreMonarchFallback(): Monaco.languages.IMonarchLanguage {
  const identifier = /\p{L}[\p{L}\p{N}_]*(?:-\p{L}[\p{L}\p{N}_]*)*/u;

  return {
    defaultToken: "",
    tokenPostfix: ".solc",
    keywords: [
      "contract",
      "import",
      "export",
      "as",
      "let",
      "data",
      "class",
      "forall",
      "instance",
      "if",
      "else",
      "for",
      "switch",
      "type",
      "case",
      "default",
      "match",
      "public",
      "payable",
      "function",
      "constructor",
      "fallback",
      "return",
      "leave",
      "continue",
      "break",
      "lam",
      "assembly",
      "pragma",
    ],
    booleans: ["true", "false"],
    builtinTypes: ["word", "bool", "unit"],
    tokenizer: {
      root: [
        [/[{}()[\]]/, "@brackets"],
        [/\/\/.*$/, "comment.line.double-slash.solcore"],
        [/\/\*/, "comment.block.solcore", "@comment"],
        [/0x[0-9a-fA-F]+/, "constant.numeric.hex.solcore"],
        [/[0-9]+/, "constant.numeric.decimal.solcore"],
        [/"([^"\\]|\\.)*$/, "invalid.illegal.string.solcore"],
        [/"/, "string.quoted.double.solcore", "@string"],
        [
          new RegExp(`${identifier.source}(?=\\s*\\()`, "u"),
          {
            cases: {
              "@keywords": "keyword.declaration.solcore",
              "@booleans": "constant.language.boolean.solcore",
              "@builtinTypes": "support.type.primitive.solcore",
              "@default": "entity.name.function.call.solcore",
            },
          },
        ],
        [
          /\p{Lu}[\p{L}\p{N}_]*(?:-\p{L}[\p{L}\p{N}_]*)*/u,
          "entity.name.type.identifier.solcore",
        ],
        [
          identifier,
          {
            cases: {
              "@keywords": "keyword.declaration.solcore",
              "@booleans": "constant.language.boolean.solcore",
              "@builtinTypes": "support.type.primitive.solcore",
              "@default": "variable.other.identifier.solcore",
            },
          },
        ],
        [/:=|\+=|-=|\^=|&=|\|=|%=|=/, "keyword.operator.assignment.solcore"],
        [/->|=>/, "keyword.operator.arrow.solcore"],
        [/==|!=|>=|<=|<|>/, "keyword.operator.comparison.solcore"],
        [/&&|\|\||!/, "keyword.operator.logical.solcore"],
        [/[+\-*/%]/, "keyword.operator.arithmetic.solcore"],
        [/[|&^]/, "keyword.operator.bitwise.solcore"],
        [/[@?_]/, "keyword.operator.other.solcore"],
        [/[.:;,]/, "punctuation.separator.solcore"],
        [/[ \t\r\n]+/, "white"],
      ],
      comment: [
        [/\/\*/, "comment.block.solcore", "@push"],
        [/\*\//, "comment.block.solcore", "@pop"],
        [/[^/*]+/, "comment.block.solcore"],
        [/[/*]/, "comment.block.solcore"],
      ],
      string: [
        [/\\[nt"\\]/, "constant.character.escape.solcore"],
        [/\\./, "invalid.illegal.escape.solcore"],
        [/[^\\"]+/, "string.quoted.double.solcore"],
        [/"/, "string.quoted.double.solcore", "@pop"],
      ],
    },
  };
}

function toMonacoLanguageConfiguration(): Monaco.languages.LanguageConfiguration {
  return {
    comments: languageConfiguration.comments,
    brackets: languageConfiguration.brackets.map(
      ([open, close]) => [open, close] as Monaco.languages.CharacterPair,
    ),
    autoClosingPairs: languageConfiguration.autoClosingPairs,
    surroundingPairs: languageConfiguration.surroundingPairs,
    wordPattern: new RegExp(languageConfiguration.wordPattern, "gu"),
    indentationRules: {
      increaseIndentPattern: new RegExp(
        languageConfiguration.indentationRules.increaseIndentPattern,
      ),
      decreaseIndentPattern: new RegExp(
        languageConfiguration.indentationRules.decreaseIndentPattern,
      ),
    },
  };
}

function tokenRules(
  palette: "light" | "dark",
): Monaco.editor.ITokenThemeRule[] {
  const light = palette === "light";

  return [
    {
      token: "keyword",
      foreground: light ? "b45309" : "fb923c",
      fontStyle: "bold",
    },
    {
      token: "keyword.control",
      foreground: light ? "b45309" : "fb923c",
      fontStyle: "bold",
    },
    {
      token: "keyword.declaration",
      foreground: light ? "b45309" : "fb923c",
      fontStyle: "bold",
    },
    {
      token: "keyword.directive",
      foreground: light ? "b45309" : "fb923c",
      fontStyle: "bold",
    },
    { token: "storage.modifier", foreground: light ? "9a3412" : "fdba74" },
    { token: "constant.language", foreground: light ? "7c3aed" : "c4b5fd" },
    { token: "support.type", foreground: light ? "4f46e5" : "a5b4fc" },
    { token: "entity.name.type", foreground: light ? "4f46e5" : "a5b4fc" },
    { token: "entity.name.function", foreground: light ? "0f766e" : "5eead4" },
    { token: "entity.name.directive", foreground: light ? "7c2d12" : "fed7aa" },
    { token: "variable.other.declaration", foreground: light ? "0369a1" : "7dd3fc" },
    { token: "constant.numeric", foreground: light ? "6d28d9" : "c4b5fd" },
    { token: "string", foreground: light ? "047857" : "86efac" },
    { token: "constant.character.escape", foreground: light ? "dc2626" : "fca5a5" },
    {
      token: "invalid.illegal",
      foreground: light ? "dc2626" : "fca5a5",
      fontStyle: "underline",
    },
    {
      token: "comment",
      foreground: light ? "64748b" : "94a3b8",
      fontStyle: "italic",
    },
    { token: "keyword.operator", foreground: light ? "475569" : "cbd5e1" },
    { token: "punctuation", foreground: light ? "475569" : "cbd5e1" },
  ];
}

export function registerSolcoreLanguage(monaco: typeof Monaco): void {
  if (registered) {
    return;
  }

  monaco.languages.register({
    id: SOLCORE_LANGUAGE_ID,
    extensions: [".solc"],
    aliases: ["Solcore", "solc"],
    mimetypes: ["text/x-solcore"],
  });

  monaco.languages.registerTokensProviderFactory(SOLCORE_LANGUAGE_ID, {
    async create() {
      try {
        return await createSolcoreTextMateTokensProvider();
      } catch (error) {
        console.warn("Falling back to Monarch tokenization for Solcore", error);
        return solcoreMonarchFallback();
      }
    },
  });

  monaco.languages.setLanguageConfiguration(
    SOLCORE_LANGUAGE_ID,
    toMonacoLanguageConfiguration(),
  );

  monaco.editor.defineTheme(SOLCORE_LIGHT_THEME, {
    base: "vs",
    inherit: true,
    rules: tokenRules("light"),
    colors: {
      "editor.background": "#ffffff",
      "editor.foreground": "#1f2937",
      "editorLineNumber.foreground": "#94a3b8",
      "editorLineNumber.activeForeground": "#b45309",
      "editorCursor.foreground": "#b45309",
      "editor.selectionBackground": "#fed7aa80",
      "editor.inactiveSelectionBackground": "#e2e8f080",
      "editor.lineHighlightBackground": "#f8fafc",
      "editorGutter.background": "#ffffff",
      "editorIndentGuide.background1": "#e2e8f0",
      "editorIndentGuide.activeBackground1": "#cbd5e1",
      "editorOverviewRuler.border": "#e2e8f0",
    },
  });

  monaco.editor.defineTheme(SOLCORE_DARK_THEME, {
    base: "vs-dark",
    inherit: true,
    rules: tokenRules("dark"),
    colors: {
      "editor.background": "#111318",
      "editor.foreground": "#e5e7eb",
      "editorLineNumber.foreground": "#64748b",
      "editorLineNumber.activeForeground": "#fb923c",
      "editorCursor.foreground": "#fb923c",
      "editor.selectionBackground": "#9a341280",
      "editor.inactiveSelectionBackground": "#33415580",
      "editor.lineHighlightBackground": "#171a21",
      "editorGutter.background": "#111318",
      "editorIndentGuide.background1": "#293241",
      "editorIndentGuide.activeBackground1": "#475569",
      "editorOverviewRuler.border": "#242936",
    },
  });

  registered = true;
}
