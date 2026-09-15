# solcore playground

A React + TypeScript + Vite frontend for the solcore-rs compiler Playground. The compile path runs in a Web Worker and calls the generated `solcore-wasm` package from `../crates/wasm/pkg`. Editor language features run in a separate LSP Worker backed by `solcore-lsp` from `../crates/lsp/pkg`.

## Run a program

Select **Hello contract** and click **Run** to execute it in the browser. The
**Execution** tab shows the return word, raw return data, gas used, and any revert
or halt. revm runs inside the existing compiler WASM worker.

Run supports a no-argument `main` returning one word, either as a plain function
or the public runtime entry of a single contract with a no-argument constructor.
Each run uses a new in-memory database. Constructor storage is available to `main`
within that run. Execution output stays visible during edits and clears when the
next run starts.

The runner uses Osaka rules, a gas limit of 1,000,000 per transaction, and a 16 MiB
EVM memory limit. Contract `main` receives empty calldata. A wrapper returns its
word to the playground; the normal Compile artifacts are unchanged.

## Run tests

Public contract methods can carry the same test comments used by the compiler's
E2E fixtures:

```sol
// #[(20, 22) -> 42]
function add(a: uint256, b: uint256) public returns (uint256) {
    return a + b;
}
```

Click **Run test** above a comment or **Run all tests** in the toolbar. Tests are
discovered in the entry file. Results appear beside their comments and in the
Execution tab. Edits retain inline results, marked outdated until the next run.
With no test comments, Run executes `main` as described above.

Tests use the contract's normal selector dispatcher, so the contract must not
have an explicit runtime `main`. Constructors currently take no arguments.
The shared directive parser supports static ABI values and expected reverts.
`// #[send(7)]` commits a state-changing call; ordinary assertions leave state
unchanged. Tests run in source order against one deployment per contract.
An individual test replays preceding sends for its contract first.

## Keyboard navigation

Use Left/Right, Home, and End to move between source or output tabs. In the
editor, Ctrl+M (Ctrl+Shift+M on macOS) toggles whether Tab indents or moves focus.

## Development

Use wasm-pack 0.15.0, as pinned in CI, for the custom WASM build profile.

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

## Styles

`src/styles/tokens.css` defines shared colors, spacing, and typography. `base.css` sets
inherited defaults; `app.css` contains component rules and responsive overrides.
Use the typography tokens for UI text and keep responsive selectors scoped to the
component they change. Monaco's code font sizes are configured in the editor components.

## Sharing examples

Every bundled example has a stable id, and the Playground reads it from an `example` query
parameter, so a link like this opens that example directly:

```
https://<host>/?example=trait
```

The link button next to the example picker copies the link for the currently selected example.
Ids live in `src/examples/index.ts`; an unknown or missing id is ignored and the Playground opens
the workspace it would otherwise restore.

A shared link loads the example as it ships with that deployment — it does not carry edited code.
Opening one replaces the locally stored workspace, and the parameter is then dropped from the
address bar so a later reload keeps whatever the visitor edited. The previous workspace payload
is kept under the `solcore-playground.workspace.v1.backup` localStorage key, so accidentally
following a link does not destroy saved work beyond recovery; to restore it, run this in the
browser console:

```js
localStorage.setItem(
  "solcore-playground.workspace.v1",
  localStorage.getItem("solcore-playground.workspace.v1.backup"),
);
location.reload();
```

An unknown id keeps the parameter in the address bar so a mistyped link stays diagnosable.

## Build

```sh
npm run build
```

The build script rebuilds both wasm packages, runs `tsc --noEmit`, then runs `vite build`.

After building, `npm run test:wasm` checks execution through the generated WASM
package. `npm run test:unit` checks the JavaScript helpers and workspace updates.

## Deploy

Deploy the generated `dist/` directory with static hosting that serves `.wasm` files. Vite emits the compiler and LSP wasm files as assets and rewrites the worker imports to those built assets.

For static hosting under a subpath, set `VITE_BASE`:

```sh
VITE_BASE=/solcore-rs/ npm run build
```

## WASM packages

Rebuild the sibling wasm crates whenever the compiler or LSP changes. Preferred (size-optimized):

```sh
npm run build:wasm     # from playground/: compiler wasm + LSP wasm + pinned `wasm-opt -Oz`
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
is instead tuned for native compiler throughput. A final `wasm-opt -Oz` pass from the exact
`binaryen` version in `package-lock.json` reduces the generated package further. `npm run
build:wasm` applies it automatically after `npm ci`; a missing optimizer is a build error rather
than silently changing the bundle contents. `vite build` reports the current raw and gzipped asset
sizes. The Playground imports `init`,
`compile`, `run`, `std_files`, and `version` from `solcore-wasm`; `src/compiler/runtime.ts` passes Vite's emitted
`solcore_wasm_bg.wasm?url` asset to `init()` and caches initialization. The LSP worker imports
`SolcoreLsp` from `solcore-lsp`; `src/languageServer/lsp.worker.ts` passes Vite's emitted
`solcore_lsp_bg.wasm?url` asset to `init()`. The shared compiler API shape lives in
`src/compiler/types.ts` and should stay the single source of truth for the Playground compile protocol.

The compiler worker accepts compile and run messages:

```ts
// request
{ id: number; kind: "compile" | "run"; input: CompileInput }

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
  execution: ExecutionResult | null;
}
```

`success` describes compilation; `execution.status` describes execution.
`execution` is null for Compile requests and compilation errors.

The Playground requests Hull, Yul, Sonatina IR, and contract ABI JSON in one compile and exposes each
textual output in its own tab. Backend fields remain `null` when an output was not requested,
compilation stopped before that backend ran, or (for ABI) the workspace contains no contract.

## File key contract

The canonical file key is always a workspace-relative path string, for example `main.sol` or `sub/Foo.sol`.

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
