import {
  Braces,
  ChevronDown,
  Loader2,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  Play,
  RotateCcw,
  Sun,
} from "lucide-react";
import { useEffect, useState } from "react";
import { version } from "../compiler/runtime";
import { examples } from "../store/workspace";
import { useWorkspaceStore } from "../store/workspace";

interface TopBarProps {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
}

export function TopBar({ sidebarOpen, onToggleSidebar }: TopBarProps): JSX.Element {
  const order = useWorkspaceStore((state) => state.order);
  const entry = useWorkspaceStore((state) => state.entry);
  const compiling = useWorkspaceStore((state) => state.compiling);
  const theme = useWorkspaceStore((state) => state.theme);
  const setEntry = useWorkspaceStore((state) => state.setEntry);
  const compileNow = useWorkspaceStore((state) => state.compileNow);
  const toggleTheme = useWorkspaceStore((state) => state.toggleTheme);
  const resetWorkspace = useWorkspaceStore((state) => state.resetWorkspace);
  const loadExample = useWorkspaceStore((state) => state.loadExample);
  const [selectedExample, setSelectedExample] = useState(examples[0]?.id ?? "hello");
  const [compilerVersion, setCompilerVersion] = useState<string | null>(null);
  const solcFiles = order.filter((path) => path.endsWith(".solc"));

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
    <header className="topbar">
      <div className="topbar__left">
        <button
          type="button"
          className="icon-button topbar__sidebar-toggle"
          onClick={onToggleSidebar}
          title={sidebarOpen ? "Hide file explorer" : "Show file explorer"}
          aria-label={sidebarOpen ? "Hide file explorer" : "Show file explorer"}
        >
          {sidebarOpen ? <PanelLeftClose size={18} /> : <PanelLeftOpen size={18} />}
        </button>

        <div className="brand" aria-label="solcore playground">
          <span className="brand__mark" aria-hidden="true">
            <Braces size={20} />
          </span>
          <span className="brand__text">solcore playground</span>
        </div>
      </div>

      <div className="topbar__controls">
        <label className="select-control">
          <span>Example</span>
          <span className="select-control__shell">
            <select
              value={selectedExample}
              onChange={(event) => {
                const nextExample = event.target.value;
                setSelectedExample(nextExample);
                loadExample(nextExample);
              }}
            >
              {examples.map((example) => (
                <option key={example.id} value={example.id}>
                  {example.name}
                </option>
              ))}
            </select>
            <ChevronDown size={14} aria-hidden="true" />
          </span>
        </label>

        <label className="select-control">
          <span>Entry</span>
          <span className="select-control__shell">
            <select value={entry} onChange={(event) => setEntry(event.target.value)}>
              {solcFiles.map((path) => (
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
          onClick={() => {
            void compileNow();
          }}
        >
          {compiling ? <Loader2 className="spin" size={16} /> : <Play size={16} />}
          <span>Compile</span>
        </button>

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
          title="Reset workspace"
        >
          <RotateCcw size={16} />
          <span>Reset</span>
        </button>

        <span className="version-pill">v{compilerVersion ?? "..."}</span>
      </div>
    </header>
  );
}
