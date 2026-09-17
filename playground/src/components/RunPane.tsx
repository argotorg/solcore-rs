import { TabList } from "./TabList";
import { Loader2, Play } from "lucide-react";
import { RecentActions, TestSequence } from "./RecentActions";
import { CallControls } from "./CallControls";
import { formatExecution } from "../compiler/executionOutput";
import { useWorkspaceStore } from "../store/workspace";
import { requestEditorNavigation } from "./editorNavigation";

export function RunPane(): JSX.Element {
  const activity = useWorkspaceStore((s) => s.runActivity);
  const setActivity = useWorkspaceStore((s) => s.setRunActivity);
  const cases = useWorkspaceStore((s) => s.testCases);
  const results = useWorkspaceStore((s) => s.testResults);
  const resultsVersion = useWorkspaceStore((s) => s.testResultsVersion);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const hasMain = useWorkspaceStore((s) => s.hasMain);
  const discovered = useWorkspaceStore((s) => s.discoveryVersion === s.workspaceVersion);
  const compiling = useWorkspaceStore((s) => s.compiling);
  const manualResult = useWorkspaceStore((s) => s.manualResult);
  const contracts = useWorkspaceStore((s) => s.contracts);
  const running = useWorkspaceStore((s) => s.running);
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
      <TabList className="run-activities" label="Run activity">
        <button role="tab" id="calls-tab" aria-controls="calls-panel" aria-selected={activity === "calls"} tabIndex={activity === "calls" ? 0 : -1} disabled={compiling} onClick={() => setActivity("calls")}>Try calls</button>
        <button role="tab" id="tests-tab" aria-controls="tests-panel" aria-selected={activity === "tests"} tabIndex={activity === "tests" ? 0 : -1} disabled={compiling} onClick={() => setActivity("tests")}>Run tests ({cases.length})</button>
      </TabList>
      <section hidden={activity !== "calls"} role="tabpanel" id="calls-panel" aria-labelledby="calls-tab">
        {hasMain && !cases.length && !contracts.some((contract) => contract.methods.length > 0) ? <div className="run-pane__toolbar">
          <button type="button" className="button button--primary" disabled={compiling || !discovered}
            title="Compile and run main()" onClick={() => void run()}>
            {running ? <Loader2 className="spin" size={16} /> : <Play size={16} />}
            {running ? "Running…" : "Run"}
          </button>
        </div> : null}
        <CallControls />
        <RecentActions />
        {!actions.length && !contracts.length && !manualResult ? <pre className="run-pane__output">{running ? "" : !discovered ? "Checking runnable code…" : hasMain ? formatExecution(null) : "No runnable functions in this file."}</pre> : null}
      </section>
      <section hidden={activity !== "tests"} role="tabpanel" id="tests-panel" aria-labelledby="tests-tab">
        <p className="call-controls__hint">Running all tests starts each contract fresh, then runs its tests in order. Your calls in Try calls are untouched.</p>
        <button className="button button--primary" disabled={compiling || !cases.length} onClick={() => void run()}>{running ? "Running tests…" : "Run all tests from start"}</button>
        <TestSequence />
      {tests.length ? <details className="run-tests">
        <summary>Individual tests ({tests.length}){hasResults && completed.length ? ` · ${passed}/${completed.length} passed` : ""}{outdated ? " (outdated)" : ""}</summary>
      {tests.length ? tests.map((test) => {
        const canRun = !compiling && cases.some((current) => current.id === test.id);
        return (
          <details className="run-test" key={test.id} >
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
      {!tests.length ? <p>No test comments in this file.</p> : null}
      </section>
    </div>
  );
}
