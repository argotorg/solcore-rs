import type * as Monaco from "monaco-editor";
import type { LspClient } from "./lspClient";
import { registerCodeAction } from "./providers/codeAction";
import { registerCompletion } from "./providers/completion";
import { registerDefinition } from "./providers/definition";
import { registerDocumentHighlight } from "./providers/documentHighlight";
import { registerDocumentSymbol } from "./providers/documentSymbol";
import { registerFolding } from "./providers/folding";
import { registerFormatting } from "./providers/formatting";
import { registerHover } from "./providers/hover";
import { registerInlayHints } from "./providers/inlayHints";
import { registerReferences } from "./providers/references";
import { registerRename } from "./providers/rename";
import { registerSelectionRange } from "./providers/selectionRange";
import { registerSemanticTokens } from "./providers/semanticTokens";
import { registerSignatureHelp } from "./providers/signatureHelp";

export function registerProviders(monaco: typeof Monaco, client: LspClient): () => void {
  const disposables: Monaco.IDisposable[] = [
    registerHover(monaco, client),
    registerCompletion(monaco, client),
    registerSignatureHelp(monaco, client),
    registerDefinition(monaco, client),
    registerReferences(monaco, client),
    registerDocumentHighlight(monaco, client),
    registerRename(monaco, client),
    registerDocumentSymbol(monaco, client),
    registerCodeAction(monaco, client),
    registerFormatting(monaco, client),
    registerFolding(monaco, client),
    registerSelectionRange(monaco, client),
    registerSemanticTokens(monaco, client),
    registerInlayHints(monaco, client),
  ];

  return () => {
    for (let index = disposables.length - 1; index >= 0; index -= 1) {
      disposables[index].dispose();
    }
  };
}
