# Language Client

This directory is a small compatibility layer for the active browser LSP integration. It re-exports `attachLanguageClient()` from `src/languageServer/attachLanguageClient.ts` so editor code can depend on a stable language-client entry point.

The LSP runs in `src/languageServer/lsp.worker.ts`, separate from `src/compiler/compile.worker.ts`. The compile worker uses a Playground-specific batch compile envelope:

```ts
{ id: number; kind: "compile"; input: CompileInput }
```

The language worker speaks standard LSP JSON-RPC 2.0 over `postMessage`, with standard LSP positions: 0-based line/character and UTF-16 character offsets.

Canonical workspace file keys are relative paths such as `main.solc` and `sub/Foo.solc`. These exact strings are used as Zustand keys, tab ids, and compile request `path` values. The LSP URI mapping is:

```text
file:///main/<relpath> <-> <relpath>
```

Do not use `file://` URIs or leading slashes as workspace store keys.

Compile diagnostics use Monaco marker owner `"solcore-compile"`. LSP diagnostics use `"solcore-lsp"` so both diagnostic streams can coexist.
