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
    emitBytecode?: boolean;
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
  yulOutputs: Array<{ name: string; code: string }>;
  sonatina: string | null;
  abi: string | null;
  bytecode: Array<{ name: string; sections: Array<{ name: string; code: string }> }>;
  execution: ExecutionResult | null;
  tests: TestCase[];
  contracts: ContractInterface[];
  hasMain: boolean;
  sandbox: { id: number; contract: string; address: string } | null;
  events: ExecutionEvent[];
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
  methods: Array<{ signature: string; inputs: string[]; returnsValue: boolean }>;
}

export interface ManualCall {
  contract: string;
  signature: string;
  arguments: string;
  constructorArguments: string;
  simulate: boolean;
  reset?: boolean;
}

export interface ExecutionEvent {
  kind: "deploy" | "call" | "setup" | "check";
  contract: string;
  signature: string;
  arguments: string;
  simulate: boolean;
  testId: string | null;
  expected: string | null;
  passed: boolean | null;
  result: ExecutionResult;
}

export interface RecentAction {
  id: number;
  label: string;
  version: number;
  sandboxId: number | null;
  events: ExecutionEvent[];
  result: ExecutionResult | null;
  message: string | null;
}

export interface TestCase {
  replayed?: boolean;
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

export interface WatchCall {
  id: string;
  contract: string;
  signature: string;
  arguments: string;
}
export interface WatchResult {
  id: string;
  value: string | null;
  error: string | null;
}
export interface WatchInput {
  workspace: CompileInput;
  watches: WatchCall[];
}
export type WorkerRequest = CompileRequest | { id: number; kind: "watch"; input: WatchInput };
export type WorkerResponse = CompileResponse | { id: number; kind: "watch-result"; result: WatchResult[] };
