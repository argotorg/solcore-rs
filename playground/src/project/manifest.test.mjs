import assert from 'node:assert/strict';
import test from 'node:test';
import { loadProject, resolveProjectUrl, validateProjectManifest } from './loadProject.js';

const manifest = { version: 1, id: 'unregistered', name: 'External project', entry: 'app/Main.sol',
  files: [{ path: 'app/Main.sol', content: 'function main() returns (word) { return 42; }' }] };
const page = 'https://example.org/courses/editor/index.html?project=../projects/demo.json';

test('a valid manifest needs no built-in example registration', () => {
  assert.deepEqual(validateProjectManifest(manifest), { ...manifest, description: '' });
  assert.equal(resolveProjectUrl('../projects/./demo.json#ignored', page), 'https://example.org/courses/projects/demo.json');
});

test('manifest validation rejects unsafe paths, missing entries, duplicates, and invalid schema', () => {
  for (const path of ['/Main.sol', '../Main.sol', 'a/../Main.sol', 'a//Main.sol', './Main.sol', 'a\\Main.sol', 'C:/Main.sol', '__proto__', 'a/ constructor', 'Main.sol\0']) {
    assert.throws(() => validateProjectManifest({ ...manifest, entry: path, files: [{ path, content: '' }] }), /path|file/);
  }
  for (const value of [null, [], {}, { ...manifest, version: 2 }, { ...manifest, id: '' }, { ...manifest, name: 3 },
    { ...manifest, description: false }, { ...manifest, files: [] }, { ...manifest, files: [{ path: 'app/Main.sol', content: 42 }] },
    { ...manifest, entry: 'Missing.sol' }, { ...manifest, files: [...manifest.files, ...manifest.files] }]) {
    assert.throws(() => validateProjectManifest(value));
  }
});

test('project fetch is same-origin HTTP(S), cannot follow redirects, and validates responses', async () => {
  const calls = [];
  const project = await loadProject(page, async (url, options) => {
    calls.push({ url, options });
    return { ok: true, url, json: async () => manifest };
  });
  assert.equal(project.url, 'https://example.org/courses/projects/demo.json');
  assert.deepEqual(calls[0].options, { credentials: 'same-origin', redirect: 'error' });
  for (const reference of ['https://other.test/project.json', 'data:application/json,{}', 'file:///tmp/project.json', '//other.test/p.json', 'https://user:password@example.org/p.json', '']) {
    await assert.rejects(loadProject(`https://example.org/?project=${encodeURIComponent(reference)}`, () => { throw new Error('must not fetch'); }));
  }
  await assert.rejects(loadProject(page, async () => ({ ok: true, url: 'https://other.test/p.json' })), /another origin/);
  await assert.rejects(loadProject(page, async () => ({ ok: false, status: 404 })), /HTTP 404/);
  await assert.rejects(loadProject(page, async () => { throw new Error('network'); }), /Could not load/);
  await assert.rejects(loadProject(page, async () => ({ ok: true, json: async () => { throw new Error('HTML'); } })), /valid JSON/);
  await assert.rejects(loadProject(page, async () => ({ ok: true, json: async () => ({ ...manifest, version: 2 }) })), /version 1/);
  assert.equal(await loadProject('https://example.org/', () => { throw new Error('must not fetch'); }), null);
});
