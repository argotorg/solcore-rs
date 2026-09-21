/** @param {import('./types').ExecutionResult | null} result
 * @param {import('./types').TestCase[]} tests */
export function formatExecution(result, tests = []) {
  if (!result) {
    return 'Press Run to execute tests or main().';
  }
  const completed = tests.filter(test => test.status !== 'ready');
  if (completed.length) {
    return [result.message ?? 'Tests', '', ...completed.map(test => {
      const detail = test.message ?? (test.status === 'passed' ? test.actual : `expected ${test.expected}, got ${test.actual}`);
      return `${test.file}:${test.line} ${test.label}\n${test.status === 'passed' ? 'PASS' : 'FAIL'}: ${detail}${test.gasUsed !== null ? ` (gas: ${test.gasUsed})` : ''}`;
    })].join('\n');
  }
  const labels = { success: 'Success', revert: 'Reverted', halt: 'Halted', error: 'Could not run' };
  const lines = [`${labels[result.status]} (${result.phase})`];
  if (result.message) lines.push(result.message);
  if (result.returnWord !== null) lines.push('', `Return word (unsigned): ${result.returnWord}`);
  lines.push('', `${result.status === 'revert' ? 'Revert data' : 'Return data'}: ${result.returnData}`);
  if (result.phase !== 'prepare') {
    lines.push('', `Gas used (${result.phase === 'deploy' ? 'deployment' : 'call'}): ${result.gasUsed}`);
    if (result.deploymentGasUsed !== null) {
      lines.push(`Gas used (deployment): ${result.deploymentGasUsed}`);
    }
    lines.push(`Gas limit per transaction: ${result.gasLimit}`);
  }
  return lines.join('\n');
}
