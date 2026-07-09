import type * as Monaco from "monaco-editor";
import type { LspClient } from "./lspClient";
import { registerCompletion } from "./providers/completion";
import { registerHover } from "./providers/hover";
import { registerSignatureHelp } from "./providers/signatureHelp";

export function registerProviders(monaco: typeof Monaco, client: LspClient): () => void {
  const disposables: Monaco.IDisposable[] = [
    registerHover(monaco, client),
    registerCompletion(monaco, client),
    registerSignatureHelp(monaco, client),
  ];

  return () => {
    for (let index = disposables.length - 1; index >= 0; index -= 1) {
      disposables[index].dispose();
    }
  };
}
