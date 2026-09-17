import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import init, { compile, run, watch } from 'solcore-wasm';

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
  assert.equal(result.tests[0].status, 'passed');
  assert.equal(result.tests[0].replayed, true);
  assert.deepEqual(result.events.map(e => e.kind), ['deploy', 'setup', 'check']);
  assert.equal(result.tests[1].status, 'passed');
  assert.equal(result.tests[1].actual, 'true');
  assert.equal(result.tests[1].expected, 'true');
});

test('browser WASM test reruns leave manual contract state untouched', async () => {
  const content = await readFile(new URL('../src/examples/trait/LightSwitch.sol', import.meta.url), 'utf8');
  const input = { files: [{ path: 'LightSwitch.sol', content }], entry: 'LightSwitch.sol', options, sandboxEpoch: 174 };
  const testcase = compile(input).tests[1];
  const invoke = () => run({ ...input, manual: { contract: 'LightSwitch', signature: 'isOn()',
    arguments: '[]', constructorArguments: '[]', simulate: true } });
  assert.equal(run({ ...input, testId: testcase.id }).sandbox, null, 'tests must not create a manual session');
  const initial = invoke();
  assert.equal(initial.execution.decoded, 'false');
  for (const testId of [testcase.id, undefined, testcase.id]) {
    const tested = run({ ...input, testId });
    assert.equal(tested.tests[1].actual, 'true');
    assert.equal(tested.sandbox.id, initial.sandbox.id);
    const manual = invoke();
    assert.equal(manual.execution.decoded, 'false');
    assert.equal(manual.sandbox.id, initial.sandbox.id);
    assert.deepEqual(manual.events.map(e => e.kind), ['call']);
  }
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


test('AMM watches track committed swaps and discard changes from watched calls', async () => {
  const files = await Promise.all(['Amm.sol', 'pool.sol'].map(async path => ({
    path, content: await readFile(new URL(`../src/examples/invariants/${path}`, import.meta.url), 'utf8'),
  })));
  const input = { files, entry: 'Amm.sol', options };
  const invoke = (signature, args, simulate) => run({ ...input, manual: {
    contract: 'Amm', signature, arguments: args, constructorArguments: '[]', simulate,
  } });
  const watches = ['poolX()', 'poolY()'].map(signature => ({ id: signature, contract: 'Amm', signature, arguments: '[]' }));
  const values = () => watch({ workspace: input, watches }).map(result => result.value);
  assert.equal(invoke('poolX()', '[]', true).execution.decoded, '10');
  assert.deepEqual(values(), ['10', '1000']);
  const inspected = watch({ workspace: input, watches: [
    { id: 'swap', contract: 'Amm', signature: 'swap(uint256)', arguments: '[10]' }, ...watches,
  ] });
  assert.deepEqual(inspected.map(result => result.value), ['500', '10', '1000']);
  assert.equal(invoke('swap(uint256)', '[10]', false).execution.decoded, '500');
  assert.deepEqual(values(), ['20', '500']);
  assert.equal(invoke('swap(uint256)', '[10]', true).execution.decoded, '166');
  assert.deepEqual(values(), ['20', '500']);
  invoke('swap(uint256)', '[10]', false);
  assert.deepEqual(values(), ['30', '334']);
  const stale = watch({ workspace: { ...input, sandboxEpoch: 1 }, watches });
  assert.ok(stale.every(result => result.error && result.value === null));
});
