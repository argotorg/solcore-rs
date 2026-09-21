/** @param {import('./types').Diag} diagnostic */
export function formatDiagnostic(diagnostic) {
  const location = range => `${range.file}:${range.startLine}:${range.startCol}`;
  const primary = diagnostic.primary ?? diagnostic.labels[0]?.range;
  const lines = [
    `${diagnostic.severity}${diagnostic.code ? ` [${diagnostic.code}]` : ''}: ${diagnostic.message}`,
    primary ? `  --> ${location(primary)}` : '  --> workspace',
    ...diagnostic.labels.filter(label => label.message).map(label => `  ${location(label.range)}: ${label.message}`),
    ...diagnostic.notes.map(note => `  note: ${note}`),
    ...diagnostic.helps.map(help => `  help: ${help}`),
  ];
  return lines.join('\n');
}
