import { Play, RotateCcw, Eye } from "lucide-react";
import { useWorkspaceStore } from "../store/workspace";

export function CallControls(): JSX.Element | null {
  const contracts = useWorkspaceStore((s) => s.contracts);
  const draft = useWorkspaceStore((s) => s.callDraft);
  const selectedId = useWorkspaceStore((s) => s.selectedTestId);
  const watches = useWorkspaceStore((s) => s.watches);
  const tests = useWorkspaceStore((s) => s.testCases);
  const compiling = useWorkspaceStore((s) => s.compiling);
  const sandbox = useWorkspaceStore((s) => s.sandbox);
  const sandboxVersion = useWorkspaceStore((s) => s.sandboxVersion);
  const sandboxAction = useWorkspaceStore((s) => s.sandboxAction);
  const version = useWorkspaceStore((s) => s.workspaceVersion);
  const update = useWorkspaceStore((s) => s.setCallDraft);
  const contract = contracts.find((c) => c.name === draft.contract) ?? contracts[0];
  const method = contract?.methods.find((m) => m.signature === draft.signature) ?? contract?.methods[0];
  const watched = watches.some((watch) => watch.contract === contract?.name
    && watch.signature === method?.signature && watch.arguments === draft.arguments.trim());
  const selected = tests.find((t) => t.id === selectedId);
  const deployed = sandbox?.contract === contract?.name && sandboxVersion === version;
  if (!contract) return null;

  return (
    <form className="call-controls" onSubmit={(event) => {
      event.preventDefault();
      const state = useWorkspaceStore.getState();
      if (!method) return;
      update({ contract: contract.name, signature: method.signature });
      void state.runCall();
    }}>
      <div className="call-controls__sandbox">
        <span>{deployed ? `${contract.name} · after action ${sandboxAction}`
          : sandbox && sandboxVersion !== version ? "Source changed. The next call starts a fresh sandbox."
          : sandbox ? `The next call replaces ${sandbox.contract} with ${contract.name}.`
          : `No deployment. The first call deploys ${contract.name}.`}</span>
        {sandbox ? <button className="button button--ghost" type="button" disabled={compiling}
          onClick={() => useWorkspaceStore.getState().resetSandbox()} title="Clear the sandbox; deploy again on the next call">
          <RotateCcw size={14} aria-hidden="true" />Reset sandbox
        </button> : null}
      </div>
      <div className="call-controls__selection">
        <label>Contract
          <select value={contract.name} disabled={compiling} onChange={(e) => update({
            contract: e.target.value, signature: "", arguments: "[]", constructorArguments: "[]",
          })}>
            {contracts.map((c) => <option key={c.name}>{c.name}</option>)}
          </select>
        </label>
        <label>Function
          <select value={method?.signature ?? ""} disabled={compiling} onChange={(e) => update({
            contract: contract.name, signature: e.target.value, arguments: "[]",
          })}>
            {!method ? <option value="">No public functions</option> : null}
            {contract.methods.map((m) => <option key={m.signature}>{m.signature}</option>)}
          </select>
        </label>
      </div>
      {contract.constructorInputs.length ? <label>Constructor arguments (JSON)
        <input aria-label="Constructor arguments" value={draft.constructorArguments}
          disabled={compiling || deployed} placeholder={`[${contract.constructorInputs.join(", ")}]`}
          onChange={(e) => update({ constructorArguments: e.target.value })} />
      </label> : null}
      {method?.inputs.length ? <label>Arguments (JSON)
        <input aria-label="Arguments" value={draft.arguments} disabled={compiling}
          placeholder={`[${method.inputs.join(", ")}]`} spellCheck={false}
          title="JSON array. Quote large integers to preserve their value."
          onChange={(e) => update({ contract: contract.name, signature: method.signature, arguments: e.target.value })} />
      </label> : null}
      <div className="call-controls__actions">
        <button className="button button--primary" type="submit" disabled={compiling || !method}>
          <Play size={14} aria-hidden="true" />Run call
        </button>
        <button className="button button--secondary" type="button" disabled={compiling || !method}
          onClick={() => {
            if (!method) return;
            update({ contract: contract.name, signature: method.signature });
            void useWorkspaceStore.getState().runCall(true);
          }}>Simulate call</button>
      </div>
      <div className="call-controls__actions">
        <span className="call-controls__hint">Run keeps changes. Simulate discards them.</span>
        <button className="button button--ghost" type="button" disabled={compiling || watched || !method?.returnsValue || watches.length >= 16}
          title={watches.length >= 16 ? "Up to 16 watches" : "Simulate this call now and after runs; discard its changes"}
          onClick={() => { if (method) useWorkspaceStore.getState().addWatch({
            contract: contract.name, signature: method.signature, arguments: draft.arguments,
          }); }}><Eye size={14} aria-hidden="true" />{watched ? "Watched" : "Watch this call"}</button>
      </div>
      {selected ? <div className="call-controls__hint">Inputs from <code>{selected.file}:{selected.line}</code></div> : null}
    </form>
  );
}
