import type * as Monaco from "monaco-editor";
import type { LspClient } from "./lspClient";
import { registerHover } from "./providers/hover";

export function registerProviders(monaco: typeof Monaco, client: LspClient): () => void {
  const disposables: Monaco.IDisposable[] = [
    registerHover(monaco, client),
  ];

  return () => {
    for (let index = disposables.length - 1; index >= 0; index -= 1) {
      disposables[index].dispose();
    }
  };
}
