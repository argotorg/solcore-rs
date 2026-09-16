import { RefreshCw, X } from "lucide-react";
import { useWorkspaceStore } from "../store/workspace";

export function StatePane(): JSX.Element | null {
  const contracts = useWorkspaceStore((s) => s.contracts);
  const draft = useWorkspaceStore((s) => s.callDraft);
  const watches = useWorkspaceStore((s) => s.watches);
  const values = useWorkspaceStore((s) => s.watchValues);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const watchVersion = useWorkspaceStore((s) => s.watchVersion);
  const epoch = useWorkspaceStore((s) => s.sandboxEpoch);
  const watchEpoch = useWorkspaceStore((s) => s.watchEpoch);
  const revision = useWorkspaceStore((s) => s.watchRevision);
  const sandbox = useWorkspaceStore((s) => s.sandbox);
  const sandboxVersion = useWorkspaceStore((s) => s.sandboxVersion);
  const loading = useWorkspaceStore((s) => s.watchLoading);
  const compiling = useWorkspaceStore((s) => s.compiling);
  const contract = contracts.find((c) => c.name === draft.contract)?.name ?? contracts[0]?.name ?? (draft.contract || watches[0]?.contract);
  const visible = watches.filter((watch) => watch.contract === contract);
  if (!contracts.length && !visible.length) return null;
  const current = sandbox?.contract === contract && sandboxVersion === version && watchVersion === version && watchEpoch === epoch;
  const canRefresh = !compiling && !loading && sandbox?.contract === contract && sandboxVersion === version;
  return <section className="state-pane" aria-label="Watched contract state">
    <div className="state-pane__heading">
      <h3>State</h3>
      <span>{loading ? "Updating…" : visible.some((watch) => values[watch.id]) && !current ? "Outdated" : ""}</span>
      {visible.length ? <button type="button" className="button button--ghost" aria-label="Refresh watched state"
        title="Refresh watched state" disabled={!canRefresh} onClick={() => void useWorkspaceStore.getState().refreshWatches()}>
        <RefreshCw size={14} aria-hidden="true" />
      </button> : null}
    </div>
    {visible.length ? <table><thead><tr><th>Watch</th><th>Value</th><th><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>{visible.map((watch) => {
        const result = values[watch.id];
        return <tr key={watch.id}>
          <th scope="row"><code>{watch.signature}</code>{watch.arguments !== "[]" ? <small>{watch.arguments}</small> : null}</th>
          <td key={`${watch.id}:${revision}`} className={`state-pane__value${result?.changed && current ? " state-pane__value--changed" : ""}${!current ? " state-pane__value--outdated" : ""}`}>
            {result?.error ? <span className="run-pane__error">{result.error}</span> : result?.value != null ? <code>{result.value}</code> : loading ? "Reading…" : "Run a call first"}
          </td>
          <td><button type="button" className="button button--ghost" aria-label={`Remove watch ${watch.signature} ${watch.arguments}`}
            onClick={() => useWorkspaceStore.getState().removeWatch(watch.id)}><X size={14} aria-hidden="true" /></button></td>
        </tr>;
      })}</tbody></table> : <p>Watch a function above to inspect its value.</p>}
  </section>;
}
