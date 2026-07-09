import Editor, { type BeforeMount, type OnMount } from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { useCallback, useEffect, useMemo, useRef } from "react";
import type { CompileResult, Diag, Pos, Severity } from "../compiler/types";
import { attachLanguageClient, type DetachLanguageClient } from "../languageClient";
import {
  monacoThemeFor,
  registerSolcoreLanguage,
  SOLCORE_LANGUAGE_ID,
} from "../monaco/solc-language";
import { uriForWorkspacePath } from "../monaco/paths";
import { useWorkspaceStore } from "../store/workspace";
import { FileProblemBadge, fileProblemSummaries } from "./FileProblemBadge";
import {
  consumePendingNavigation,
  subscribeEditorNavigation,
  type EditorNavigationTarget,
} from "./editorNavigation";

export interface CursorPosition {
  line: number;
  column: number;
}

interface EditorPaneProps {
  onCursorChange: (cursor: CursorPosition) => void;
}

const COMPILE_MARKER_OWNER = "solcore-compile";
let languageClientAttached = false;
let detachLanguageClient: DetachLanguageClient | null = null;

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    detachLanguageClient?.();
    detachLanguageClient = null;
    languageClientAttached = false;
  });
}

function markerSeverity(monaco: typeof Monaco, severity: Severity): Monaco.MarkerSeverity {
  switch (severity) {
    case "error":
      return monaco.MarkerSeverity.Error;
    case "warning":
      return monaco.MarkerSeverity.Warning;
    case "note":
      return monaco.MarkerSeverity.Info;
    case "help":
      return monaco.MarkerSeverity.Hint;
  }
}

function markerMessage(diagnostic: Diag, labelMessage: string | null): string {
  const prefix = diagnostic.code ? `[${diagnostic.code}] ` : "";
  const lines = [`${prefix}${diagnostic.message}`];

  if (labelMessage) {
    lines.push(labelMessage);
  }

  for (const note of diagnostic.notes) {
    lines.push(`note: ${note}`);
  }

  for (const help of diagnostic.helps) {
    lines.push(`help: ${help}`);
  }

  return lines.join("\n");
}

function normalizeMarkerRange(range: Pos): Monaco.IRange {
  return {
    startLineNumber: range.startLine,
    startColumn: range.startCol,
    endLineNumber: range.endLine,
    endColumn: Math.max(range.endCol, range.startCol + 1),
  };
}

function selectionAtRangeStart(range: Monaco.IRange): Monaco.ISelection {
  return {
    selectionStartLineNumber: range.endLineNumber,
    selectionStartColumn: range.endColumn,
    positionLineNumber: range.startLineNumber,
    positionColumn: range.startColumn,
  };
}

function diagnosticsToMarkers(
  monaco: typeof Monaco,
  result: CompileResult | null,
  activePath: string,
): Monaco.editor.IMarkerData[] {
  if (!result) {
    return [];
  }

  return result.diagnostics.flatMap((diagnostic) => {
    const labels =
      diagnostic.labels.length > 0
        ? diagnostic.labels
        : diagnostic.primary
          ? [{ range: diagnostic.primary, message: null, isPrimary: true }]
          : [];

    return labels
      .filter((label) => label.range.file === activePath)
      .map((label) => ({
        ...normalizeMarkerRange(label.range),
        severity: markerSeverity(monaco, diagnostic.severity),
        message: markerMessage(diagnostic, label.message),
        code: diagnostic.code ?? undefined,
        source: label.isPrimary ? "solcore" : "solcore label",
      }));
  });
}

export function EditorPane({ onCursorChange }: EditorPaneProps): JSX.Element {
  const files = useWorkspaceStore((state) => state.files);
  const order = useWorkspaceStore((state) => state.order);
  const activePath = useWorkspaceStore((state) => state.activePath);
  const entry = useWorkspaceStore((state) => state.entry);
  const result = useWorkspaceStore((state) => state.result);
  const theme = useWorkspaceStore((state) => state.theme);
  const setActive = useWorkspaceStore((state) => state.setActive);
  const setContent = useWorkspaceStore((state) => state.setContent);
  const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);
  const monacoRef = useRef<typeof Monaco | null>(null);
  const activePathRef = useRef(activePath);

  const activeFile = files[activePath];
  const editorUri = uriForWorkspacePath(activePath);
  const problemsByFile = useMemo(() => fileProblemSummaries(result), [result]);

  const editorOptions = useMemo<Monaco.editor.IStandaloneEditorConstructionOptions>(
    () => ({
      automaticLayout: true,
      bracketPairColorization: { enabled: true },
      cursorBlinking: "smooth",
      fontFamily:
        '"SFMono-Regular", Consolas, "Liberation Mono", Menlo, ui-monospace, monospace',
      fontLigatures: false,
      fontSize: 14,
      lineHeight: 22,
      minimap: { enabled: false },
      padding: { top: 18, bottom: 18 },
      renderLineHighlight: "gutter",
      scrollBeyondLastLine: false,
      smoothScrolling: true,
      tabSize: 2,
      wordWrap: "on",
    }),
    [],
  );

  const beforeMount = useCallback<BeforeMount>((monaco) => {
    registerSolcoreLanguage(monaco);
    if (!languageClientAttached) {
      languageClientAttached = true;
      detachLanguageClient = attachLanguageClient(monaco);
    }
  }, []);

  const revealRange = useCallback((range: Pos): void => {
    const editor = editorRef.current;
    if (!editor) {
      return;
    }

    const monacoRange = normalizeMarkerRange(range);
    editor.setSelection(selectionAtRangeStart(monacoRange));
    editor.revealRangeInCenter(monacoRange, 0);
    editor.focus();
  }, []);

  const handleNavigation = useCallback(
    (target: EditorNavigationTarget): void => {
      if (target.path !== activePathRef.current) {
        return;
      }

      revealRange(target.range);
    },
    [revealRange],
  );

  const onMount = useCallback<OnMount>(
    (editor, monaco) => {
      editorRef.current = editor;
      monacoRef.current = monaco;
      monaco.editor.setTheme(monacoThemeFor(theme));
      onCursorChange({
        line: editor.getPosition()?.lineNumber ?? 1,
        column: editor.getPosition()?.column ?? 1,
      });
      editor.onDidChangeCursorPosition((event) => {
        onCursorChange({
          line: event.position.lineNumber,
          column: event.position.column,
        });
      });
    },
    [onCursorChange, theme],
  );

  useEffect(() => {
    activePathRef.current = activePath;
  }, [activePath]);

  useEffect(() => subscribeEditorNavigation(handleNavigation), [handleNavigation]);

  useEffect(() => {
    const pending = consumePendingNavigation(activePath);
    if (pending) {
      window.requestAnimationFrame(() => revealRange(pending.range));
    }
  }, [activePath, revealRange]);

  useEffect(() => {
    const monaco = monacoRef.current;
    if (!monaco) {
      return;
    }

    monaco.editor.setTheme(monacoThemeFor(theme));
  }, [theme]);

  useEffect(() => {
    const monaco = monacoRef.current;
    const editor = editorRef.current;
    const model = editor?.getModel();

    if (!monaco || !model) {
      return;
    }

    monaco.editor.setModelMarkers(
      model,
      COMPILE_MARKER_OWNER,
      diagnosticsToMarkers(monaco, result, activePath),
    );
  }, [activePath, result]);

  return (
    <section className="editor-pane" aria-label="Source editor">
      <div className="tab-strip" role="tablist" aria-label="Open files">
        {order.map((path) => {
          const problemSummary = problemsByFile.get(path);

          return (
            <button
              key={path}
              type="button"
              role="tab"
              aria-selected={path === activePath}
              className={`source-tab ${path === activePath ? "is-active" : ""}`}
              onClick={() => setActive(path)}
              title={path}
            >
              <span className="source-tab__name">{path}</span>
              {problemSummary ? <FileProblemBadge summary={problemSummary} /> : null}
              {path === entry ? <span className="source-tab__entry">entry</span> : null}
            </button>
          );
        })}
      </div>

      <div className="editor-surface">
        <Editor
          beforeMount={beforeMount}
          defaultLanguage={SOLCORE_LANGUAGE_ID}
          keepCurrentModel
          language={SOLCORE_LANGUAGE_ID}
          onChange={(value) => setContent(activePath, value ?? "")}
          onMount={onMount}
          options={editorOptions}
          path={editorUri}
          theme={monacoThemeFor(theme)}
          value={activeFile?.content ?? ""}
        />
      </div>
    </section>
  );
}
