# Future LSP integration

This directory is the seam for the future language server integration. It is intentionally a no-op today.

The LSP must be a separate Worker from `src/compiler/compile.worker.ts`. The compile worker uses a Playground-specific batch compile envelope:

```ts
{ id: number; kind: "compile"; input: CompileInput }
```

The future language worker should instead speak standard LSP JSON-RPC 2.0 over `postMessage`, with standard LSP positions: 0-based line/character and UTF-16 character offsets. The browser side should attach it to Monaco with `monaco-languageclient`; that dependency is deliberately not included yet.

Canonical workspace file keys are relative paths such as `main.solc` and `sub/Foo.solc`. These exact strings are used as Zustand keys, tab ids, and compile request `path` values. The LSP URI mapping should be:

```text
file:///main/<relpath> <-> <relpath>
```

Do not use `file://` URIs or leading slashes as workspace store keys.

Compile diagnostics use Monaco marker owner `"solcore-compile"`. The LSP diagnostics integration should use a distinct marker owner so both diagnostic streams can coexist during migration.
