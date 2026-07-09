import { uriForWorkspacePath } from "../monaco/paths";
import { useWorkspaceStore, type WorkspaceFile } from "../store/workspace";
import type { LspClient } from "./lspClient";

type WorkspaceFiles = Record<string, WorkspaceFile>;

function initialVersions(files: WorkspaceFiles): Map<string, number> {
  return new Map(Object.keys(files).map((path) => [uriForWorkspacePath(path), 1]));
}

export function startWorkspaceSync(client: LspClient): () => void {
  let previousFiles = useWorkspaceStore.getState().files;
  const versions = initialVersions(previousFiles);

  return useWorkspaceStore.subscribe((state) => {
    const nextFiles = state.files;

    for (const path of Object.keys(previousFiles)) {
      if (nextFiles[path]) {
        continue;
      }

      const uri = uriForWorkspacePath(path);
      client.didClose(uri);
      versions.delete(uri);
    }

    for (const [path, file] of Object.entries(nextFiles)) {
      const previousFile = previousFiles[path];
      const uri = uriForWorkspacePath(path);

      if (!previousFile) {
        versions.set(uri, 1);
        client.didOpen(uri, 1, file.content);
        continue;
      }

      if (previousFile.content !== file.content) {
        const version = (versions.get(uri) ?? 1) + 1;
        versions.set(uri, version);
        client.didChange(uri, version, file.content);
      }
    }

    previousFiles = nextFiles;
  });
}
