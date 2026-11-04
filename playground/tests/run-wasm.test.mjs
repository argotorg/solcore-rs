import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { backends, initializeBrowserWasm } from '../../scripts/test-support/browser-wasm.mjs';

// Exercise the actual browser WASM package and the examples shipped by the UI.
const { compile, run, watch } = await initializeBrowserWasm();

// Uncomments the commented-out function `name` in a workshop example.
function uncomment(source, name) {
  const lines = source.split('\n');
  const start = lines.findIndex(line => line.startsWith(`    // function ${name}(`));
  const end = lines.findIndex((line, i) => i > start && line === '    // }');
  assert.ok(start !== -1 && end !== -1, `no commented-out ${name}()`);
  return lines.map((line, i) => (i >= start && i <= end ? line.replace('// ', '') : line)).join('\n');
}

test('browser WASM rejects excessive parser recursion and remains usable', () => {
  for (const [content, expected] of [
    [`pragma solcore; function main() returns (word) { return ${'true ? '.repeat(512)}0${' : 0'.repeat(512)}; }`, /expression nesting/],
    [`pragma solcore; function main() returns (word) { return ${'lam() { return true; }() ? '.repeat(512)}0${' : 0'.repeat(512)}; }`, /expression nesting/],
    [`pragma solcore; type T = ${'Box<'.repeat(4096)}word${'>'.repeat(4096)};`, /type nesting/],
    [`pragma solcore; type T = ${'@'.repeat(4096)}word;`, /type nesting/],
    [`pragma solcore; function main() returns (word) { return ${'('.repeat(80)}42${')'.repeat(80)}; }`, /parse error|expression nesting/],
    [`pragma solcore; function main() { a${'.x'.repeat(4096)} += 1; }`, /expression nesting/],
    [`pragma solcore; function main() { let a = 1${' + 1'.repeat(16384)} }`, /parse error|expression nesting/],
    [`function main() {${'if (true) { ) '.repeat(512)};${'}'.repeat(513)}`, /delimiter nesting/],
  ]) {
    const result = compile({ files: [{ path: 'main.sol', content }], entry: 'main.sol' });
    assert.equal(result.success, false);
    assert.ok(result.diagnostics.some((diagnostic) => expected.test(diagnostic.message)), JSON.stringify(result.diagnostics));
  }
  const content = `pragma solcore; function main() returns (word) { return ${'('.repeat(24)}42${')'.repeat(24)}; }`;
  const result = compile({ files: [{ path: 'main.sol', content }], entry: 'main.sol' });
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
});

test('browser WASM bounds exponential compile-time literal growth', () => {
  const content = `pragma solcore;\n
import * from std;
function grow(comptime n: integer, comptime s: string) returns (comptime<string>) {
  if (integerLt(n, 1)) { return s; }
  else { return grow(integerSub(n, 1), concatLit(s, s)); }
}
contract C {
  function main() returns (word) { return strlenLit(grow(40, "x")); }
}`;
  const result = compile({ files: [{ path: 'main.sol', content }], entry: 'main.sol', options: { emitHull: true } });
  assert.equal(result.success, false);
  assert.ok(result.diagnostics.some((diagnostic) => /fuel|budget|work limit/.test(diagnostic.message)), JSON.stringify(result.diagnostics));
});

for (const backend of backends) {
const options = { emitHull: true, emitYul: true, emitSonatina: true, emitAbi: true, backend };

test(`${backend}: browser WASM runs the bundled Hello contract without changing Compile output`, async () => {
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

test(`${backend}: browser WASM discovers and runs the bundled Calculator test`, async () => {
  const content = await readFile(new URL('../src/examples/std-usage/Calculator.sol', import.meta.url), 'utf8');
  const result = run({ files: [{ path: 'Calculator.sol', content }], entry: 'Calculator.sol', options });
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution?.status, 'success');
  assert.equal(result.tests[0].status, 'passed');
  assert.equal(result.tests[0].actual, '42');
});

test(`${backend}: a changed source is recompiled, and Compile does not execute`, () => {
  const input = {
    files: [{ path: 'main.sol', content: 'pragma solcore; function main() returns (word) { return 99; }' }],
    entry: 'main.sol', options,
  };
  assert.equal(run(input).execution?.returnWord, '99');
  assert.equal(compile(input).execution, null);
});

test(`${backend}: browser WASM reports unsupported structured returns explicitly`, () => {
  const result = run({
    files: [{ path: 'main.sol', content: 'pragma solcore; function main() returns (bool) { return true; }' }],
    entry: 'main.sol', options,
  });
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution?.status, 'error', JSON.stringify(result.execution));
  assert.equal(result.execution.phase, 'prepare');
  assert.match(result.execution.message, /returning one word/);
});

test(`${backend}: browser WASM preserves constructor effects only within each run`, () => {
  const input = {
    files: [{ path: 'main.sol', content: `pragma solcore;\n
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

test(`${backend}: browser WASM replays sends for an individual test and renders booleans`, async () => {
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

test(`${backend}: browser WASM test reruns leave manual contract state untouched`, async () => {
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

test(`${backend}: Composition exports each Yul object and runs the selected vault`, async () => {
  const files = await Promise.all(['Vaults.sol', 'engine.sol'].map(async path => ({
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


test(`${backend}: AMM watches track committed swaps and discard changes from watched calls`, async () => {
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

for (const [directory, entry, helpers, count] of [
  ['std-usage', 'Calculator.sol', [], 4],
  ['option', 'Splitter.sol', ['option.sol'], 4],
  ['generics', 'Registry.sol', [], 6],
  ['pattern-matching', 'Escrow.sol', [], 6],
  ['invariants', 'Amm.sol', ['pool.sol'], 5],
  ['comptime', 'Answer.sol', [], 1],
  ['workshop-1-done', 'Token.sol', [], 5],
  ['workshop-2-done', 'Token.sol', [], 8],
  ['workshop-3-done', 'Token.sol', ['ownable.sol'], 11],
  ['workshop-solution', 'Token.sol', ['events.sol', 'ownable.sol'], 12],
  ['workshop-demo-hooks', 'Token.sol', ['change.sol', 'hooks.sol', 'events.sol', 'ownable.sol'], 14],
  ['workshop-bonus-done', 'Token.sol', ['change.sol', 'hooks.sol', 'events.sol', 'ownable.sol'], 14],
]) {
  test(`${backend}: ${directory} example tests pass together and replay setup for the final check`, async () => {
    const files = await Promise.all([entry, ...helpers].map(async path => ({
      path, content: await readFile(new URL(`../src/examples/${directory}/${path}`, import.meta.url), 'utf8'),
    })));
    const input = { files, entry, options };
    const result = run(input);
    assert.equal(result.success, true, JSON.stringify(result.diagnostics));
    assert.equal(result.tests.length, count);
    assert.ok(result.tests.every(t => t.status === 'passed'), JSON.stringify(result.tests));
    const selected = result.tests.at(-1);
    const replayed = run({ ...input, testId: selected.id });
    assert.equal(replayed.tests.find(t => t.id === selected.id).status, 'passed', JSON.stringify(replayed.tests));
    assert.equal(replayed.tests.find(t => t.id === selected.id).actual, selected.actual);
  });
}

// These starters do not compile until a TODO is done.
for (const [directory, helpers, matches] of [
  ['workshop-2', [], d => d.message === 'non-exhaustive pattern match'
    && d.notes.includes('missing case: Change.Transfer(_, _)')],
  ['workshop-3', ['ownable.sol'], d => d.message === 'undefined type constructor: Ownable'],
]) {
  test(`${backend}: ${directory} starter reports the error its TODO fixes`, async () => {
    const files = await Promise.all(['Token.sol', ...helpers].map(async path => ({
      path, content: await readFile(new URL(`../src/examples/${directory}/${path}`, import.meta.url), 'utf8'),
    })));
    const result = compile({ files, entry: 'Token.sol', options });
    assert.equal(result.success, false);
    assert.ok(result.diagnostics.some(matches), JSON.stringify(result.diagnostics));
  });
}

// Each starter fails exactly the tests that its remaining TODOs fix.
// The step 3 starter gets its import first.
for (const [directory, helpers, count, failing, prepare = source => source] of [
  ['workshop-1', [], 5, [
    '#[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 5000) -> revert]',
    '#[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> 900]',
    '#[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8) -> 100]',
  ]],
  ['workshop-3', ['ownable.sol'], 11, [
    '#[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> revert]',
    '#[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 50) -> revert]',
  ], source => source.replace('import * from std.dispatch;\n', 'import * from std.dispatch;\nimport * from ownable;\n')],
  ['workshop-4', ['events.sol', 'ownable.sol'], 12, [
    '#[() -> 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef]',
  ]],
  ['workshop-bonus', ['change.sol', 'hooks.sol', 'events.sol', 'ownable.sol'], 14, [
    '#[(200) -> revert]',
  ]],
]) {
  test(`${backend}: ${directory} starter fails only the tests of its TODO`, async () => {
    const files = await Promise.all(['Token.sol', ...helpers].map(async path => ({
      path, content: await readFile(new URL(`../src/examples/${directory}/${path}`, import.meta.url), 'utf8'),
    })));
    files[0].content = prepare(files[0].content);
    const result = run({ files, entry: 'Token.sol', options });
    assert.equal(result.success, true, JSON.stringify(result.diagnostics));
    assert.equal(result.tests.length, count);
    assert.deepEqual(result.tests.filter(t => t.status !== 'passed').map(t => t.label), failing);
  });
}

for (const [directory, helpers, name, message] of [
  ['workshop-3-done', ['ownable.sol'], 'steal', 'type name used as callee: `Ownable`'],
  ['workshop-solution', ['events.sol', 'ownable.sol'], 'anyTopic',
    "comptime evaluation failed: runtime value passed to comptime parameter 'signature' of 'topic'"],
]) {
  test(`${backend}: ${directory} rejects the uncommented ${name}()`, async () => {
    const files = await Promise.all(['Token.sol', ...helpers].map(async path => ({
      path, content: await readFile(new URL(`../src/examples/${directory}/${path}`, import.meta.url), 'utf8'),
    })));
    files[0].content = uncomment(files[0].content, name);
    const result = compile({ files, entry: 'Token.sol', options });
    assert.equal(result.success, false);
    assert.ok(result.diagnostics.some(d => d.message === message), JSON.stringify(result.diagnostics));
  });
}


test(`${backend}: bytecode output contains creation and runtime sections without deploying`, () => {
  const input = {
    files: [{ path: 'main.sol', content: 'pragma solcore; contract A { function main() public returns (word) { return 1; } } contract B { function main() public returns (word) { return 2; } }' }],
    entry: 'main.sol', options: { emitBytecode: true, backend },
  };
  const result = compile(input);
  assert.equal(result.success, true, JSON.stringify(result.diagnostics));
  assert.equal(result.execution, null);
  assert.equal(result.sandbox, null);
  assert.deepEqual(result.bytecode.map(object => object.name).sort(), ['ADeploy', 'BDeploy']);
  for (const object of result.bytecode) {
    assert.deepEqual(object.sections.map(section => section.name).sort(), ['init', 'runtime']);
    for (const section of object.sections) assert.match(section.code, /^0x(?:[0-9a-f]{2})+$/);
  }
  const main = compile({ ...input, files: [{ path: 'main.sol', content: 'pragma solcore; function main() returns (word) { return 42; }' }] });
  assert.equal(main.hasMain, true);
  assert.deepEqual(main.bytecode[0].sections.map(section => section.name), ['runtime']);
  assert.equal(compile({ ...input, options: { backend } }).bytecode.length, 0);
});
}
