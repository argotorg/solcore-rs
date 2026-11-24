import type { RecordedCall } from "./types";
import type { Breakpoints, WorkspaceFile } from "../store/filesSlice";
import type { ReplayLocation } from "../store/viewSlice";

type RecordedSource = RecordedCall["program"]["sources"][number];

function sourceLocation(source: RecordedSource, offset: number, length: number): ReplayLocation | null {
  const bytes = new TextEncoder().encode(source.contents);
  if (offset < 0 || length <= 0 || offset + length > bytes.length) return null;
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  try {
    const prefix = decoder.decode(bytes.subarray(0, offset)).split(/\r\n|\n|\r/);
    const before = prefix.at(-1) ?? "";
    const lines = decoder.decode(bytes.subarray(offset, offset + length)).split(/\r\n|\n|\r/);
    return {
      contents: source.contents,
      range: {
        file: source.path,
        startByte: offset,
        endByte: offset + length,
        startLine: prefix.length,
        endLine: prefix.length + lines.length - 1,
        startCol: before.length + 1,
        endCol: (lines.at(-1)?.length ?? 0) + (lines.length === 1 ? before.length : 0) + 1,
      },
    };
  } catch {
    return null;
  }
}

const positions = new WeakMap<RecordedCall, Array<ReplayLocation | null>>();

export function replayPositions(recording: RecordedCall): Array<ReplayLocation | null> {
  const cached = positions.get(recording);
  if (cached) return cached;
  const sources = new Map(recording.program.sources.map((source) => [source.id, source]));
  const sourceLocations = new Map(recording.program.locations);
  const locations = new Map<string, ReplayLocation | null>();
  const sourcesAtStep = recording.steps.map(({ pc }) => {
    const location = sourceLocations.get(pc);
    const source = location && sources.get(location.source_id);
    let position: ReplayLocation | null = null;
    if (source && location) {
      const { offset, length } = location;
      const span = `${source.id}:${offset}:${length}`;
      if (!locations.has(span)) locations.set(span, sourceLocation(source, offset, length));
      position = locations.get(span) ?? null;
    }
    return position;
  });
  positions.set(recording, sourcesAtStep);
  return sourcesAtStep;
}

export function breakpointSteps(
  sources: Array<ReplayLocation | null>,
  breakpoints: Breakpoints,
  files: Record<string, WorkspaceFile>,
): number[] {
  if (!Object.keys(breakpoints).length) return [];
  const hits: number[] = [];
  let previous: ReplayLocation | null = null;
  for (let step = 0; step < sources.length; step++) {
    const source = sources[step];
    if (!source || source.range.file.startsWith("std:")) continue;
    const { file, startLine } = source.range;
    const newLine = previous?.range.file !== file || previous?.range.startLine !== startLine;
    if (newLine && breakpoints[file]?.includes(startLine) && files[file]?.content === source.contents) hits.push(step);
    previous = source;
  }
  return hits;
}
