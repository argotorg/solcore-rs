import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

// Exercise the real entry point, including its dynamic import boundary and Retry.
const bundle = await build({
  entryPoints: [new URL('../main.tsx', import.meta.url).pathname],
  bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'bootstrap-ui', setup(build) {
    build.onResolve({ filter: /\/mount$/ }, () => ({ path: 'mount', namespace: 'mock' }));
    build.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({
      contents: 'globalThis.__editorImports++; export function mount() { globalThis.__editorMounts++; }', loader: 'js',
    }));
    build.onResolve({ filter: /\.css$/ }, () => ({ path: 'style', namespace: 'style' }));
    build.onLoad({ filter: /.*/, namespace: 'style' }, () => ({ contents: '', loader: 'js' }));
  } }],
});

function element(tag) {
  return { tag, children: [], listeners: {}, textContent: '',
    append(...children) { this.children.push(...children); },
    replaceChildren(...children) { this.children = children; },
    setAttribute() {},
    addEventListener(name, callback) { this.listeners[name] = callback; },
  };
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test('bootstrap defers editor initialization, shows a failed load, and retries before mounting', async () => {
  const root = element('root');
  globalThis.document = { createElement: element, getElementById: () => root };
  globalThis.window = { location: new URL('https://example.org/editor/?project=../projects/demo.json') };
  globalThis.__editorImports = 0;
  globalThis.__editorMounts = 0;
  let rejectFetch;
  globalThis.fetch = () => new Promise((_resolve, reject) => { rejectFetch = reject; });
  await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
  assert.equal(root.children[0].children[0].textContent, 'Loading playground…');
  assert.equal(globalThis.__editorImports, 0);
  rejectFetch(new Error('offline'));
  await tick();
  const errorPanel = root.children[0];
  assert.equal(errorPanel.children[0].textContent, 'Could not open the playground');
  assert.match(errorPanel.children[1].textContent, /Could not load the project/);
  const retry = errorPanel.children.find(node => node.tag === 'button');
  assert.equal(retry.textContent, 'Retry');
  assert.equal(globalThis.__editorImports, 0);
  globalThis.fetch = async url => ({ ok: true, url, json: async () => ({
    version: 1, id: 'external', name: 'External', entry: 'Main.sol', files: [{ path: 'Main.sol', content: '' }],
  }) });
  retry.listeners.click();
  await tick();
  assert.equal(globalThis.__editorImports, 1);
  assert.equal(globalThis.__editorMounts, 1);
});
