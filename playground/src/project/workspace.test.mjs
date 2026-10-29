import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

// Match browser bootstrap ordering: project loading completes before the real
// store/router modules are imported. Only fetch, browser APIs, and the worker
// are replaced, so failed loads cannot accidentally initialize persistence.
const bundle = await build({
  stdin: {
    contents: `import { prepareProject } from '../project/context';
      await prepareProject(window.location.href, globalThis.__projectFetch);
      export const { useWorkspaceStore: store } = await import('../store/workspace');
      export const { attachExampleRouter } = await import('../share/useExampleRouter');
      export const { embedded, project, workspaceStorage } = await import('../store/session');
      export const { backupStoredWorkspace } = await import('../store/filesSlice');`,
    resolveDir: new URL('.', import.meta.url).pathname, loader: 'ts',
  },
  bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'project-workspace-environment', setup(build) {
    build.onResolve({ filter: /\.sol\?raw$/ }, ({ path, resolveDir }) => ({
      path: new URL(path, 'file://' + resolveDir + '/').pathname, namespace: 'raw',
    }));
    build.onLoad({ filter: /.*/, namespace: 'raw' }, async ({ path }) => ({
      contents: await readFile(path.replace(/\?raw$/, ''), 'utf8'), loader: 'text',
    }));
    build.onResolve({ filter: /compiler\/compileClient$/ }, () => ({ path: 'worker', namespace: 'mock' }));
    build.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({
      contents: `export const compileClient = new Proxy({}, { get() {
        return () => { throw new Error('Project loading and navigation must not execute code'); };
      }});`, loader: 'js',
    }));
  } }],
});
const moduleUrl = `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`;
let navigation = 0;
const manifest = { version: 1, id: 'external-project', name: 'External project', entry: 'Main.sol',
  files: [{ path: 'Main.sol', content: '// original external source' }, { path: 'lib/helper.sol', content: '// helper' }] };

async function open(search, hash, storage, source = manifest) {
  globalThis.document = { documentElement: { dataset: {} } };
  globalThis.__projectFetch = async (url) => {
    if (source instanceof Error) throw source;
    return { ok: true, url, json: async () => source };
  };
  globalThis.window = Object.assign(new EventTarget(), {
    location: new URL(`https://example.org/book/playground/index.html${search}${hash}`),
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) },
    matchMedia: () => ({ matches: false }),
    history: {
      entries: [],
      pushState(_state, _title, href) { this.entries.push(href); window.location = new URL(href); },
      replaceState(_state, _title, href) { window.location = new URL(href); },
    },
  });
  return import(`${moduleUrl}#${navigation++}`);
}

test('arbitrary projects save by canonical URL, isolate normal/other projects, and reset to manifest files', async () => {
  const storage = new Map();
  const normal = await open('', '#/examples/contract-output', storage);
  normal.store.getState().setContent('Hello.sol', '// personal scratch work');
  normal.backupStoredWorkspace();
  const original = storage.get(normal.workspaceStorage.key);
  const originalBackup = storage.get(normal.workspaceStorage.backupKey);

  const first = await open('?embed=1&project=../projects/one.json', '#/project?tab=run&view=tests', storage);
  assert.equal(first.embedded, true);
  assert.equal(first.store.getState().exampleId, 'external-project');
  first.store.getState().setContent('Main.sol', '// changed project');
  first.backupStoredWorkspace();
  assert.equal(JSON.parse(storage.get(first.workspaceStorage.backupKey)).files[0].content, '// changed project');

  const second = await open('?embed=1&project=../projects/two.json', '#/project', storage);
  second.store.getState().setContent('Main.sol', '// another project');
  assert.notEqual(second.workspaceStorage.key, first.workspaceStorage.key, 'identical manifest ids do not share storage');
  assert.equal(storage.get(normal.workspaceStorage.key), original);
  assert.equal(storage.get(normal.workspaceStorage.backupKey), originalBackup);

  const fullTab = await open('?project=https://example.org/book/projects/./one.json', '#/project?tab=run&view=tests', storage);
  assert.equal(fullTab.embedded, false);
  assert.equal(fullTab.store.getState().files['Main.sol'].content, '// changed project');
  assert.equal(fullTab.workspaceStorage.key, first.workspaceStorage.key);
  fullTab.store.getState().resetWorkspace();
  assert.equal(fullTab.store.getState().exampleId, manifest.id);
  assert.equal(fullTab.store.getState().files['Main.sol'].content, '// original external source');
  assert.equal(storage.get(normal.workspaceStorage.key), original);
  const restored = await open('?project=../projects/two.json', '#/project', storage);
  assert.equal(restored.store.getState().files['Main.sol'].content, '// another project');
});

test('a project id colliding with a built-in example never loads built-in files', async () => {
  const external = { ...manifest, id: 'contract-output' };
  const { store, attachExampleRouter } = await open('?project=../projects/collision.json', '#/examples/contract-output', new Map(), external);
  const detach = attachExampleRouter();
  try {
    assert.equal(window.location.hash, '#/project');
    assert.equal(store.getState().files['Hello.sol'], undefined);
    store.getState().loadExample('trait');
    assert.equal(store.getState().exampleId, external.id);
    store.getState().resetWorkspace();
    assert.equal(store.getState().files['Main.sol'].content, external.files[0].content);
  } finally { detach(); }
});

test('project view links restore files, tests, backend, and history without running code', async () => {
  const { store, attachExampleRouter } = await open('?embed=1&project=../projects/one.json', '#/project?file=lib%2Fhelper.sol&tab=run&view=tests&backend=sonatina', new Map());
  const detach = attachExampleRouter();
  try {
    assert.equal(store.getState().activePath, 'lib/helper.sol');
    assert.equal(store.getState().runActivity, 'tests');
    assert.equal(store.getState().options.backend, 'sonatina');
    store.getState().setActive('Main.sol');
    store.getState().setRunActivity('calls');
    const href = window.location.href;
    assert.equal(new URL(href).searchParams.get('project'), '../projects/one.json');
    assert.equal(new URL(href).searchParams.get('embed'), '1');
    assert.match(new URL(href).hash, /^#\/project\?/);
    store.getState().setRunActivity('tests');
    const count = window.history.entries.length;
    window.location = new URL(href);
    window.dispatchEvent(new Event('hashchange'));
    assert.equal(store.getState().runActivity, 'calls');
    assert.equal(window.history.entries.length, count);
  } finally { detach(); }
});

test('failed or malformed external projects never initialize normal workspace storage', async () => {
  let reads = 0;
  let writes = 0;
  const storage = { get() { reads++; return null; }, set() { writes++; } };
  for (const source of [new Error('offline'), { ...manifest, entry: '../outside.sol' }]) {
    await assert.rejects(open('?project=../projects/broken.json', '#/project', storage, source));
  }
  assert.equal(reads, 0);
  assert.equal(writes, 0);
});
