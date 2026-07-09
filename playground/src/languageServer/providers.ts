import type * as Monaco from "monaco-editor";
import type { LspClient } from "./lspClient";
import { registerCompletion } from "./providers/completion";
import { registerDefinition } from "./providers/definition";
import { registerHover } from "./providers/hover";
import { registerReferences } from "./providers/references";
import { registerSignatureHelp } from "./providers/signatureHelp";

export function registerProviders(monaco: typeof Monaco, client: LspClient): () => void {
  const disposables: Monaco.IDisposable[] = [
    registerHover(monaco, client),
    registerCompletion(monaco, client),
    registerSignatureHelp(monaco, client),
    registerDefinition(monaco, client),
    registerReferences(monaco, client),
  ];

  return () => {
    for (let index = disposables.length - 1; index >= 0; index -= 1) {
      disposables[index].dispose();
    }
  };
}
