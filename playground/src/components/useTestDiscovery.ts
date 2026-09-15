import { useEffect } from "react";
import { compileClient } from "../compiler/compileClient";
import { useWorkspaceStore } from "../store/workspace";

export function useTestDiscovery(): void {
  const files = useWorkspaceStore((s) => s.files);
  const entry = useWorkspaceStore((s) => s.entry);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const compiling = useWorkspaceStore((s) => s.compiling);
  useEffect(() => {
    if (compiling) return;
    if (!files[entry]?.content.includes("#[")) {
      useWorkspaceStore.setState({ testCases: [] });
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      void compileClient.discover({
        files: Object.values(files), entry,
        options: { emitHull: false, emitYul: false, emitSonatina: false, emitAbi: false },
      }).then((result) => {
        if (active && useWorkspaceStore.getState().workspaceVersion === version) {
          useWorkspaceStore.setState({ testCases: result.tests });
        }
      }).catch(() => {
        // Compilation and worker errors are reported by the explicit Run action.
      });
    }, 500);
    return () => { active = false; window.clearTimeout(timer); };
  }, [files, entry, version, compiling]);
}
