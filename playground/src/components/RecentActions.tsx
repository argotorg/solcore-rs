import { useEffect, useRef } from "react";
import { formatCall } from "../compiler/formatCall";
import type { ExecutionResult } from "../compiler/types";
import { useWorkspaceStore } from "../store/workspace";

function ResultDetails({ result }: { result: ExecutionResult }): JSX.Element {
  return <dl className="run-action__details">
    {result.deploymentGasUsed != null ? <><dt>Deployment gas</dt><dd>{result.deploymentGasUsed.toLocaleString()}</dd></> : null}
    <dt>Gas used</dt><dd>{result.gasUsed.toLocaleString()}</dd>
    <dt>{result.status === "revert" ? "Revert data" : "Return data"}</dt><dd><code>{result.returnData}</code></dd>
  </dl>;
}

export function RecentActions(): JSX.Element | null {
  const actions = useWorkspaceStore((s) => s.recentActions);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const running = useWorkspaceStore((s) => s.running);
  const list = useRef<HTMLOListElement>(null);
  useEffect(() => { if (list.current) list.current.scrollTop = list.current.scrollHeight; }, [actions]);
  if (!actions.length && !running) return null;
  return <section className="recent-actions" aria-label="Calls">
    <h3>Calls</h3>
    <ol ref={list}>{actions.map((action) => {
      const call = action.events.find((e) => e.kind === "call");
      const deployment = action.events.find((e) => e.kind === "deploy");
      const failed = action.result && action.result.status !== "success";
      const value = action.result?.decoded ?? action.result?.returnWord;
      return <li key={action.id} value={action.id}>
        {deployment ? <p className="call-controls__hint">{deployment.result.status === "success" ? "Started" : "Could not start"} {formatCall(deployment.contract, deployment.arguments)}</p> : null}
        <details className="run-action">
          <summary>
            <code>{call ? `${call.contract}.${formatCall(call.signature, call.arguments)}` : action.label}</code>
            {value != null ? <> → <code>{value}</code></> : null}
            {call?.simulate ? <span className="call-controls__hint"> · Preview, not saved</span> : null}
            {failed ? <span className="run-pane__error"> · {action.message ?? action.result?.status}</span> : null}
            {action.version !== version ? <span className="call-controls__hint"> · Earlier source</span> : null}
          </summary>
          {action.result ? <ResultDetails result={action.result} /> : <p>{action.message}</p>}
        </details>
      </li>;
    })}</ol>
    {running ? <p role="status">Calling…</p> : null}
  </section>;
}

export function TestSequence(): JSX.Element | null {
  const run = useWorkspaceStore((s) => s.testRun);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  if (!run) return null;
  return <section className="test-sequence" aria-label="Last test run">
    <h3>{run.label}</h3>
    <p role="status">{run.message}{run.version !== version ? " (earlier source)" : ""}</p>
    <ol>{run.events.map((event, index) => <li key={index}>
      <details><summary>
        <code>{event.kind === "deploy" ? `Start ${formatCall(event.contract, event.arguments)}`
          : `${event.kind === "setup" ? "Setup: " : "Check: "}${event.contract}.${formatCall(event.signature, event.arguments)}`}</code>
        {event.result.decoded != null ? <> → <code>{event.result.decoded}</code></> : null}
        {event.passed != null ? event.passed ? " · Passed" : " · Failed" : event.result.status !== "success" ? " · Failed" : ""}
      </summary>
        {event.expected ? <p>Expected <code>{event.expected}</code></p> : null}
        {event.result.message ? <p>{event.result.message}</p> : null}
        <ResultDetails result={event.result} />
      </details>
    </li>)}</ol>
    <p className="call-controls__hint">This run is finished. Run it again to start over.</p>
  </section>;
}
