import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { Panel, PanelGroup, type ImperativePanelHandle } from "react-resizable-panels";
import { EditorPane, type CursorPosition } from "./components/EditorPane";
import { FileExplorer } from "./components/FileExplorer";
import { OutputPane } from "./components/OutputPane";
import { ResizeHandle } from "./components/ResizeHandle";
import { StatusBar } from "./components/StatusBar";
import { TopBar } from "./components/TopBar";

// Below this main-area width, the editor and output panes can no longer sit
// side by side without either squeezing the editor unreasonably narrow or
// letting the output tab strip (Hull/Yul/Sonatina IR/ABI/Problems) overflow.
const EDITOR_MIN_PX = 320;
const OUTPUT_TAB_SAFE_PX = 420;
const MAIN_DIRECTION_THRESHOLD_PX = EDITOR_MIN_PX + OUTPUT_TAB_SAFE_PX;
// Small deadband around the threshold so a width sitting right on the edge
// doesn't flip-flop between layouts every pixel.
const MAIN_DIRECTION_HYSTERESIS_PX = 24;

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

// Tracks the pixel width of the element the ref is attached to. Used to
// decide the editor/output layout from the space actually available to it
// (sidebar width + window width combined), not just the window's own size.
function useElementWidth(): [RefObject<HTMLDivElement>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const element = ref.current;
    if (!element) {
      return;
    }

    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry) {
        setWidth(entry.contentRect.width);
      }
    });

    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return [ref, width];
}

export function App(): JSX.Element {
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [outputOpen, setOutputOpen] = useState(true);
  // The sidebar's own auto-collapse still just follows the window - it isn't
  // part of the tab-overflow problem the main-area measurement below solves.
  const isNarrow = useMediaQuery("(max-width: 900px)");
  const [cursor, setCursor] = useState<CursorPosition>({ line: 1, column: 1 });
  const sidebarRef = useRef<ImperativePanelHandle>(null);
  const outputRef = useRef<ImperativePanelHandle>(null);
  const [mainAreaRef, mainAreaWidth] = useElementWidth();

  // Space-aware editor/output direction, with hysteresis so it doesn't
  // flip-flop when the width sits right on the threshold. Driven by the
  // pixel width actually available to the editor+output pair (which shrinks
  // as the sidebar widens or the window narrows), not the window alone -
  // this is what guarantees the output tab strip can never overflow: once
  // there isn't room for editor-min + tab-safe side by side, the layout
  // stacks instead of squeezing.
  const [mainDirection, setMainDirection] = useState<"horizontal" | "vertical">("horizontal");

  useEffect(() => {
    if (mainAreaWidth <= 0) {
      return;
    }

    const toVerticalBelowPx = MAIN_DIRECTION_THRESHOLD_PX - MAIN_DIRECTION_HYSTERESIS_PX / 2;
    const toHorizontalAbovePx = MAIN_DIRECTION_THRESHOLD_PX + MAIN_DIRECTION_HYSTERESIS_PX / 2;

    setMainDirection((current) => {
      if (current === "horizontal" && mainAreaWidth < toVerticalBelowPx) {
        return "vertical";
      }
      if (current === "vertical" && mainAreaWidth > toHorizontalAbovePx) {
        return "horizontal";
      }
      return current;
    });
  }, [mainAreaWidth]);

  // In horizontal mode, cap how far the output pane can be dragged narrow so
  // its tab strip (Hull/Yul/Sonatina IR/ABI/Problems) never overflows. Full
  // width in vertical mode always fits the tabs, so a modest height-percent
  // minimum is enough there.
  const outputMinSize =
    mainDirection === "vertical"
      ? 24
      : mainAreaWidth > 0
        ? Math.min(60, (OUTPUT_TAB_SAFE_PX / mainAreaWidth) * 100)
        : 24;

  // Collapse (not unmount) on narrow viewports so react-resizable-panels
  // keeps tracking the panel's identity/layout across the transition.
  useEffect(() => {
    if (isNarrow) {
      sidebarRef.current?.collapse();
    }
  }, [isNarrow]);

  const toggleSidebar = useCallback(() => {
    const panel = sidebarRef.current;
    if (!panel) {
      return;
    }
    if (panel.isCollapsed()) {
      panel.expand();
    } else {
      panel.collapse();
    }
  }, []);

  const toggleOutput = useCallback(() => {
    const panel = outputRef.current;
    if (!panel) {
      return;
    }
    if (panel.isCollapsed()) {
      panel.expand();
    } else {
      panel.collapse();
    }
  }, []);

  return (
    <div className="app-shell">
      <TopBar
        sidebarOpen={sidebarOpen}
        onToggleSidebar={toggleSidebar}
        outputOpen={outputOpen}
        onToggleOutput={toggleOutput}
        outputStacked={mainDirection === "vertical"}
      />

      <main className="workspace-shell">
        <PanelGroup direction="horizontal" className="workspace-panels">
          <Panel
            ref={sidebarRef}
            id="sidebar"
            order={1}
            defaultSize={18}
            minSize={14}
            maxSize={28}
            collapsible
            collapsedSize={0}
            onCollapse={() => setSidebarOpen(false)}
            onExpand={() => setSidebarOpen(true)}
          >
            <FileExplorer hidden={!sidebarOpen} />
          </Panel>
          <ResizeHandle />

          <Panel id="main" order={2} minSize={45}>
            <div ref={mainAreaRef} className="main-area">
              <PanelGroup direction={mainDirection} className="main-panels">
                <Panel id="editor" order={1} defaultSize={100 - Math.max(38, outputMinSize)} minSize={32}>
                  <EditorPane onCursorChange={setCursor} />
                </Panel>
                <ResizeHandle direction={mainDirection} />
                <Panel
                  ref={outputRef}
                  id="output"
                  order={2}
                  defaultSize={Math.max(38, outputMinSize)}
                  minSize={outputMinSize}
                  collapsible
                  collapsedSize={0}
                  onCollapse={() => setOutputOpen(false)}
                  onExpand={() => setOutputOpen(true)}
                >
                  <OutputPane hidden={!outputOpen} />
                </Panel>
              </PanelGroup>
            </div>
          </Panel>
        </PanelGroup>
      </main>

      <StatusBar cursor={cursor} />
    </div>
  );
}
