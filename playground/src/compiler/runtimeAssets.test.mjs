import assert from 'node:assert/strict';
import test from 'node:test';
import { runtimeAssetUrl } from './runtimeAssets.js';

test('standalone runtime assets follow the editor at any nested hosting path', () => {
  assert.equal(runtimeAssetUrl('./', 'solc/compiler.js', 'https://example.org/classes/solcore/playground/index.html?embed=1&project=../projects/demo.json#/project'),
    'https://example.org/classes/solcore/playground/solc/compiler.js');
  assert.equal(runtimeAssetUrl('./', 'solc/compiler.js', 'https://example.org/playground/'),
    'https://example.org/playground/solc/compiler.js');
});

test('regular builds retain absolute and configured-base runtime asset URLs', () => {
  assert.equal(runtimeAssetUrl('/', 'solc/compiler.js', 'https://example.org/index.html'),
    'https://example.org/solc/compiler.js');
  assert.equal(runtimeAssetUrl('/ide/', 'solc/compiler.js', 'https://example.org/ide/index.html'),
    'https://example.org/ide/solc/compiler.js');
});
