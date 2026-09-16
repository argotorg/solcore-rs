import { RecentActions } from "./RecentActions";
import { StatePane } from "./StatePane";
import { CallControls } from "./CallControls";
import { formatExecution } from "../compiler/executionOutput";
import { useWorkspaceStore } from "../store/workspace";
import { requestEditorNavigation } from "./editorNavigation";

export function RunPane(): JSX.Element {
  const cases = useWorkspaceStore((s) => s.testCases);
  const results = useWorkspaceStore((s) => s.testResults);
  const resultsVersion = useWorkspaceStore((s) => s.testResultsVersion);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const compiling = useWorkspaceStore((s) => s.compiling);
  const manualResult = useWorkspaceStore((s) => s.manualResult);
  const contracts = useWorkspaceStore((s) => s.contracts);
  const selectedId = useWorkspaceStore((s) => s.selectedTestId);
  const running = useWorkspaceStore((s) => s.running);
  const execution = useWorkspaceStore((s) => s.result?.execution ?? null);
  const run = useWorkspaceStore((s) => s.runNow);
  const hasResults = resultsVersion !== null;
  const outdated = hasResults && resultsVersion !== version;
  const tests = hasResults ? results : cases.map((test) => ({
    ...test,
    status: test.status === "error" ? "error" : "ready",
    actual: null,
    gasUsed: null,
  }));
  const completed = tests.filter((test) => test.status !== "ready" && !test.replayed);
  const actions = useWorkspaceStore((s) => s.recentActions);
  const passed = completed.filter((test) => test.status === "passed").length;

  return (
    <div className="run-pane">
      <CallControls />
      <RecentActions />
      <StatePane />
      {tests.length ? <details className="run-tests" open={selectedId ? true : undefined}>
        <summary>Tests ({tests.length}){hasResults && completed.length ? ` · ${passed}/${completed.length} passed` : ""}{outdated ? " (outdated)" : ""}</summary>
        <p className="call-controls__hint">Tests start fresh. Running one test replays earlier setup calls in its contract. Assertions discard changes.</p>
      {tests.length ? tests.map((test) => {
        const canRun = !compiling && cases.some((current) => current.id === test.id);
        return (
          <details className="run-test" key={test.id} open={selectedId === test.id ? true : undefined}>
            <summary>
              <span className={`run-test__status run-test__status--${test.status}`}>
                {test.status === "passed" ? "✓" : test.status === "ready" ? "○" : "✗"}
              </span>
              <code>{test.invocation?.signature ?? test.contract}: {test.label}</code>
              <span className="run-test__outcome">{test.status === "ready" ? "Not run" : test.replayed ? `Setup ${test.status}` : test.status}</span>
            </summary>
            <div className="run-test__details">
              <dl>
                <dt>Contract</dt><dd>{test.contract || "None"}</dd>
                {test.expected !== null ? <><dt>Expected</dt><dd><code>{test.expected}</code></dd></> : null}
                {test.actual !== null ? <><dt>Actual</dt><dd><code>{test.actual}</code></dd></> : null}
                {test.gasUsed !== null ? <><dt>Gas used</dt><dd>{test.gasUsed.toLocaleString()}</dd></> : null}
              </dl>
              {test.message ? <p className="run-pane__error">{test.message}</p> : null}
              <p className="call-controls__hint">Starts fresh{(() => {
                const setup = cases.filter((t) => t.contract === test.contract && t.line < test.line && t.invocation?.simulate === false).length;
                return setup ? ` → replays ${setup} setup call${setup === 1 ? "" : "s"}` : "";
              })()} → runs this test</p>
              <div className="run-test__actions">
                <button type="button" className="button button--secondary" disabled={!canRun}
                  onClick={() => void run(test.id)}>Run test</button>
                <button type="button" className="run-test__source" onClick={() => {
                  const state = useWorkspaceStore.getState();
                  const file = state.files[test.file];
                  if (!file) return;
                  const line = Math.min(test.line, file.content.split("\n").length);
                  state.setActive(test.file);
                  requestEditorNavigation({ path: test.file, range: {
                    file: test.file, startByte: 0, endByte: 0,
                    startLine: line, endLine: line, startCol: 1, endCol: 1,
                  } });
                }}>{test.file}:{test.line}</button>
              </div>
            </div>
          </details>
        );
      }) : null}
      </details> : null}
      {!tests.length && !actions.length && !contracts.length && !manualResult ? <pre className="run-pane__output">{running ? "" : formatExecution(execution)}</pre> : null}
    </div>
  );
}
