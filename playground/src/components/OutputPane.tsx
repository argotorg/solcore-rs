import Editor, { type BeforeMount } from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { CircleCheck, CircleX, Info, TriangleAlert } from "lucide-react";
import { useCallback, useMemo } from "react";
import type { Diag, Pos, Severity } from "../compiler/types";
import { monacoThemeFor, registerSolcoreLanguage } from "../monaco/solc-language";
import { requestEditorNavigation } from "./editorNavigation";
import { useWorkspaceStore, type OutputTab } from "../store/workspace";

function diagnosticRange(diagnostic: Diag): Pos | null {
  return diagnostic.primary ?? diagnostic.labels[0]?.range ?? null;
}

function severityIcon(severity: Severity): JSX.Element {
  switch (severity) {
    case "error":
      return <CircleX size={16} />;
    case "warning":
      return <TriangleAlert size={16} />;
    case "note":
      return <Info size={16} />;
    case "help":
      return <CircleCheck size={16} />;
  }
}

function formatLocation(range: Pos | null): string {
  if (!range) {
    return "workspace";
  }

  return `${range.file}:${range.startLine}:${range.startCol}`;
}

function outputText(
  tab: OutputTab,
  hull: string | null,
  yul: string | null,
  sonatina: string | null,
  abi: string | null,
): string {
  if (tab === "hull") {
    return hull ?? "// Hull output will appear here after a successful compile.";
  }

  if (tab === "yul") {
    return yul ?? "// Yul output will appear here after a successful compile.";
  }

  if (tab === "sonatina") {
    return sonatina ?? "; Sonatina IR output will appear here after a successful compile.";
  }

  if (tab === "abi") {
    return abi ?? "// ABI output will appear here after compiling a contract.";
  }

  return "";
}

export function OutputPane(): JSX.Element {
  const rawResult = useWorkspaceStore((state) => state.result);
  const workspaceVersion = useWorkspaceStore((state) => state.workspaceVersion);
  const lastCompiledVersion = useWorkspaceStore((state) => state.lastCompiledVersion);
  const outputTab = useWorkspaceStore((state) => state.outputTab);
  const theme = useWorkspaceStore((state) => state.theme);
  const files = useWorkspaceStore((state) => state.files);
  const setOutputTab = useWorkspaceStore((state) => state.setOutputTab);
  const setActive = useWorkspaceStore((state) => state.setActive);
  const result = lastCompiledVersion === workspaceVersion ? rawResult : null;

  const diagnostics = result?.diagnostics ?? [];
  const problemCount = diagnostics.filter(
    (diagnostic) => diagnostic.severity === "error" || diagnostic.severity === "warning",
  ).length;
  const beforeMount = useCallback<BeforeMount>((monaco) => {
    registerSolcoreLanguage(monaco);
  }, []);

  const editorOptions = useMemo<Monaco.editor.IStandaloneEditorConstructionOptions>(
    () => ({
      automaticLayout: true,
      domReadOnly: true,
      fontFamily:
        '"SFMono-Regular", Consolas, "Liberation Mono", Menlo, ui-monospace, monospace',
      fontSize: 13,
      lineHeight: 21,
      minimap: { enabled: false },
      padding: { top: 16, bottom: 16 },
      readOnly: true,
      renderLineHighlight: "none",
      scrollBeyondLastLine: false,
      smoothScrolling: true,
      wordWrap: "on",
    }),
    [],
  );

  const renderedOutput = outputText(
    outputTab,
    result?.hull ?? null,
    result?.yul ?? null,
    result?.sonatina ?? null,
    result?.abi ?? null,
  );

  return (
    <section className="output-pane" aria-label="Compiler output">
      <div className="output-tabs" role="tablist" aria-label="Output tabs">
        <button
          type="button"
          role="tab"
          aria-selected={outputTab === "hull"}
          className={`output-tab ${outputTab === "hull" ? "is-active" : ""}`}
          onClick={() => setOutputTab("hull")}
        >
          Hull
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={outputTab === "yul"}
          className={`output-tab ${outputTab === "yul" ? "is-active" : ""}`}
          onClick={() => setOutputTab("yul")}
        >
          Yul
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={outputTab === "sonatina"}
          className={`output-tab ${outputTab === "sonatina" ? "is-active" : ""}`}
          onClick={() => setOutputTab("sonatina")}
        >
          Sonatina IR
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={outputTab === "abi"}
          className={`output-tab ${outputTab === "abi" ? "is-active" : ""}`}
          onClick={() => setOutputTab("abi")}
        >
          ABI
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={outputTab === "problems"}
          className={`output-tab ${outputTab === "problems" ? "is-active" : ""}`}
          onClick={() => setOutputTab("problems")}
        >
          Problems
          {problemCount > 0 ? <span className="tab-badge">{problemCount}</span> : null}
        </button>
      </div>

      <div className="output-content">
        {outputTab === "problems" ? (
          <div className="problems-list">
            {diagnostics.length === 0 ? (
              <div className="empty-state">
                <CircleCheck size={20} />
                <span>
                  {result ? "No problems - compiles cleanly ✓" : "Compile to view diagnostics."}
                </span>
              </div>
            ) : (
              diagnostics.map((diagnostic, index) => {
                const range = diagnosticRange(diagnostic);
                const canNavigate = Boolean(range && files[range.file]);

                return (
                  <button
                    type="button"
                    key={`${diagnostic.code ?? "diag"}-${index}`}
                    className={`problem-item problem-item--${diagnostic.severity}`}
                    onClick={() => {
                      if (!range || !canNavigate) {
                        return;
                      }

                      setActive(range.file);
                      requestEditorNavigation({ path: range.file, range });
                    }}
                    disabled={!canNavigate}
                  >
                    <span className="problem-item__icon" aria-hidden="true">
                      {severityIcon(diagnostic.severity)}
                    </span>
                    <span className="problem-item__body">
                      <span className="problem-item__message">
                        {diagnostic.code ? (
                          <span className="problem-item__code">{diagnostic.code}</span>
                        ) : null}
                        {diagnostic.message}
                      </span>
                      <span className="problem-item__location">{formatLocation(range)}</span>
                    </span>
                  </button>
                );
              })
            )}
          </div>
        ) : (
          <Editor
            beforeMount={beforeMount}
            defaultLanguage="plaintext"
            language={outputTab === "abi" ? "json" : "plaintext"}
            options={editorOptions}
            theme={monacoThemeFor(theme)}
            value={renderedOutput}
          />
        )}
      </div>
    </section>
  );
}
