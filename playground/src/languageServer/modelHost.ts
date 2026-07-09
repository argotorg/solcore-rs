import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../monaco/solc-language";
import { uriForWorkspacePath, workspacePathFromUri } from "../monaco/paths";
import { useWorkspaceStore, type WorkspaceFile } from "../store/workspace";

/**
 * Keeps a live Monaco text model for EVERY workspace file, not just the active
 * tab. The standalone editor only materializes a model for the file it shows,
 * but cross-file LSP features need every document to exist as a model:
 * - go-to-definition / find-references can open a target in another file,
 * - a rename WorkspaceEdit applies text edits across several files,
 * - diagnostics render on files that are not currently focused.
 *
 * Models are the workspace store's mirror: edits to any model (including bulk
 * rename edits to background files) flow back to the store via setContent, and
 * external store changes (load example, reset, cross-file rename) are pushed
 * into the corresponding models. `@monaco-editor/react` reuses these models by
 * URI, so it must be configured with `keepCurrentModel` to avoid disposing them
 * on tab switches — this module owns their lifecycle.
 */
export function startModelHost(monaco: typeof Monaco): () => void {
  const contentSubscriptions = new Map<string, Monaco.IDisposable>();
  let modelOriginatedStoreUpdateDepth = 0;

  const setStoreContentFromModel = (path: string, content: string): void => {
    modelOriginatedStoreUpdateDepth += 1;
    try {
      useWorkspaceStore.getState().setContent(path, content);
    } finally {
      modelOriginatedStoreUpdateDepth -= 1;
    }
  };

  const ensureContentSubscription = (
    path: string,
    model: Monaco.editor.ITextModel,
  ): void => {
    const uriString = model.uri.toString();
    if (contentSubscriptions.has(uriString)) {
      return;
    }

    const subscription = model.onDidChangeContent(() => {
      setStoreContentFromModel(path, model.getValue());
    });
    contentSubscriptions.set(uriString, subscription);
  };

  const ensureModel = (path: string, content: string): void => {
    const uri = monaco.Uri.parse(uriForWorkspacePath(path));
    const existing = monaco.editor.getModel(uri);

    if (!existing) {
      const model = monaco.editor.createModel(content, SOLCORE_LANGUAGE_ID, uri);
      ensureContentSubscription(path, model);
      return;
    }

    if (existing.getValue() !== content && modelOriginatedStoreUpdateDepth === 0) {
      existing.setValue(content);
    }
    ensureContentSubscription(path, existing);
  };

  const disposeModel = (uriString: string): void => {
    contentSubscriptions.get(uriString)?.dispose();
    contentSubscriptions.delete(uriString);
    monaco.editor.getModel(monaco.Uri.parse(uriString))?.dispose();
  };

  const sync = (files: Record<string, WorkspaceFile>): void => {
    const wanted = new Set<string>();
    for (const [path, file] of Object.entries(files)) {
      wanted.add(monaco.Uri.parse(uriForWorkspacePath(path)).toString());
      ensureModel(path, file.content);
    }

    for (const model of monaco.editor.getModels()) {
      const uriString = model.uri.toString();
      const path = workspacePathFromUri(uriString);
      if (path !== null && !wanted.has(uriString)) {
        disposeModel(uriString);
      }
    }
  };

  sync(useWorkspaceStore.getState().files);
  const unsubscribe = useWorkspaceStore.subscribe((state) => sync(state.files));

  return () => {
    unsubscribe();
    for (const subscription of contentSubscriptions.values()) {
      subscription.dispose();
    }
    contentSubscriptions.clear();
  };
}
