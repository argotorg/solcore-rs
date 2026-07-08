import init, {
  compile as wasmCompile,
  std_files as wasmStdFiles,
  version as wasmVersion,
} from "solcore-wasm";
import wasmUrl from "solcore-wasm/solcore_wasm_bg.wasm?url";
import type { CompileInput, CompileResult } from "./types";

export interface StdFile {
  path: string;
  content: string;
}

let initPromise: Promise<void> | null = null;
let versionPromise: Promise<string> | null = null;
let stdFilesPromise: Promise<StdFile[]> | null = null;

export function initializeCompiler(): Promise<void> {
  if (!initPromise) {
    initPromise = init(wasmUrl).then(() => undefined);
  }

  return initPromise;
}

export function compile(input: CompileInput): CompileResult {
  return wasmCompile(input) as CompileResult;
}

export function version(): Promise<string> {
  versionPromise ??= initializeCompiler().then(() => wasmVersion());
  return versionPromise;
}

export function std_files(): Promise<StdFile[]> {
  stdFilesPromise ??= initializeCompiler().then(() => wasmStdFiles() as StdFile[]);
  return stdFilesPromise;
}
