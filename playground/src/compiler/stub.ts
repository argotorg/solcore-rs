import type { CompileInput, CompileResult, Pos } from "./types";

const encoder = new TextEncoder();

function locateNeedle(file: string, content: string, needle: string): Pos | null {
  const startIndex = content.indexOf(needle);
  if (startIndex < 0) {
    return null;
  }

  const before = content.slice(0, startIndex);
  const lineBreaks = before.match(/\n/g);
  const startLine = (lineBreaks?.length ?? 0) + 1;
  const lastLineBreak = before.lastIndexOf("\n");
  const startCol = startIndex - lastLineBreak;
  const endCol = startCol + needle.length;

  return {
    file,
    startByte: encoder.encode(content.slice(0, startIndex)).length,
    endByte: encoder.encode(content.slice(0, startIndex + needle.length)).length,
    startLine,
    startCol,
    endLine: startLine,
    endCol,
  };
}

export function stubCompile(input: CompileInput): CompileResult {
  const entryFile = input.files.find((file) => file.path === input.entry);
  const entryContent = entryFile?.content ?? "";
  const boolRange = locateNeedle(input.entry, entryContent, "return true");

  if (boolRange) {
    const trueRange: Pos = {
      ...boolRange,
      startByte: boolRange.startByte + encoder.encode("return ").length,
      startCol: boolRange.startCol + "return ".length,
    };

    return {
      success: false,
      diagnostics: [
        {
          severity: "error",
          code: "SC0000",
          message: "mismatched types: expected word, found bool",
          primary: trueRange,
          labels: [
            {
              range: trueRange,
              message: "this expression has type bool",
              isPrimary: true,
            },
          ],
          notes: ["The stub compiler emits this diagnostic for `return true`."],
          helps: ["Try returning a word literal such as `42`."],
        },
      ],
      hull: null,
      yul: null,
      sonatina: null,
      abi: null,
    };
  }

  return {
    success: true,
    diagnostics: [],
    hull: `// hull for ${input.entry}\nfunction main() { }`,
    yul: 'object "Output" { code { } }',
    sonatina: `; Sonatina IR for ${input.entry}\nfunc private %main() {}`,
    abi: null,
  };
}

export function version(): string {
  return "0.1.0";
}

export function std_files(): Array<{ path: string; content: string }> {
  return [];
}
