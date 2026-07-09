import { useEffect, useState } from "react";
import { version } from "../compiler/runtime";
import { formatCompileDuration } from "../compiler/timing";
import { useWorkspaceStore } from "../store/workspace";
import type { CursorPosition } from "./EditorPane";
import { useCompileElapsed } from "./useCompileElapsed";

interface StatusBarProps {
  cursor: CursorPosition;
}

export function StatusBar({ cursor }: StatusBarProps): JSX.Element {
  const entry = useWorkspaceStore((state) => state.entry);
  const compiling = useWorkspaceStore((state) => state.compiling);
  const rawResult = useWorkspaceStore((state) => state.result);
  const lastCompileDurationMs = useWorkspaceStore((state) => state.lastCompileDurationMs);
  const workspaceVersion = useWorkspaceStore((state) => state.workspaceVersion);
  const lastCompiledVersion = useWorkspaceStore((state) => state.lastCompiledVersion);
  const [compilerVersion, setCompilerVersion] = useState<string | null>(null);
  const compileElapsedMs = useCompileElapsed();
  const compileIsOutdated =
    lastCompiledVersion !== null && lastCompiledVersion !== workspaceVersion;
  const result = compileIsOutdated ? null : rawResult;
  const diagnostics = result?.diagnostics ?? [];
  const errorCount = diagnostics.filter((diagnostic) => diagnostic.severity === "error").length;
  const warningCount = diagnostics.filter((diagnostic) => diagnostic.severity === "warning").length;
  const status = compiling
    ? "Compiling..."
    : compileIsOutdated
      ? "Needs compile"
      : errorCount > 0
        ? `${errorCount} error${errorCount === 1 ? "" : "s"}`
        : warningCount > 0
          ? `${warningCount} warning${warningCount === 1 ? "" : "s"}`
          : result
            ? "Compiled ✓"
            : "Ready";
  const statusClass =
    errorCount > 0 ? "statusbar__error" : !compiling && compileIsOutdated ? "statusbar__warning" : "";
  const durationText =
    compileElapsedMs !== null
      ? `Elapsed ${formatCompileDuration(compileElapsedMs)}`
      : lastCompileDurationMs !== null
        ? `Last compile ${formatCompileDuration(lastCompileDurationMs)}`
        : null;

  useEffect(() => {
    let isMounted = true;

    void version()
      .then((nextVersion) => {
        if (isMounted) {
          setCompilerVersion(nextVersion);
        }
      })
      .catch(() => {
        if (isMounted) {
          setCompilerVersion("unavailable");
        }
      });

    return () => {
      isMounted = false;
    };
  }, []);

  return (
    <footer className="statusbar" aria-label="Workspace status">
      <span>Entry {entry}</span>
      <span>
        Ln {cursor.line}, Col {cursor.column}
      </span>
      <span className={statusClass}>{status}</span>
      {durationText ? <span>{durationText}</span> : null}
      <span>solcore {compilerVersion ?? "..."}</span>
    </footer>
  );
}
