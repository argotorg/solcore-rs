import init, {
  compile as wasmCompile,
  run as wasmRun,
  set_solc as wasmSetSolc,
  watch as wasmWatch,
  std_files as wasmStdFiles,
  version as wasmVersion,
} from "solcore-wasm";
import wasmUrl from "solcore-wasm/solcore_wasm_bg.wasm?url";
import { bindSolc } from "./solcBinding";
import { SOLJSON_FILE } from "./solcRelease";
import type { CompileInput, CompileResult, WatchInput, WatchResult } from "./types";

export interface StdFile {
  path: string;
  content: string;
}

let initPromise: Promise<void> | null = null;
let versionPromise: Promise<string> | null = null;
let stdFilesPromise: Promise<StdFile[]> | null = null;
let solcPromise: Promise<string> | null = null;

export function initializeCompiler(): Promise<void> {
  if (!initPromise) {
    initPromise = init({ module_or_path: wasmUrl }).then(() => undefined);
  }

  return initPromise;
}

/** Whether a request needs the solc backend to generate bytecode. */
export function needsSolc(input: CompileInput, execute: boolean): boolean {
  return (input.options.backend ?? "solc") === "solc" && (execute || Boolean(input.options.emitBytecode));
}

/**
 * Loads soljson in this worker and registers it with the compiler wasm.
 * Resolves to the solc version string. A failed load can be retried.
 */
export function initializeSolc(): Promise<string> {
  solcPromise ??= (async () => {
    const url = `${import.meta.env.BASE_URL}solc/${SOLJSON_FILE}`;
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`Could not load solc from ${url} (HTTP ${response.status}).`);
    }
    const source = await response.text();
    // soljson is a classic script that defines a global `Module`. Indirect eval
    // runs it in global scope, which module workers cannot do with importScripts.
    (0, eval)(source);
    const solc = bindSolc((globalThis as { Module?: unknown }).Module);
    await initializeCompiler();
    wasmSetSolc(solc.compile);
    return solc.version;
  })().catch((error: unknown) => {
    solcPromise = null;
    throw error;
  });
  return solcPromise;
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

export function run(input: CompileInput): CompileResult {
  return wasmRun(input) as CompileResult;
}

export function watch(input: WatchInput): WatchResult[] {
  return wasmWatch(input) as WatchResult[];
}
