import type * as Monaco from "monaco-editor";

export type DetachLanguageClient = () => void;

/**
 * Future LSP seam.
 *
 * The compiler worker in src/compiler is a Playground-specific batch compile
 * worker. A future LSP integration should use a separate Worker that speaks
 * standard LSP JSON-RPC 2.0 over postMessage and is consumed through
 * monaco-languageclient.
 *
 * Canonical file keys in the Zustand workspace remain relative paths such as
 * "main.solc" or "sub/Foo.solc". The LSP layer should map Monaco/LSP document
 * URIs of the form "file:///main/<relpath>" to that exact key by stripping the
 * prefix and decoding path segments. Store keys, tab ids, and compile request
 * paths must never become file:// URIs or leading-slash paths.
 *
 * Compile diagnostics reserve Monaco marker owner "solcore-compile". A future
 * LSP diagnostics owner should use its own owner so the two streams can coexist
 * during migration.
 */
export function attachLanguageClient(monaco: typeof Monaco): DetachLanguageClient {
  void monaco;
  return () => undefined;
}
