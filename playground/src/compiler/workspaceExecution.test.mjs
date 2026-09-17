import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

const pending = [];
globalThis.__playgroundCompileMock = Object.fromEntries(['compile', 'run', 'watch'].map(kind => [
  kind, input => new Promise(resolve => pending.push({ kind, input, resolve })),
]));
const bundle = await build({
  entryPoints: [new URL('../store/workspace.ts', import.meta.url).pathname],
  bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{
    name: 'compiler-worker-mock',
    setup(build) {
      build.onResolve({ filter: /\.sol\?raw$/ }, ({ path, resolveDir }) => ({ path: new URL(path, 'file://' + resolveDir + '/').pathname, namespace: 'raw' }));
      build.onLoad({ filter: /.*/, namespace: 'raw' }, async ({ path }) => ({
        contents: await readFile(path.replace(/\?raw$/, ''), 'utf8'), loader: 'text',
      }));
      build.onResolve({ filter: /compiler\/compileClient$/ }, () => ({ path: 'mock', namespace: 'mock' }));
      build.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({
        contents: 'export const compileClient = globalThis.__playgroundCompileMock;', loader: 'js',
      }));
    },
  }],
});
const { useWorkspaceStore: store } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
assert.equal(store.getState().outputTab, 'execution');
assert.equal(store.getState().discoveryVersion, null);
const execution = {
  status: 'success', phase: 'call', returnData: '0x', returnWord: '42',
  gasUsed: 21018, deploymentGasUsed: null, gasLimit: 1000000, message: null,
};
const result = { tests: [], success: true, diagnostics: [], hull: null, yul: null, sonatina: null, abi: null, execution };

function visibleResult() {
  const state = store.getState();
  return state.workspaceVersion === state.lastCompiledVersion ? state.result : null;
}

test('Run uses the current workspace and retains its output during edits', async () => {
  store.getState().loadExample('contract-output');
  const operation = store.getState().runNow();
  assert.equal(store.getState().running, true);
  const request = pending.shift();
  assert.equal(request.kind, 'run');
  assert.match(request.input.files[0].content, /return uint256\(42\)/);
  request.resolve(result);
  await operation;
  assert.equal(store.getState().running, false);
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(visibleResult().execution.returnWord, '42');
  store.getState().setContent('Hello.sol', 'function main() returns (word) { return 7; }');
  assert.equal(visibleResult(), null);
  assert.equal(store.getState().result.execution.returnWord, '42');
});

test('starting Run clears the previous execution output', async () => {
  assert.equal(store.getState().result.execution.returnWord, '42');
  const operation = store.getState().runNow();
  assert.equal(store.getState().result.execution, null);
  pending.shift().resolve(result);
  await operation;
  assert.equal(store.getState().result.execution.returnWord, '42');
});

test('edits during Run keep a late result from appearing current', async () => {
  store.getState().loadExample('contract-output');
  const operation = store.getState().runNow();
  const request = pending.shift();
  store.getState().setContent('Hello.sol', 'function main() returns (word) { return 9; }');
  request.resolve(result);
  await operation;
  assert.equal(store.getState().running, false);
  assert.equal(visibleResult(), null);
});

test('a newer Compile result cannot be overwritten by an older Run', async () => {
  const run = store.getState().runNow();
  const old = pending.shift();
  const compile = store.getState().compileNow();
  pending.shift().resolve({ ...result, execution: null });
  await compile;
  old.resolve(result);
  await run;
  assert.equal(visibleResult().execution, null);
  assert.equal(store.getState().running, false);
});

test('switching examples during Run preserves the new example output tab', async () => {
  const operation = store.getState().runNow();
  const request = pending.shift();
  store.getState().loadExample('std-usage');
  request.resolve(result);
  await operation;
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(store.getState().result, null);
  assert.equal(store.getState().running, false);
  assert.equal(visibleResult(), null);
});

test('inline test results survive edits and Compile, then clear on a test rerun', async () => {
  store.getState().loadExample('std-usage');
  const tests = [{ id: 'Calculator.sol:10', status: 'passed', actual: '42' }];
  let operation = store.getState().runNow(tests[0].id);
  let request = pending.shift();
  assert.equal(request.input.testId, tests[0].id);
  request.resolve({ ...result, tests });
  await operation;
  const version = store.getState().testResultsVersion;
  store.getState().setContent('Calculator.sol', '// edited');
  assert.deepEqual(store.getState().testResults, tests);
  assert.notEqual(store.getState().workspaceVersion, version);
  operation = store.getState().compileNow();
  pending.shift().resolve({ ...result, execution: null });
  await operation;
  assert.deepEqual(store.getState().testResults, tests);
  operation = store.getState().runNow(tests[0].id);
  assert.deepEqual(store.getState().testResults, []);
  pending.shift().resolve(result);
  await operation;
});

test('test play fills the manual controls before running and edits detach the assertion', async () => {
  store.getState().loadExample('std-usage');
  const testcase = { id: 'Calculator.sol:20', contract: 'Calculator', invocation: {
    signature: 'viaOperator(uint256,uint256)', arguments: '["20","22"]', simulate: true,
  } };
  store.setState({ testCases: [testcase] });
  const operation = store.getState().runNow(testcase.id);
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(store.getState().selectedTestId, testcase.id);
  assert.deepEqual(store.getState().callDraft, { contract: 'Calculator', constructorArguments: '[]', ...testcase.invocation });
  pending.shift().resolve({ ...result, tests: [testcase], sandbox: { contract: 'Calculator', address: '0x1234' } });
  await operation;
  store.getState().setCallDraft({ arguments: '[3,4]' });
  assert.equal(store.getState().selectedTestId, null);
  assert.equal(store.getState().testResults.length, 1);
  const manual = store.getState().runCall();
  const request = pending.shift();
  assert.equal(request.input.manual.arguments, '[3,4]');
  assert.equal(request.input.testId, undefined);
  assert.deepEqual(store.getState().testResults, [testcase]);
  request.resolve({ ...result, execution: { ...execution, decoded: '7' } });
  await manual;
  assert.equal(store.getState().manualResult.decoded, '7');
  const compile = store.getState().compileNow();
  pending.shift().resolve({ ...result, execution: null });
  await compile;
  assert.equal(store.getState().manualResult.decoded, '7');
  const epoch = request.input.sandboxEpoch;
  const content = store.getState().files['Calculator.sol'].content;
  store.getState().setContent('Calculator.sol', content + '\n');
  store.getState().setContent('Calculator.sol', content);
  store.getState().resetSandbox();
  assert.equal(store.getState().manualResult, null);
  assert.deepEqual(store.getState().recentActions, []);
  const rerun = store.getState().runCall();
  const next = pending.shift();
  assert.ok(next.input.sandboxEpoch > epoch);
  assert.equal(store.getState().manualResult, null);
  next.resolve(result);
  await rerun;
});


test('watches refresh after a call, retain results on edits, and reject late reads after reset', async () => {
  store.getState().loadExample('std-usage');
  const definition = { contract: 'Calculator', signature: 'answer()', arguments: '[]' };
  store.getState().addWatch(definition);
  assert.equal(pending.length, 0, 'adding a watch must not deploy');
  store.getState().addWatch(definition);
  assert.equal(store.getState().watches.length, 1);
  const id = store.getState().watches[0].id;
  const run = store.getState().runCall();
  pending.shift().resolve({ ...result, sandbox: { contract: 'Calculator', address: '0x1234' } });
  await run;
  const first = pending.shift();
  assert.equal(first.kind, 'watch');
  first.resolve([{ id, value: '42', error: null }]);
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(store.getState().watchValues[id].value, '42');
  assert.equal(store.getState().watchValues[id].changed, false);
  const refresh = store.getState().refreshWatches();
  pending.shift().resolve([{ id, value: '43', error: null }]);
  await refresh;
  assert.equal(store.getState().watchValues[id].changed, true);
  const late = store.getState().refreshWatches();
  const request = pending.shift();
  store.getState().setContent('Calculator.sol', '// changed');
  request.resolve([{ id, value: '99', error: null }]);
  await late;
  assert.equal(store.getState().watchValues[id].value, '43');
  assert.notEqual(store.getState().watchVersion, store.getState().workspaceVersion);
  await store.getState().refreshWatches();
  assert.equal(pending.length, 0, 'outdated source must not read an old deployment');
  store.setState({ sandboxVersion: store.getState().workspaceVersion });
  const resetRead = store.getState().refreshWatches();
  const resetRequest = pending.shift();
  store.getState().resetSandbox();
  resetRequest.resolve([{ id, value: '100', error: null }]);
  await resetRead;
  assert.deepEqual(store.getState().watchValues, {});
  assert.equal(store.getState().watches.length, 1);
  store.getState().loadExample('trait');
  assert.equal(store.getState().watches.length, 0);
});

test('recent actions preserve call identity and clear when the contract is reset', async () => {
  store.getState().loadExample('std-usage');
  store.getState().setCallDraft({ contract: 'Counter', signature: 'set(uint256)', arguments: '[7]' });
  const sandbox = { id: 41, contract: 'Counter', address: '0x1234' };
  const deploy = { kind: 'deploy', contract: 'Counter', signature: '', arguments: '[]', simulate: false, result: execution };
  for (const simulate of [false, true]) {
    const run = store.getState().runCall(simulate);
    const request = pending.shift();
    assert.equal(request.input.manual.simulate, simulate);
    request.resolve({ ...result, sandbox, events: simulate ? [] : [deploy] });
    await run;
  }
  let actions = store.getState().recentActions;
  assert.deepEqual(actions.map(a => a.label), ['Call Counter.set(7)', 'Preview Counter.set(7)']);
  assert.equal(actions[0].events[0].kind, 'deploy');
  assert.equal(store.getState().sandboxAction, 2);
  store.getState().setCallDraft({ arguments: '[9]' });
  assert.equal(store.getState().recentActions[0].label, 'Call Counter.set(7)');
  store.getState().setContent('Calculator.sol', '// changed');
  assert.notEqual(store.getState().workspaceVersion, actions[0].version);
  store.getState().resetSandbox();
  assert.equal(store.getState().sandbox, null);
  assert.equal(store.getState().manualResult, null);
  assert.deepEqual(store.getState().recentActions, []);
  for (let i = 0; i < 25; i++) store.getState().resetSandbox();
  actions = store.getState().recentActions;
  assert.equal(actions.length, 0);
  assert.equal(store.getState().actionSequence, 0);
  store.getState().loadExample('trait');
  assert.equal(store.getState().recentActions.length, 0);
});

test('opening manual controls and removing watches do not run calls', async () => {
  store.getState().loadExample('std-usage');
  store.setState({ contracts: [{ name: 'Counter', methods: [{ signature: 'read()' }] }], testCases: [] });
  await store.getState().runNow();
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(pending.length, 0);
  store.setState({ watches: [{ id: 'read', contract: 'Counter', signature: 'read()', arguments: '[]' }] });
  store.getState().removeWatch('read');
  assert.equal(pending.length, 0);
});

test('test runs have separate results and never replace the manual session or history', async () => {
  store.getState().loadExample('std-usage');
  const testcase = { id: 'Calculator.sol:20', file: 'Calculator.sol', line: 20, contract: 'Calculator', status: 'passed', invocation: {
    signature: 'answer()', arguments: '[]', simulate: true,
  } };
  store.getState().setCallDraft({ contract: 'Calculator', signature: 'answer()', constructorArguments: '[7]' });
  const manual = store.getState().runCall();
  const sandbox = { id: 91, contract: 'Calculator', address: '0x1234' };
  pending.shift().resolve({ ...result, tests: [testcase], sandbox });
  await manual;
  const history = store.getState().recentActions;
  const value = store.getState().manualResult;
  for (const id of [testcase.id, undefined]) {
    const tested = store.getState().runNow(id);
    assert.equal(store.getState().runActivity, 'tests');
    assert.equal(store.getState().manualResult, value);
    pending.shift().resolve({ ...result, tests: [testcase], sandbox: null, events: [] });
    await tested;
    assert.equal(store.getState().sandbox, sandbox);
    assert.equal(store.getState().recentActions, history);
    assert.equal(store.getState().actionSequence, 1);
    assert.equal(store.getState().sandboxAction, 1);
    assert.equal(store.getState().callDraft.constructorArguments, '[7]');
    assert.ok(store.getState().testRun);
    assert.equal(pending.length, 0, 'tests must not refresh manual watches');
  }
  const testRun = store.getState().testRun;
  const next = store.getState().runCall();
  assert.equal(store.getState().runActivity, 'calls');
  assert.equal(store.getState().testRun, testRun);
  assert.deepEqual(store.getState().testResults, [testcase]);
  pending.shift().resolve({ ...result, sandbox });
  await next;
  assert.equal(store.getState().recentActions.length, 2);
  store.getState().resetSandbox();
  assert.equal(store.getState().testRun, testRun);
});


test('Run is the default tab on startup, example changes, and workspace reset', () => {
  store.getState().loadExample('contract-output');
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(store.getState().discoveryVersion, null);
  store.getState().setOutputTab('abi');
  store.getState().resetWorkspace();
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(store.getState().options.emitBytecode, true);
});
