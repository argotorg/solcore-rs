import assert from 'node:assert/strict';
import { beforeEach, test } from 'node:test';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

// Exercise the real store and router; only the browser and compiler worker are replaced.
const bundle = await build({
  stdin: {
    contents: `export { attachExampleRouter } from './useExampleRouter';
      export { useWorkspaceStore as store } from '../store/workspace';`,
    resolveDir: new URL('.', import.meta.url).pathname,
    loader: 'ts',
  },
  bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{
    name: 'navigation-environment',
    setup(build) {
      build.onResolve({ filter: /\.sol\?raw$/ }, ({ path, resolveDir }) => ({
        path: new URL(path, 'file://' + resolveDir + '/').pathname, namespace: 'raw',
      }));
      build.onLoad({ filter: /.*/, namespace: 'raw' }, async ({ path }) => ({
        contents: await readFile(path.replace(/\?raw$/, ''), 'utf8'), loader: 'text',
      }));
      build.onResolve({ filter: /compiler\/compileClient$/ }, () => ({ path: 'worker', namespace: 'mock' }));
      build.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({
        contents: `export const compileClient = new Proxy({}, { get() {
          return () => { throw new Error('Navigation must not execute code'); };
        }});`, loader: 'js',
      }));
    },
  }],
});
const { store, attachExampleRouter } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const first = 'viaOperator(uint256,uint256)';
const second = 'viaTrait(uint256,uint256)';
const testId = 'Calculator.sol:359';
const contracts = [{ name: 'Calculator', constructorInputs: [], methods: [
  { signature: first, inputs: ['uint256', 'uint256'] },
  { signature: second, inputs: ['uint256', 'uint256'] },
] }];
const testCases = [{ id: testId, contract: 'Calculator', file: 'Calculator.sol',
  invocation: { signature: second, arguments: '[20,22]', simulate: true } }];

function navigate(href) {
  window.location = new URL(href, window.location.href);
  window.dispatchEvent(new Event('hashchange'));
}
function discover() {
  store.setState({ contracts, testCases, discoveryVersion: store.getState().workspaceVersion });
}

beforeEach(() => {
  const storage = new Map();
  globalThis.window = Object.assign(new EventTarget(), {
    location: new URL('https://example.org/#/examples/std-usage'),
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) },
    history: {
      entries: [],
      pushState(_state, _title, href) { this.entries.push(href); window.location = new URL(href); },
      replaceState(_state, _title, href) { window.location = new URL(href); },
    },
  });
  globalThis.document = { documentElement: { dataset: {} } };
  store.setState(store.getInitialState(), true);
  store.getState().loadExample('std-usage');
  store.setState({ callDraft: { ...store.getState().callDraft, contract: 'Calculator', signature: first } });
  discover();
});

test('Calls survives a round trip even when a test is being viewed', t => {
  t.after(attachExampleRouter());
  store.setState({ viewedTestId: testId, runActivity: 'tests' });
  store.getState().setRunActivity('calls');
  const href = window.location.href;
  assert.match(href, /view=calls/);
  store.getState().setRunActivity('tests');
  const entries = window.history.entries.length;
  navigate(href);
  assert.equal(store.getState().runActivity, 'calls');
  assert.equal(store.getState().viewedTestId, testId);
  assert.equal(window.history.entries.length, entries, 'restoring history must not add another entry');
});

test('viewing another test does not change the manual call or runner selection on restore', t => {
  t.after(attachExampleRouter());
  store.setState({ selectedTestId: 'previously-run-test', viewedTestId: testId, runActivity: 'tests' });
  const before = store.getState().callDraft;
  const href = window.location.href;
  navigate(href);
  assert.deepEqual(store.getState().callDraft, before);
  assert.equal(store.getState().selectedTestId, 'previously-run-test');
  assert.equal(store.getState().viewedTestId, testId);
  assert.equal(store.getState().recentActions.length, 0);
});

test('non-default function and viewed test restore independently', t => {
  t.after(attachExampleRouter());
  store.getState().setCallDraft({ signature: second });
  store.setState({ viewedTestId: testId });
  const href = window.location.href;
  assert.match(href, /function=/);
  store.getState().setCallDraft({ signature: first });
  navigate(href);
  assert.equal(store.getState().callDraft.signature, second);
  assert.equal(store.getState().viewedTestId, testId);
  assert.equal(store.getState().runActivity, 'calls');
});

test('delayed discovery restores test and function without overriding explicit Calls', t => {
  window.location.hash = `#/examples/std-usage?view=calls&function=${encodeURIComponent(second)}&test=${encodeURIComponent(testId)}`;
  store.setState({ contracts: [], testCases: [], discoveryVersion: null });
  t.after(attachExampleRouter());
  assert.equal(store.getState().viewedTestId, null);
  discover();
  assert.equal(store.getState().viewedTestId, testId);
  assert.equal(store.getState().callDraft.signature, second);
  assert.equal(store.getState().runActivity, 'calls');
  assert.equal(store.getState().selectedTestId, null);
  assert.equal(window.history.entries.length, 0);
});

test('a test-only link opens Tests but does not fill or execute its call', t => {
  window.location.hash = `#/examples/std-usage?test=${encodeURIComponent(testId)}`;
  const draft = store.getState().callDraft;
  t.after(attachExampleRouter());
  assert.equal(store.getState().runActivity, 'tests');
  assert.equal(store.getState().viewedTestId, testId);
  assert.deepEqual(store.getState().callDraft, draft);
  assert.equal(store.getState().selectedTestId, null);
});

test('a newer navigation cancels a test selection waiting for discovery', t => {
  window.location.hash = `#/examples/std-usage?test=${encodeURIComponent(testId)}`;
  store.setState({ contracts: [], testCases: [], discoveryVersion: null });
  t.after(attachExampleRouter());
  navigate('#/examples/std-usage?view=calls');
  discover();
  assert.equal(store.getState().runActivity, 'calls');
  assert.equal(store.getState().viewedTestId, null);
});

test('stale view parameters fall back without executing or adding history', t => {
  window.location.hash = '#/examples/std-usage?test=missing&function=gone&view=wrong&tab=gone&file=gone.sol';
  t.after(attachExampleRouter());
  assert.equal(store.getState().viewedTestId, null);
  assert.equal(store.getState().runActivity, 'calls');
  assert.equal(store.getState().outputTab, 'execution');
  assert.equal(store.getState().activePath, 'Calculator.sol');
  assert.equal(store.getState().callDraft.signature, first);
  assert.equal(window.history.entries.length, 0);
});
