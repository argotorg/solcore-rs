import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const workers = [];
globalThis.__WatchTestWorker = class {
  listeners = new Map();
  requests = [];
  constructor() { workers.push(this); }
  addEventListener(name, listener) { this.listeners.set(name, listener); }
  removeEventListener(name) { this.listeners.delete(name); }
  postMessage(request) { this.requests.push(request); }
  terminate() { }
  reply(data) { this.listeners.get('message')({ data }); }
};
const bundle = await build({
  entryPoints: [new URL('./compileClient.ts', import.meta.url).pathname],
  bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'worker', setup(build) {
    build.onResolve({ filter: /compile\.worker\?worker$/ }, () => ({ path: 'worker', namespace: 'mock' }));
    build.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({ contents: 'export default globalThis.__WatchTestWorker;' }));
  } }],
});
const { CompileClient } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

test('watch refreshes supersede one another without cancelling Run', async () => {
  const client = new CompileClient();
  const worker = workers.at(-1);
  const run = client.run({});
  const first = client.watch({});
  const rejected = assert.rejects(first, { name: 'AbortError' });
  const latest = client.watch({});
  await rejected;
  worker.reply({ id: worker.requests[1].id, kind: 'watch-result', result: [{ value: 'old' }] });
  worker.reply({ id: worker.requests[2].id, kind: 'watch-result', result: [{ value: '42' }] });
  assert.deepEqual(await latest, [{ value: '42' }]);
  worker.reply({ id: worker.requests[0].id, kind: 'result', result: { success: true } });
  assert.deepEqual(await run, { success: true });
  const pending = client.watch({});
  const terminated = assert.rejects(pending, { name: 'AbortError' });
  client.terminate();
  await terminated;
});
