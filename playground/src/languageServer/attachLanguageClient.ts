import type * as Monaco from "monaco-editor";
import { uriForWorkspacePath } from "../monaco/paths";
import { useWorkspaceStore } from "../store/workspace";
import { LspClient } from "./lspClient";
import { registerProviders } from "./providers";
import { startWorkspaceSync } from "./workspaceSync";

export type DetachLanguageClient = () => void;

let currentDetach: DetachLanguageClient | null = null;

function openCurrentWorkspace(client: LspClient): void {
  const { files, order } = useWorkspaceStore.getState();
  const opened = new Set<string>();

  for (const path of order) {
    const file = files[path];
    if (!file) {
      continue;
    }

    opened.add(path);
    client.didOpen(uriForWorkspacePath(path), 1, file.content);
  }

  for (const [path, file] of Object.entries(files)) {
    if (opened.has(path)) {
      continue;
    }

    client.didOpen(uriForWorkspacePath(path), 1, file.content);
  }
}

export function attachLanguageClient(monaco: typeof Monaco): DetachLanguageClient {
  if (currentDetach) {
    return currentDetach;
  }

  const client = new LspClient();
  const disposers: DetachLanguageClient[] = [];
  let disposed = false;

  const detach = (): void => {
    if (disposed) {
      return;
    }

    disposed = true;
    for (let index = disposers.length - 1; index >= 0; index -= 1) {
      disposers[index]();
    }
    disposers.length = 0;
    client.dispose();

    if (currentDetach === detach) {
      currentDetach = null;
    }
  };

  currentDetach = detach;

  void client
    .initialize()
    .then(() => {
      if (disposed) {
        return;
      }

      openCurrentWorkspace(client);
      disposers.push(startWorkspaceSync(client));
      disposers.push(registerProviders(monaco, client));
    })
    .catch((error: unknown) => {
      console.error("Failed to initialize Solcore language server", error);
      detach();
    });

  return detach;
}
