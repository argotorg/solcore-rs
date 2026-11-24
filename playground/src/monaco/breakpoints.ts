import type * as Monaco from "monaco-editor";
import { useWorkspaceStore } from "../store/workspace";
import { workspacePathFromUri } from "./paths";

export function attachBreakpoints(editor: Monaco.editor.IStandaloneCodeEditor, monaco: typeof Monaco): Monaco.IDisposable {
  const marks = editor.createDecorationsCollection();
  const path = () => workspacePathFromUri(editor.getModel()?.uri.toString() ?? "");
  const render = (): void => {
    const model = editor.getModel();
    const file = path();
    const lines = file ? useWorkspaceStore.getState().breakpoints[file] : undefined;
    marks.set((lines ?? []).filter((line) => model && line <= model.getLineCount()).map((line) => ({
      range: new monaco.Range(line, 1, line, 1),
      options: {
        glyphMarginClassName: "debug-breakpoint",
        glyphMarginHoverMessage: { value: "Breakpoint" },
        stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
      },
    })));
  };
  const toggle = (line: number | undefined): void => {
    const file = path();
    if (!file || !line) return;
    const state = useWorkspaceStore.getState();
    const lines = state.breakpoints[file] ?? [];
    state.setBreakpoints(file, lines.includes(line) ? lines.filter((item) => item !== line) : [...lines, line].sort((a, b) => a - b));
  };
  const mouse = editor.onMouseDown(({ event, target }) => {
    if (event.leftButton && target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN) {
      toggle(target.position?.lineNumber);
    }
  });
  const action = editor.addAction({
    id: "solcore.toggleBreakpoint",
    label: "Toggle Breakpoint",
    keybindings: [monaco.KeyCode.F9],
    contextMenuGroupId: "debug",
    run: () => toggle(editor.getPosition()?.lineNumber),
  });
  const model = editor.onDidChangeModel(render);
  const content = editor.onDidChangeModelContent((event) => {
    const file = path();
    if (!file || event.isFlush) return;
    const lines = [...new Set(marks.getRanges().map((range) => range.startLineNumber))].sort((a, b) => a - b);
    useWorkspaceStore.getState().setBreakpoints(file, lines);
  });
  const unsubscribe = useWorkspaceStore.subscribe((state, previous) => {
    if (state.breakpoints !== previous.breakpoints) render();
  });
  render();

  return {
    dispose() {
      mouse.dispose();
      action.dispose();
      model.dispose();
      content.dispose();
      unsubscribe();
    },
  };
}
