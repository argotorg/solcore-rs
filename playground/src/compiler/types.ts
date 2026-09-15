export interface CompileInput {
  files: Array<{ path: string; content: string }>;
  entry: string;
  testId?: string;
  manual?: ManualCall;
  sandboxEpoch?: number;
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
  execution: ExecutionResult | null;
  tests: TestCase[];
  contracts: ContractInterface[];
  sandbox: { contract: string; address: string } | null;
}

export interface CompileRequest {
  id: number;
  kind: "compile" | "run" | "discover";
  input: CompileInput;
}

export type CompileResponse =
  | { id: number; kind: "result"; result: CompileResult }
  | { id: number; kind: "error"; message: string };

export interface ExecutionResult {
  status: "success" | "revert" | "halt" | "error";
  phase: "prepare" | "deploy" | "call";
  returnData: string;
  returnWord: string | null;
  decoded?: string | null;
  gasUsed: number;
  deploymentGasUsed: number | null;
  gasLimit: number;
  message: string | null;
}

export interface ContractInterface {
  name: string;
  constructorInputs: string[];
  methods: Array<{ signature: string; inputs: string[] }>;
}

export interface ManualCall {
  contract: string;
  signature: string;
  arguments: string;
  constructorArguments: string;
  simulate: boolean;
  reset?: boolean;
}

export interface TestCase {
  id: string;
  file: string;
  line: number;
  contract: string;
  label: string;
  invocation: { signature: string; arguments: string; simulate: boolean } | null;
  status: "ready" | "passed" | "failed" | "error";
  message: string | null;
  actual: string | null;
  expected: string | null;
  gasUsed: number | null;
}
