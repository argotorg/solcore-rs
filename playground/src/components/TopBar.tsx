import {
  Braces,
  Check,
  ChevronDown,
  Link as LinkIcon,
  X,
  Loader2,
  Moon,
  PanelBottomClose,
  PanelBottomOpen,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Play,
  RotateCcw,
  Sun,
} from "lucide-react";
import { useEffect, useState } from "react";
import { version } from "../compiler/runtime";
import { formatCompileDuration } from "../compiler/timing";
import { buildExampleLink } from "../share/exampleLink";
import { examples } from "../store/workspace";
import { useWorkspaceStore } from "../store/workspace";
import { useCompileElapsed } from "./useCompileElapsed";

interface TopBarProps {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  outputOpen: boolean;
  onToggleOutput: () => void;
  outputStacked: boolean;
}

export function TopBar({
  sidebarOpen,
  onToggleSidebar,
  outputOpen,
  onToggleOutput,
  outputStacked,
}: TopBarProps): JSX.Element {
  const order = useWorkspaceStore((state) => state.order);
  const entry = useWorkspaceStore((state) => state.entry);
  const compiling = useWorkspaceStore((state) => state.compiling);
  const lastCompileDurationMs = useWorkspaceStore((state) => state.lastCompileDurationMs);
  const workspaceVersion = useWorkspaceStore((state) => state.workspaceVersion);
  const lastCompiledVersion = useWorkspaceStore((state) => state.lastCompiledVersion);
  const theme = useWorkspaceStore((state) => state.theme);
  const setEntry = useWorkspaceStore((state) => state.setEntry);
  const compileNow = useWorkspaceStore((state) => state.compileNow);
  const toggleTheme = useWorkspaceStore((state) => state.toggleTheme);
  const resetWorkspace = useWorkspaceStore((state) => state.resetWorkspace);
  const loadExample = useWorkspaceStore((state) => state.loadExample);
  const selectedExample = useWorkspaceStore((state) => state.exampleId);
  const [compilerVersion, setCompilerVersion] = useState<string | null>(null);
  const [linkCopyState, setLinkCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const solFiles = order.filter((path) => path.endsWith(".sol"));
  const compileElapsedMs = useCompileElapsed();
  const compileIsOutdated =
    lastCompiledVersion !== null && lastCompiledVersion !== workspaceVersion;
  const compileTimeLabel =
    compileElapsedMs !== null
      ? formatCompileDuration(compileElapsedMs)
      : lastCompileDurationMs !== null
        ? formatCompileDuration(lastCompileDurationMs)
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

  useEffect(() => {
    if (linkCopyState === "idle") {
      return;
    }

    const timer = window.setTimeout(() => setLinkCopyState("idle"), 2000);
    return () => window.clearTimeout(timer);
  }, [linkCopyState]);

  const copyExampleLink = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(buildExampleLink(window.location.href, selectedExample));
      setLinkCopyState("copied");
    } catch {
      // Clipboard access needs a secure context and may be denied.
      setLinkCopyState("failed");
    }
  };

  return (
    <header className="topbar">
      <div className="topbar__left">
        <button
          type="button"
          className="icon-button topbar__sidebar-toggle"
          onClick={onToggleSidebar}
          aria-expanded={sidebarOpen}
          aria-controls="workspace-files"
          title={sidebarOpen ? "Hide file explorer" : "Show file explorer"}
          aria-label={sidebarOpen ? "Hide file explorer" : "Show file explorer"}
        >
          {sidebarOpen ? <PanelLeftClose size={18} /> : <PanelLeftOpen size={18} />}
        </button>

        <div className="brand">
          <span className="brand__mark" aria-hidden="true">
            <Braces size={20} />
          </span>
          <h1 className="brand__text">solcore playground</h1>
        </div>
      </div>

      <div className="topbar__controls">
        <label className="select-control">
          <span>Example</span>
          <span className="select-control__shell">
            <select aria-label="Example" value={selectedExample} onChange={(event) => loadExample(event.target.value)}>
              {examples.map((example) => (
                <option key={example.id} value={example.id}>
                  {example.name}
                </option>
              ))}
            </select>
            <ChevronDown size={14} aria-hidden="true" />
          </span>
        </label>

        <button
          type="button"
          className="icon-button"
          onClick={() => {
            void copyExampleLink();
          }}
          title={
            linkCopyState === "failed"
              ? "Copy failed - copy the address bar URL with ?example=" + selectedExample
              : "Copy a link to this example"
          }
          aria-label="Copy a link to this example"
        >
          {linkCopyState === "copied" ? (
            <Check size={18} />
          ) : linkCopyState === "failed" ? (
            <X size={18} />
          ) : (
            <LinkIcon size={18} />
          )}
        </button>

        <label className="select-control">
          <span>Entry</span>
          <span className="select-control__shell">
            <select aria-label="Entry" value={entry} onChange={(event) => setEntry(event.target.value)}>
              {solFiles.map((path) => (
                <option key={path} value={path}>
                  {path}
                </option>
              ))}
            </select>
            <ChevronDown size={14} aria-hidden="true" />
          </span>
        </label>

        <button
          type="button"
          className="button button--primary"
          aria-label={compiling ? "Compiling" : "Compile"}
          disabled={compiling}
          onClick={() => {
            void compileNow();
          }}
        >
          {compiling ? <Loader2 className="spin" size={16} /> : <Play size={16} />}
          <span>{compiling ? "Compiling" : "Compile"}</span>
        </button>

        {compileTimeLabel ? (
          <span
            className={`compile-timing ${compileIsOutdated && !compiling ? "is-outdated" : ""}`}
            title={
              compileIsOutdated && !compiling
                ? "Workspace changed since the last compile"
                : "Last compile duration"
            }
          >
            {compiling ? compileTimeLabel : `Last ${compileTimeLabel}`}
          </span>
        ) : null}

        <button
          type="button"
          className="icon-button"
          onClick={toggleTheme}
          title={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
        >
          {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
        </button>

        <button
          type="button"
          className="button button--secondary"
          onClick={resetWorkspace}
          aria-label="Reset workspace"
          title="Reset workspace"
        >
          <RotateCcw size={16} />
          <span>Reset</span>
        </button>

        <span className="version-pill">v{compilerVersion ?? "..."}</span>

        <button
          type="button"
          className="icon-button"
          onClick={onToggleOutput}
          aria-expanded={outputOpen}
          aria-controls="compiler-output"
          title={outputOpen ? "Hide output pane" : "Show output pane"}
          aria-label={outputOpen ? "Hide output pane" : "Show output pane"}
        >
          {outputStacked ? (
            outputOpen ? (
              <PanelBottomClose size={18} />
            ) : (
              <PanelBottomOpen size={18} />
            )
          ) : outputOpen ? (
            <PanelRightClose size={18} />
          ) : (
            <PanelRightOpen size={18} />
          )}
        </button>
      </div>
    </header>
  );
}
