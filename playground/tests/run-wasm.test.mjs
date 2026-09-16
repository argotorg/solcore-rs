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

test('browser WASM discovers and runs the bundled Calculator test', async () => {
  const content = await readFile(new URL('../src/examples/std-usage/Calculator.sol', import.meta.url), 'utf8');
  const result = run({ files: [{ path: 'Calculator.sol', content }], entry: 'Calculator.sol', options });
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution?.status, 'success');
  assert.equal(result.tests[0].status, 'passed');
  assert.equal(result.tests[0].actual, '42');
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

test('browser WASM replays sends for an individual test and renders booleans', async () => {
  const content = await readFile(new URL('../src/examples/trait/LightSwitch.sol', import.meta.url), 'utf8');
  const input = { files: [{ path: 'LightSwitch.sol', content }], entry: 'LightSwitch.sol', options };
  const compiled = compile(input);
  assert.equal(compiled.tests.length, 2);
  assert.ok(compiled.tests.every(test => test.status === 'ready'));
  const result = run({ ...input, testId: compiled.tests[1].id });
  assert.equal(result.execution?.status, 'success', JSON.stringify(result));
  assert.equal(result.tests[0].status, 'ready');
  assert.equal(result.tests[1].status, 'passed');
  assert.equal(result.tests[1].actual, 'true');
  assert.equal(result.tests[1].expected, 'true');
});

test('browser WASM shares a test deployment with editable manual calls', async () => {
  const content = await readFile(new URL('../src/examples/std-usage/Calculator.sol', import.meta.url), 'utf8');
  const input = { files: [{ path: 'Calculator.sol', content }], entry: 'Calculator.sol', options };
  const discovered = compile(input);
  const testcase = discovered.tests[0];
  assert.deepEqual(JSON.parse(testcase.invocation.arguments), ['20', '22']);
  const tested = run({ ...input, testId: testcase.id });
  assert.equal(tested.tests[0].status, 'passed');
  assert.equal(tested.sandbox.contract, testcase.contract);
  const manual = run({ ...input, manual: {
    contract: testcase.contract, ...testcase.invocation,
    constructorArguments: '[]', arguments: '[30,12]',
  } });
  assert.equal(manual.execution.status, 'success');
  assert.equal(manual.execution.decoded, '42');
  assert.equal(manual.sandbox.address, tested.sandbox.address);
});

test('Composition exports each Yul object and runs the selected vault', async () => {
  const files = await Promise.all(['Vaults.sol', 'context.sol', 'engine.sol'].map(async path => ({
    path, content: await readFile(new URL(`../src/examples/composition/${path}`, import.meta.url), 'utf8'),
  })));
  const input = { files, entry: 'Vaults.sol', options };
  const compiled = compile(input);
  assert.equal(compiled.success, true, JSON.stringify(compiled.diagnostics));
  assert.deepEqual(compiled.yulOutputs.map(output => output.name).sort(), ['VaultDirectDeploy', 'VaultGaslessDeploy', 'VaultPremiumDeploy']);
  for (const output of compiled.yulOutputs) assert.ok(output.code.startsWith(`object "${output.name}"`));
  for (const contract of ['VaultDirect', 'VaultPremium', 'VaultGasless']) {
    const result = run({ ...input, manual: {
      contract, signature: 'balanceOf(address)', arguments: '["0x1111111111111111111111111111111111111111"]',
      constructorArguments: contract === 'VaultGasless' ? '[2]' : '[]', simulate: true,
    } });
    assert.equal(result.success, true, JSON.stringify(result.diagnostics));
    assert.equal(result.execution.status, 'success', JSON.stringify(result.execution));
    assert.equal(result.execution.decoded, '0');
    assert.equal(result.sandbox.contract, contract);
  }
});
