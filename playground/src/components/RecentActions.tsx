import { useEffect, useRef } from "react";
import { formatCall } from "../compiler/formatCall";
import type { ExecutionEvent, ExecutionResult } from "../compiler/types";
import { useWorkspaceStore } from "../store/workspace";

function effect(event: ExecutionEvent): string {
  if (event.result.status === "revert" || event.result.status === "halt") return "Contract changes reverted";
  if (event.result.status === "error") return "Not completed";
  if (event.simulate) return "Changes discarded";
  return event.kind === "deploy" ? "Deployed" : "Changes kept";
}

function ResultDetails({ result }: { result: ExecutionResult }): JSX.Element {
  return <details className="run-action__details"><summary>Details</summary>
    <dl>{result.deploymentGasUsed !== null ? <><dt>Deployment gas</dt><dd>{result.deploymentGasUsed.toLocaleString()}</dd></> : null}<dt>Gas used</dt><dd>{result.gasUsed.toLocaleString()}</dd>
      <dt>{result.status === "revert" ? "Revert data" : "Return data"}</dt><dd><code>{result.returnData}</code></dd></dl>
  </details>;
}

export function RecentActions(): JSX.Element | null {
  const actions = useWorkspaceStore((s) => s.recentActions);
  const sandbox = useWorkspaceStore((s) => s.sandbox);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const running = useWorkspaceStore((s) => s.running);
  const list = useRef<HTMLOListElement>(null);
  useEffect(() => { if (list.current) list.current.scrollTop = list.current.scrollHeight; }, [actions]);
  if (!actions.length && !running) return null;
  return <section className="recent-actions" aria-label="Recent actions">
    <h3>Recent actions</h3>
    <ol ref={list}>{actions.map((action) => <li key={action.id} value={action.id}>
      <details className="run-action" open={action.id === actions.at(-1)?.id ? true : undefined}>
        <summary><span>{action.label}</span>
          {action.result ? <span className={`run-action__status run-action__status--${action.result.status}`}>
            {action.result.decoded != null ? `→ ${action.result.decoded} · ` : ""}{action.result.status}
          </span> : null}
        </summary>
        {action.version !== version ? <p className="call-controls__hint">Earlier source version</p>
          : action.sandboxId !== null && action.sandboxId !== sandbox?.id ? <p className="call-controls__hint">Previous sandbox</p> : null}
        {action.events.length === 1 && action.events[0].kind === "call" ? <>
          <small>{effect(action.events[0])}</small>
          <ResultDetails result={action.events[0].result} />
        </> : action.events.length ? <ol className="run-action__events">{action.events.map((event, index) => <li key={index}>
          <div><code>{event.kind === "deploy" ? `Deploy ${formatCall(event.contract, event.arguments)}`
            : `${event.kind === "setup" ? "Setup: " : event.kind === "check" ? "Check: " : event.simulate ? "Simulate " : "Run "}${event.contract}.${formatCall(event.signature, event.arguments)}`}</code>
            {event.result.decoded != null ? <> → <code>{event.result.decoded}</code></> : null}</div>
          <small>{effect(event)}{event.passed != null ? event.passed ? " · Passed" : " · Failed" : event.result.status !== "success" ? ` · ${event.result.status}` : ""}</small>
          {event.passed === false ? <p>Expected <code>{event.expected}</code></p> : null}
          {event.result.message ? <p className={event.passed ? undefined : "run-pane__error"}>{event.result.message}</p> : null}
          <ResultDetails result={event.result} />
        </li>)}</ol> : action.result && action.result.phase !== "prepare" ? <>
          {action.result.returnWord != null ? <p>Returned <code>{action.result.returnWord}</code></p> : null}
          <ResultDetails result={action.result} />
        </> : null}
        {action.message ? <p>{action.message}</p> : null}
      </details>
    </li>)}</ol>
    {running ? <p role="status">Running…</p> : null}
  </section>;
}
