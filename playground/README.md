# solcore playground

A React + TypeScript + Vite frontend for the solcore-rs compiler Playground. The compile path runs in a Web Worker and calls the generated `solcore-wasm` package from `../crates/wasm/pkg`.

## Development

The local wasm package and `node_modules` must already exist:

```sh
npm install
npm run dev
```

`package.json` depends on `solcore-wasm` through `file:../crates/wasm/pkg`, so a normal install links the generated wasm-pack output into Vite without publishing it.

## Build

```sh
npm run build
```

The build script runs `tsc --noEmit` first, then `vite build`.

## Deploy

Deploy the generated `dist/` directory with static hosting that serves `.wasm` files. Vite emits the compiler wasm as an asset and rewrites the worker import to that built asset.

For static hosting under a subpath, set `VITE_BASE`:

```sh
VITE_BASE=/solcore-rs/ npm run build
```

## WASM compiler

The sibling wasm crate is built from the repository root when the compiler changes:

```sh
wasm-pack build --target web crates/wasm
```

That produces `crates/wasm/pkg/`. The Playground imports `init`, `compile`, `std_files`, and `version` from `solcore-wasm`; `src/compiler/runtime.ts` passes Vite's emitted `solcore_wasm_bg.wasm?url` asset to `init()` and caches initialization. The shared API shape lives in `src/compiler/types.ts` and should stay the single source of truth for the Playground compile protocol.

The compile worker protocol is intentionally Playground-specific batch compile messaging:

```ts
// request
{ id: number; kind: "compile"; input: CompileInput }

// response
{ id: number; kind: "result"; result: CompileResult }
{ id: number; kind: "error"; message: string }
```

## File key contract

The canonical file key is always a workspace-relative path string, for example `main.solc` or `sub/Foo.solc`.

Use that exact key everywhere:

- Zustand `files` record keys
- tab ids
- active and entry file values
- compile request `{ path }` and `entry`

Do not use `file://` URIs or leading slashes as store keys. Monaco model URIs may use `file:///main/<relpath>` internally, but must map back to the same relative key.

## Future LSP integration

`src/languageClient/` contains a documented no-op `attachLanguageClient()` seam.

The future LSP must be a separate Worker from the compile worker and should speak standard LSP JSON-RPC 2.0 over `postMessage`. It should be consumed via `monaco-languageclient` when that dependency is added later. LSP positions are standard 0-based line/character values with UTF-16 character offsets.

The URI/key mapping for LSP documents is:

```text
file:///main/<relpath> <-> <relpath>
```

Compile diagnostics reserve Monaco marker owner `"solcore-compile"`:

```ts
monaco.editor.setModelMarkers(model, "solcore-compile", markers);
```

A future LSP diagnostics owner should use a separate owner so compile diagnostics and LSP diagnostics can coexist or be migrated cleanly.
