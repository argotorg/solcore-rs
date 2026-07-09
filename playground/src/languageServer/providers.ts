import type * as Monaco from "monaco-editor";
import type { LspClient } from "./lspClient";

export function registerProviders(monaco: typeof Monaco, client: LspClient): () => void {
  void monaco;
  void client;

  // NOTE(codex): Feature-specific LSP provider registrations are intentionally
  // deferred; this foundation commit only wires lifecycle, sync, and diagnostics.
  const disposers: Array<() => void> = [];

  return () => {
    for (let index = disposers.length - 1; index >= 0; index -= 1) {
      disposers[index]();
    }
  };
}
