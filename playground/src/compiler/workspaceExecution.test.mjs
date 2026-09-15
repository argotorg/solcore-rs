import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

const pending = [];
globalThis.__playgroundCompileMock = Object.fromEntries(['compile', 'run'].map(kind => [
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
  assert.equal(store.getState().outputTab, 'hull');
  assert.equal(store.getState().result, null);
  assert.equal(store.getState().running, false);
  assert.equal(visibleResult(), null);
});

test('inline test results survive edits and Compile, then clear on Run', async () => {
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
  operation = store.getState().runNow();
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
  assert.deepEqual(store.getState().testResults, []);
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
  assert.equal(store.getState().manualResult.decoded, '7');
  const rerun = store.getState().runCall();
  const next = pending.shift();
  assert.ok(next.input.sandboxEpoch > epoch);
  assert.equal(store.getState().manualResult, null);
  next.resolve(result);
  await rerun;
});
