# solcore playground

A React + TypeScript + Vite frontend for the solcore-rs compiler Playground. The compile path runs in a Web Worker and calls the generated `solcore-wasm` package from `../crates/wasm/pkg`. Editor language features run in a separate LSP Worker backed by `solcore-lsp` from `../crates/lsp/pkg`.

## Development

On a fresh checkout, generate the local wasm packages once before installing JavaScript dependencies:

```sh
npm run build:wasm
npm install
npm run dev
```

After dependencies are installed, `npm run dev` rebuilds both local wasm packages before starting Vite:

- `../crates/wasm/pkg` for compiler/runtime calls
- `../crates/lsp/pkg` for Monaco language features

`package.json` depends on `solcore-wasm` and `solcore-lsp` through `file:` dependencies, so a normal install links the generated wasm-pack output into Vite without publishing it. If a dev server was already running while rebuilding wasm, restart with `npm run dev:force` once to clear Vite's dependency cache.

## Build

```sh
npm run build
```

The build script rebuilds both wasm packages, runs `tsc --noEmit`, then runs `vite build`.

## Deploy

Deploy the generated `dist/` directory with static hosting that serves `.wasm` files. Vite emits the compiler and LSP wasm files as assets and rewrites the worker imports to those built assets.

For static hosting under a subpath, set `VITE_BASE`:

```sh
VITE_BASE=/solcore-rs/ npm run build
```

## WASM packages

Rebuild the sibling wasm crates whenever the compiler or LSP changes. Preferred (size-optimized):

```sh
npm run build:wasm     # from playground/: compiler wasm + LSP wasm + optional `wasm-opt -Oz`
```

To rebuild only one package:

```sh
npm run build:compiler-wasm
npm run build:lsp-wasm
```

Or directly from the repository root:

```sh
wasm-pack build --target web crates/wasm --out-dir pkg --profile wasm-release
wasm-pack build --target web crates/lsp --out-dir pkg --profile wasm-release -- --features wasm
```

That produces `crates/wasm/pkg/` and `crates/lsp/pkg/`. The workspace `[profile.wasm-release]` keeps
browser builds size-tuned (`strip` + `opt-level = "z"` + fat `lto`); the normal `[profile.release]`
is instead tuned for native compiler throughput. A final `wasm-opt -Oz` pass (requires
`brew install binaryen`; wasm-pack's bundled wasm-opt is too old for reference-types) reduces the
generated package further. `npm run build:wasm` applies it automatically when `wasm-opt` is on
`PATH` and skips it gracefully otherwise; `vite build` reports the current raw and gzipped asset
sizes. The Playground imports `init`,
`compile`, `std_files`, and `version` from `solcore-wasm`; `src/compiler/runtime.ts` passes Vite's emitted
`solcore_wasm_bg.wasm?url` asset to `init()` and caches initialization. The LSP worker imports
`SolcoreLsp` from `solcore-lsp`; `src/languageServer/lsp.worker.ts` passes Vite's emitted
`solcore_lsp_bg.wasm?url` asset to `init()`. The shared compiler API shape lives in
`src/compiler/types.ts` and should stay the single source of truth for the Playground compile protocol.

The compile worker protocol is intentionally Playground-specific batch compile messaging:

```ts
// request
{ id: number; kind: "compile"; input: CompileInput }

interface CompileInput {
  files: Array<{ path: string; content: string }>;
  entry: string;
  options: {
    emitHull: boolean;
    emitYul: boolean;
    emitSonatina: boolean;
    emitAbi: boolean;
  };
}

// response
{ id: number; kind: "result"; result: CompileResult }
{ id: number; kind: "error"; message: string }

interface CompileResult {
  success: boolean;
  diagnostics: Diag[];
  hull: string | null;
  yul: string | null;
  sonatina: string | null;
  abi: string | null;
}
```

The Playground requests Hull, Yul, and Sonatina IR in one compile and exposes each textual output in
its own tab. Backend fields remain `null` when an output was not requested or compilation stopped
before that backend ran.

## File key contract

The canonical file key is always a workspace-relative path string, for example `main.solc` or `sub/Foo.solc`.

Use that exact key everywhere:

- Zustand `files` record keys
- tab ids
- active and entry file values
- compile request `{ path }` and `entry`

Do not use `file://` URIs or leading slashes as store keys. Monaco model URIs may use `file:///main/<relpath>` internally, but must map back to the same relative key.

## Language Server

`src/languageClient/` re-exports the active browser LSP integration from `src/languageServer/`. The LSP runs in `src/languageServer/lsp.worker.ts`, separate from the compile worker, and speaks standard LSP JSON-RPC 2.0 over `postMessage`. LSP positions are standard 0-based line/character values with UTF-16 character offsets.

The URI/key mapping for LSP documents is:

```text
file:///main/<relpath> <-> <relpath>
```

Compile diagnostics reserve Monaco marker owner `"solcore-compile"`:

```ts
monaco.editor.setModelMarkers(model, "solcore-compile", markers);
```

LSP diagnostics use Monaco marker owner `"solcore-lsp"` so compile diagnostics and LSP diagnostics can coexist.
