import assert from 'node:assert/strict';
import test from 'node:test';
import { backends, initializeBrowserWasm } from '../../scripts/test-support/browser-wasm.mjs';
import { loadProjects } from '../scripts/projects.mjs';

const { compile, run } = await initializeBrowserWasm();
const projects = await loadProjects();

// These counts are part of the lessons' expected results, not inferred from
// the compiler output: losing a directive must fail the teaching contract.
const expectedTests = {
  'std-usage': 4,
  trait: 4,
  generics: 6,
  option: 4,
  'pattern-matching': 6,
  invariants: 5,
  comptime: 1,
  'tutorial-basics': 5,
  'tutorial-counter': 3,
  'tutorial-bounded-counter': 5,
  'tutorial-ticket-office-starter': 2,
  'tutorial-ticket-office': 18,
};
const address = '0x1111111111111111111111111111111111111111';

// These projects have no test comments. Exercise their public
// functions explicitly, including constructors and the advertised guards.
const manualChecks = {
  'contract-output': [
    { contract: 'Hello', signature: 'answer()', decoded: '42' },
  ],
  'mini-nft': [
    { contract: 'MiniNFT', signature: 'exists(uint256)', arguments: [0], decoded: 'false' },
    { contract: 'MiniNFT', signature: 'mint()', decoded: '0' },
    { contract: 'MiniNFT', signature: 'exists(uint256)', arguments: [0], decoded: 'true' },
  ],
  composition: ['VaultDirect', 'VaultPremium', 'VaultGasless'].map(contract => ({
    contract, signature: 'balanceOf(address)', arguments: [address], decoded: '0',
    constructorArguments: contract === 'VaultGasless' ? [2] : [],
  })),
  extensions: [
    { contract: 'Token', signature: 'totalSupply()', decoded: '0' },
    { contract: 'Token', signature: 'pause()' },
    { contract: 'Token', signature: 'paused()', decoded: 'true' },
    { contract: 'Token', signature: 'unpause()' },
    { contract: 'Token', signature: 'paused()', decoded: 'false' },
  ],
  reentrancy: [
    { contract: 'Vault', signature: 'balanceOf(address)', arguments: [address], decoded: '0' },
    { contract: 'Vault', signature: 'reenter()', status: 'revert' },
    { contract: 'Vault', signature: 'readDuringWrite()', status: 'revert' },
    { contract: 'Vault', signature: 'balanceOf(address)', arguments: [address], decoded: '0' },
  ],
};

test('every tutorial catalog project has an execution expectation', () => {
  const expected = [...Object.keys(expectedTests), ...Object.keys(manualChecks)];
  assert.deepEqual(projects.map(project => project.id).sort(), expected.sort());
});

for (const backend of backends) {
  const options = { emitHull: true, emitYul: true, emitSonatina: true, emitAbi: true, backend };

  for (const project of projects) {
    test(`${backend}: tutorial project ${project.id} compiles and checks its execution outcome`, () => {
      const input = { files: project.files, entry: project.entry, options };
      const compiled = compile(input);
      assert.equal(compiled.success, true, JSON.stringify(compiled.diagnostics));
      assert.equal(compiled.execution, null, 'compiling does not execute a lesson');

      if (Object.hasOwn(expectedTests, project.id)) {
        const count = expectedTests[project.id];
        assert.equal(compiled.tests.length, count);
        assert.ok(compiled.tests.every(check => check.status === 'ready'));
        const result = run(input);
        assert.equal(result.success, true, JSON.stringify(result.diagnostics));
        assert.equal(result.execution?.status, 'success', JSON.stringify(result.execution));
        assert.equal(result.tests.length, count);
        assert.ok(result.tests.every(check => check.status === 'passed'), JSON.stringify(result.tests));

        // Running only the final assertion must replay its preceding sends.
        const selected = result.tests.at(-1);
        const replayed = run({ ...input, testId: selected.id });
        const check = replayed.tests.find(candidate => candidate.id === selected.id);
        assert.equal(check?.status, 'passed', JSON.stringify(replayed.tests));
        assert.equal(check.actual, selected.actual);
        return;
      }

      assert.ok(manualChecks[project.id], `missing execution checks for ${project.id}`);
      for (const check of manualChecks[project.id]) {
        const result = run({ ...input, manual: {
          contract: check.contract,
          signature: check.signature,
          arguments: JSON.stringify(check.arguments ?? []),
          constructorArguments: JSON.stringify(check.constructorArguments ?? []),
          simulate: false,
        } });
        assert.equal(result.success, true, JSON.stringify(result.diagnostics));
        assert.equal(result.execution?.status, check.status ?? 'success', JSON.stringify(result.execution));
        if (check.decoded !== undefined) assert.equal(result.execution.decoded, check.decoded);
      }
    });
  }

  test(`${backend}: bounded counter keeps manual state after a rejected increment`, () => {
    const project = projects.find(project => project.id === 'tutorial-bounded-counter');
    assert.ok(project, 'bounded-counter lesson must be present');
    const input = { files: project.files, entry: project.entry, options, sandboxEpoch: 175 };
    const invoke = (signature, args) => run({ ...input, manual: {
      contract: 'BoundedCounter', signature, arguments: JSON.stringify(args), constructorArguments: '[]', simulate: false,
    } }).execution;
    assert.equal(invoke('read()', []).decoded, '0');
    assert.equal(invoke('remaining()', []).decoded, '10');
    assert.equal(invoke('add(uint256)', [4]).status, 'success');
    assert.equal(invoke('read()', []).decoded, '4');
    assert.equal(invoke('remaining()', []).decoded, '6');
    assert.equal(invoke('add(uint256)', [6]).status, 'success');
    assert.equal(invoke('add(uint256)', [1]).status, 'revert');
    assert.equal(invoke('read()', []).decoded, '10');
    assert.equal(invoke('remaining()', []).decoded, '0');
  });
}
