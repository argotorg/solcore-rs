import { useEffect, useState } from "react";
import { version } from "../compiler/runtime";
import { useWorkspaceStore } from "../store/workspace";
import type { CursorPosition } from "./EditorPane";

interface StatusBarProps {
  cursor: CursorPosition;
}

export function StatusBar({ cursor }: StatusBarProps): JSX.Element {
  const entry = useWorkspaceStore((state) => state.entry);
  const compiling = useWorkspaceStore((state) => state.compiling);
  const result = useWorkspaceStore((state) => state.result);
  const [compilerVersion, setCompilerVersion] = useState<string | null>(null);
  const diagnostics = result?.diagnostics ?? [];
  const errorCount = diagnostics.filter((diagnostic) => diagnostic.severity === "error").length;
  const warningCount = diagnostics.filter((diagnostic) => diagnostic.severity === "warning").length;
  const status = compiling
    ? "Compiling..."
    : errorCount > 0
      ? `${errorCount} error${errorCount === 1 ? "" : "s"}`
      : warningCount > 0
        ? `${warningCount} warning${warningCount === 1 ? "" : "s"}`
        : result
          ? "Compiled ✓"
          : "Ready";

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
      <span className={errorCount > 0 ? "statusbar__error" : ""}>{status}</span>
      <span>solcore {compilerVersion ?? "..."}</span>
    </footer>
  );
}
