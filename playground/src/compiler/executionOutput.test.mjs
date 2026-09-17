import assert from 'node:assert/strict';
import test from 'node:test';
import { formatExecution } from './executionOutput.js';

const result = {
  status: 'success', phase: 'call', returnData: '0x2a', returnWord: '42',
  gasUsed: 21018, deploymentGasUsed: 60000, gasLimit: 1000000, message: null,
};

test('execution displays return words as strings and separates deployment gas', () => {
  const large = '115792089237316195423570985008687907853269984665640564039457584007913129639935';
  const text = formatExecution({ ...result, returnWord: large });
  assert.ok(text.includes(large));
  assert.doesNotMatch(text, /Osaka|fresh state/i);
  assert.match(text, /Gas used \(call\): 21018/);
  assert.match(text, /Gas used \(deployment\): 60000/);
});

test('reverts retain raw data and do not display a successful return word', () => {
  const text = formatExecution({ ...result, status: 'revert', returnWord: null });
  assert.match(text, /Reverted/);
  assert.match(text, /Revert data: 0x2a/);
  assert.doesNotMatch(text, /Return word/);
});

test('empty execution output prompts a run', () => {
  assert.match(formatExecution(null), /Press Run/);
});
