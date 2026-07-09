export function nowMs(): number {
  return typeof performance === "undefined" ? Date.now() : performance.now();
}

export function formatCompileDuration(durationMs: number): string {
  if (!Number.isFinite(durationMs) || durationMs < 0) {
    return "0 ms";
  }

  if (durationMs < 1) {
    return "<1 ms";
  }

  if (durationMs < 1000) {
    return `${Math.round(durationMs)} ms`;
  }

  if (durationMs < 10_000) {
    return `${(durationMs / 1000).toFixed(2)} s`;
  }

  if (durationMs < 60_000) {
    return `${(durationMs / 1000).toFixed(1)} s`;
  }

  const minutes = Math.floor(durationMs / 60_000);
  const seconds = Math.round((durationMs % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}
