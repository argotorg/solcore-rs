import type * as Monaco from "monaco-editor";
import type { ThemeMode } from "../store/workspace";

export const SOLCORE_LANGUAGE_ID = "solcore";
export const SOLCORE_LIGHT_THEME = "solcore-light";
export const SOLCORE_DARK_THEME = "solcore-dark";

let registered = false;

export function monacoThemeFor(theme: ThemeMode): string {
  return theme === "dark" ? SOLCORE_DARK_THEME : SOLCORE_LIGHT_THEME;
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

  monaco.languages.setMonarchTokensProvider(SOLCORE_LANGUAGE_ID, {
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
    operators: [
      ":=",
      "->",
      "=>",
      "==",
      "!=",
      ">=",
      "<=",
      "&&",
      "||",
      "+=",
      "-=",
      "^=",
      "&=",
      "|=",
      "%=",
      "+",
      "-",
      "*",
      "/",
      "%",
      "!",
      "<",
      ">",
      "=",
      "|",
      "&",
      "^",
      "@",
      "?",
      ".",
      ":",
      ";",
      ",",
      "_",
    ],
    tokenizer: {
      root: [
        [/[{}()[\]]/, "@brackets"],
        [/\/\/.*$/, "comment"],
        [/\/\*/, "comment", "@comment"],
        [/0x[0-9a-fA-F]+/, "number.hex"],
        [/[0-9]+/, "number"],
        [/"([^"\\]|\\.)*$/, "string.invalid"],
        [/"/, "string", "@string"],
        [
          /\p{L}[\p{L}\p{N}_]*(?=\s*\()/u,
          {
            cases: {
              "@keywords": "keyword",
              "@booleans": "constant.language",
              "@builtinTypes": "type.identifier",
              "@default": "entity.name.function",
            },
          },
        ],
        [/\p{Lu}[\p{L}\p{N}_]*/u, "type.identifier"],
        [
          /\p{L}[\p{L}\p{N}_]*/u,
          {
            cases: {
              "@keywords": "keyword",
              "@booleans": "constant.language",
              "@builtinTypes": "type.identifier",
              "@default": "identifier",
            },
          },
        ],
        [/:=|->|=>|==|!=|>=|<=|&&|\|\||\+=|-=|\^=|&=|\|=|%=|[+\-*/%!<>=|&^@?.:;,_]/, "operator"],
        [/[ \t\r\n]+/, "white"],
      ],
      comment: [
        [/[^/*]+/, "comment"],
        [/\*\//, "comment", "@pop"],
        [/[/*]/, "comment"],
      ],
      string: [
        [/[^\\"]+/, "string"],
        [/\\./, "string.escape"],
        [/"/, "string", "@pop"],
      ],
    },
  });

  monaco.languages.setLanguageConfiguration(SOLCORE_LANGUAGE_ID, {
    comments: {
      lineComment: "//",
      blockComment: ["/*", "*/"],
    },
    brackets: [
      ["{", "}"],
      ["[", "]"],
      ["(", ")"],
    ],
    autoClosingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: "(", close: ")" },
      { open: '"', close: '"', notIn: ["string", "comment"] },
    ],
    surroundingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: "(", close: ")" },
      { open: '"', close: '"' },
    ],
    wordPattern: /[\p{L}][\p{L}\p{N}_]*/gu,
    indentationRules: {
      increaseIndentPattern: /^.*\{[^}"']*$/,
      decreaseIndentPattern: /^\s*\}/,
    },
  });

  monaco.editor.defineTheme(SOLCORE_LIGHT_THEME, {
    base: "vs",
    inherit: true,
    rules: [
      { token: "keyword", foreground: "b45309", fontStyle: "bold" },
      { token: "constant.language", foreground: "7c3aed" },
      { token: "type.identifier", foreground: "4f46e5" },
      { token: "entity.name.function", foreground: "0f766e" },
      { token: "number", foreground: "6d28d9" },
      { token: "number.hex", foreground: "6d28d9" },
      { token: "string", foreground: "047857" },
      { token: "string.escape", foreground: "dc2626" },
      { token: "string.invalid", foreground: "dc2626", fontStyle: "underline" },
      { token: "comment", foreground: "64748b", fontStyle: "italic" },
      { token: "operator", foreground: "475569" },
    ],
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
    rules: [
      { token: "keyword", foreground: "fb923c", fontStyle: "bold" },
      { token: "constant.language", foreground: "c4b5fd" },
      { token: "type.identifier", foreground: "a5b4fc" },
      { token: "entity.name.function", foreground: "5eead4" },
      { token: "number", foreground: "c4b5fd" },
      { token: "number.hex", foreground: "c4b5fd" },
      { token: "string", foreground: "86efac" },
      { token: "string.escape", foreground: "fca5a5" },
      { token: "string.invalid", foreground: "fca5a5", fontStyle: "underline" },
      { token: "comment", foreground: "94a3b8", fontStyle: "italic" },
      { token: "operator", foreground: "cbd5e1" },
    ],
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
