import { useEffect, useState } from "react";
import { Panel, PanelGroup } from "react-resizable-panels";
import { EditorPane, type CursorPosition } from "./components/EditorPane";
import { FileExplorer } from "./components/FileExplorer";
import { OutputPane } from "./components/OutputPane";
import { ResizeHandle } from "./components/ResizeHandle";
import { StatusBar } from "./components/StatusBar";
import { TopBar } from "./components/TopBar";
import { useWorkspaceStore } from "./store/workspace";

function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() =>
    typeof window === "undefined" ? false : window.matchMedia(query).matches,
  );

  useEffect(() => {
    const mediaQuery = window.matchMedia(query);
    const updateMatches = (): void => setMatches(mediaQuery.matches);
    updateMatches();
    mediaQuery.addEventListener("change", updateMatches);

    return () => mediaQuery.removeEventListener("change", updateMatches);
  }, [query]);

  return matches;
}

export function App(): JSX.Element {
  const compileNow = useWorkspaceStore((state) => state.compileNow);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const isNarrow = useMediaQuery("(max-width: 900px)");
  const [cursor, setCursor] = useState<CursorPosition>({ line: 1, column: 1 });
  const mainDirection = isNarrow ? "vertical" : "horizontal";

  useEffect(() => {
    void compileNow();
  }, [compileNow]);

  useEffect(() => {
    if (isNarrow) {
      setSidebarOpen(false);
    }
  }, [isNarrow]);

  return (
    <div className="app-shell">
      <TopBar
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((isOpen) => !isOpen)}
      />

      <main className="workspace-shell">
        <PanelGroup direction="horizontal" className="workspace-panels">
          {sidebarOpen ? (
            <>
              <Panel defaultSize={18} minSize={14} maxSize={28} collapsible>
                <FileExplorer />
              </Panel>
              <ResizeHandle />
            </>
          ) : null}

          <Panel minSize={45}>
            <PanelGroup direction={mainDirection} className="main-panels">
              <Panel defaultSize={62} minSize={32}>
                <EditorPane onCursorChange={setCursor} />
              </Panel>
              <ResizeHandle direction={mainDirection} />
              <Panel defaultSize={38} minSize={24}>
                <OutputPane />
              </Panel>
            </PanelGroup>
          </Panel>
        </PanelGroup>
      </main>

      <StatusBar cursor={cursor} />
    </div>
  );
}
