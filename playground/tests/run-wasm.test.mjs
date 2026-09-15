import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import init, { compile, run } from 'solcore-wasm';

// Exercise the actual browser WASM package and the examples shipped by the UI.
await init({
  module_or_path: await readFile(new URL('../../crates/wasm/pkg/solcore_wasm_bg.wasm', import.meta.url)),
});
const options = { emitHull: true, emitYul: true, emitSonatina: true, emitAbi: true };

test('browser WASM runs the bundled Hello contract without changing Compile output', async () => {
  const content = await readFile(new URL('../src/examples/contract-output/Hello.sol', import.meta.url), 'utf8');
  const input = { files: [{ path: 'Hello.sol', content }], entry: 'Hello.sol', options };
  const compiled = compile(input);
  const result = run(input);
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution?.status, 'success', JSON.stringify(result.execution));
  assert.equal(result.execution.returnWord, '42');
  assert.ok(result.execution.gasUsed > 21000);
  assert.deepEqual({ ...result, execution: null }, compiled);
});

test('browser WASM reports that the bundled Calculator requires a main entry', async () => {
  const content = await readFile(new URL('../src/examples/std-usage/Calculator.sol', import.meta.url), 'utf8');
  const result = run({ files: [{ path: 'Calculator.sol', content }], entry: 'Calculator.sol', options });
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution?.status, 'error');
  assert.match(result.execution.message, /main/);
});

test('a changed source is recompiled, and Compile does not execute', () => {
  const input = {
    files: [{ path: 'main.sol', content: 'function main() returns (word) { return 99; }' }],
    entry: 'main.sol', options,
  };
  assert.equal(run(input).execution?.returnWord, '99');
  assert.equal(compile(input).execution, null);
});

test('browser WASM reports unsupported structured returns explicitly', () => {
  const result = run({
    files: [{ path: 'main.sol', content: 'function main() returns (bool) { return true; }' }],
    entry: 'main.sol', options,
  });
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution?.status, 'error', JSON.stringify(result.execution));
  assert.equal(result.execution.phase, 'prepare');
  assert.match(result.execution.message, /returning one word/);
});

test('browser WASM preserves constructor effects only within each run', () => {
  const input = {
    files: [{ path: 'main.sol', content: `
import * from std;
import * from std.dispatch;
contract Counter {
  stored: uint256;
  constructor() { stored = uint256(7); }
  function main() public returns (uint256) {
    stored = stored + uint256(1);
    return stored;
  }
}
` }],
    entry: 'main.sol', options,
  };
  for (let i = 0; i < 2; i++) {
    const result = run(input);
    assert.equal(result.success, true, JSON.stringify(result.diagnostics));
    assert.equal(result.execution?.status, 'success', JSON.stringify(result.execution));
    assert.equal(result.execution.returnWord, '8');
    assert.ok(result.execution.deploymentGasUsed > 0);
  }
});
