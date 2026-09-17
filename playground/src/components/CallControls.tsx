import { StatePane } from "./StatePane";
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
        <span>{deployed ? `${contract.name} is ready. Each call continues from here.`
          : sandbox && sandboxVersion !== version ? "Source changed. The next call starts a new contract."
          : sandbox ? `The next call replaces ${sandbox.contract} with ${contract.name}.`
          : `The first call starts ${contract.name}.`}</span>
        {sandbox ? <button className="button button--ghost" type="button" disabled={compiling}
          onClick={() => useWorkspaceStore.getState().resetSandbox()} title="Reset this contract; deploy again on the next call">
          <RotateCcw size={14} aria-hidden="true" />Reset contract
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
          <Play size={14} aria-hidden="true" />Call function
        </button>
      </div>
      <p className="call-controls__hint">Each click calls this function once. Changes are kept until you reset the contract.</p>
      {selected ? <div className="call-controls__hint">Inputs copied from <code>{selected.file}:{selected.line}</code>. This calls the function without running its test setup.</div> : null}
      <details className="call-controls__more"><summary>More options</summary>
        <button className="button button--secondary" type="button" disabled={compiling || !method}
          onClick={() => {
            if (!method) return;
            update({ contract: contract.name, signature: method.signature });
            void useWorkspaceStore.getState().runCall(true);
          }}>Preview without saving</button>
        <p className="call-controls__hint">Preview runs one call against this contract and discards its changes.</p>
      <div className="call-controls__actions">
        <button className="button button--ghost" type="button" disabled={compiling || watched || !method?.returnsValue || watches.length >= 16}
          title={watches.length >= 16 ? "Up to 16 watches" : "Simulate this call now and after runs; discard its changes"}
          onClick={() => { if (method) useWorkspaceStore.getState().addWatch({
            contract: contract.name, signature: method.signature, arguments: draft.arguments,
          }); }}><Eye size={14} aria-hidden="true" />{watched ? "Watched" : "Watch this call"}</button>
      </div>
      <StatePane />
      </details>
    </form>
  );
}
