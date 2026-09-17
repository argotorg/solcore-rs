import assert from 'node:assert/strict';
import test from 'node:test';
import { formatDiagnostic } from './diagnostics.js';

test('copied diagnostics retain the code, locations, labels, notes, and help', () => {
  const range = { file: 'Vaults.sol', startLine: 12, startCol: 5 };
  const text = formatDiagnostic({ severity: 'error', code: 'E1', message: 'Invalid call\n  expected uint256',
    primary: range, labels: [{ range: { ...range, startLine: 3 }, message: 'declared here' }],
    notes: ['argument types must match'], helps: ['pass uint256(1)'],
  });
  assert.equal(text, 'error [E1]: Invalid call\n  expected uint256\n  --> Vaults.sol:12:5\n  Vaults.sol:3:5: declared here\n  note: argument types must match\n  help: pass uint256(1)');
});

test('workspace problems can be copied without a source location', () => {
  assert.equal(formatDiagnostic({ severity: 'error', code: null, message: 'Yul translation failed', primary: null,
    labels: [], notes: [], helps: [],
  }), 'error: Yul translation failed\n  --> workspace');
});
