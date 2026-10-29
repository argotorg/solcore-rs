import assert from 'node:assert/strict';
import test from 'node:test';
import { backends, initializeBrowserWasm } from '../../scripts/test-support/browser-wasm.mjs';
import { loadProjects } from '../scripts/projects.mjs';

const { compile, run } = await initializeBrowserWasm();
const projects = await loadProjects();

function exercise(id, edit, backend) {
  const project = projects.find(candidate => candidate.id === id);
  assert.ok(project, `missing lesson project ${id}`);
  return {
    files: project.files.map(file => file.path === project.entry
      ? { ...file, content: edit(file.content) }
      : file),
    entry: project.entry,
    options: { backend, emitYul: true, emitHull: true, emitAbi: true },
  };
}

for (const backend of backends) {
  test(`${backend}: removing the generic constraint produces the lesson's diagnostic`, () => {
    const input = exercise('generics', source => source.replace(' where a: Ord', ''), backend);
    const result = compile(input);
    assert.equal(result.success, false);
    const diagnostic = result.diagnostics.find(item => item.code === 'SC0207');
    assert.match(diagnostic?.message ?? '', /cannot satisfy trait constraint: a: Ord/);
    assert.equal(diagnostic.primary.file, 'Registry.sol');
  });

  test(`${backend}: a public argument cannot supply a comptime parameter`, () => {
    const input = exercise('comptime', source => source
      .replace('function answer()', 'function answer(value: uint256)')
      .replace('double(uint256(21))', 'double(value)')
      .replace('// #[() -> 42]', '// #[(21) -> 42]'), backend);
    const result = compile(input);
    assert.equal(result.success, false);
    const diagnostic = result.diagnostics.find(item => item.code === 'SC0240');
    assert.match(diagnostic?.message ?? '', /runtime value passed to comptime parameter 'value' of 'double'/);
    assert.equal(diagnostic.primary.file, 'Answer.sol');
    assert.equal(result.execution, null);
  });

  test(`${backend}: the traits exercise toggles both types from their own current state`, () => {
    const input = exercise('trait', source => source.replace(/\n}\s*$/, `
    function flipBoth() public {
        state = Toggle.toggle(state);
        door = Toggle.toggle(door);
    }
}
`), backend);
    const compiled = compile(input);
    assert.equal(compiled.success, true, JSON.stringify(compiled.diagnostics));
    const invoke = signature => {
      const result = run({ ...input, sandboxEpoch: 12807, manual: {
        contract: 'LightSwitch', signature, arguments: '[]', constructorArguments: '[]', simulate: false,
      } });
      assert.equal(result.execution?.status, 'success', JSON.stringify(result));
      return result.execution.decoded;
    };
    invoke('flipBoth()');
    assert.equal(invoke('isOn()'), 'true');
    assert.equal(invoke('isOpen()'), 'true');
    invoke('flipBoth()');
    assert.equal(invoke('isOn()'), 'false');
    assert.equal(invoke('isOpen()'), 'false');
    invoke('flip()');
    invoke('flipBoth()');
    assert.equal(invoke('isOn()'), 'false');
    assert.equal(invoke('isOpen()'), 'true');
  });
}
