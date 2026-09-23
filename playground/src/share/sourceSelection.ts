export interface SourceSelection {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
}

// One-based positions, with an exclusive end: 12, 12:4, or 12:4-15:8.
export function parseSourceSelection(value?: string): SourceSelection | null {
  const match = /^(\d+)(?::(\d+))?(?:-(\d+)(?::(\d+))?)?$/.exec(value ?? "");
  if (!match) return null;
  const startLineNumber = Number(match[1]);
  const startColumn = Number(match[2] ?? 1);
  const endLineNumber = Number(match[3] ?? match[1]);
  const endColumn = Number(match[4] ?? (match[3] ? 1 : startColumn));
  if (![startLineNumber, startColumn, endLineNumber, endColumn].every(n => Number.isSafeInteger(n) && n > 0) ||
      endLineNumber < startLineNumber || (endLineNumber === startLineNumber && endColumn < startColumn)) return null;
  return { startLineNumber, startColumn, endLineNumber, endColumn };
}

export function formatSourceSelection(range: SourceSelection): string {
  const start = `${range.startLineNumber}:${range.startColumn}`;
  return range.startLineNumber === range.endLineNumber && range.startColumn === range.endColumn
    ? start : `${start}-${range.endLineNumber}:${range.endColumn}`;
}
