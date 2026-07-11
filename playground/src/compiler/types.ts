export interface CompileInput {
  files: Array<{ path: string; content: string }>;
  entry: string;
  options: {
    emitHull: boolean;
    emitYul: boolean;
    emitSonatina: boolean;
    emitAbi: boolean;
  };
}

export type Severity = "error" | "warning" | "note" | "help";

export interface Pos {
  file: string;
  startByte: number;
  endByte: number;
  startLine: number;
  startCol: number;
  endLine: number;
  endCol: number;
}

export interface Diag {
  severity: Severity;
  code: string | null;
  message: string;
  primary: Pos | null;
  labels: Array<{ range: Pos; message: string | null; isPrimary: boolean }>;
  notes: string[];
  helps: string[];
}

export interface CompileResult {
  success: boolean;
  diagnostics: Diag[];
  hull: string | null;
  yul: string | null;
  sonatina: string | null;
  abi: string | null;
}

export interface CompileRequest {
  id: number;
  kind: "compile";
  input: CompileInput;
}

export type CompileResponse =
  | { id: number; kind: "result"; result: CompileResult }
  | { id: number; kind: "error"; message: string };
