/** @param {import('./types').ExecutionResult | null} result */
export function formatExecution(result) {
  if (!result) {
    return 'Press Run to compile and execute main().';
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
