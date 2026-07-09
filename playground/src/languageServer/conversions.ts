import type * as Monaco from "monaco-editor";
import type { LspLocation, LspPosition, LspRange } from "./protocol";

export interface MonacoLocation {
  uri: Monaco.Uri;
  range: Monaco.IRange;
}

export function toLspPosition(position: Monaco.IPosition): LspPosition {
  return {
    line: Math.max(0, position.lineNumber - 1),
    character: Math.max(0, position.column - 1),
  };
}

export function fromLspPosition(
  monaco: typeof Monaco,
  position: LspPosition,
): Monaco.IPosition {
  return new monaco.Position(position.line + 1, position.character + 1);
}

export function toLspRange(range: Monaco.IRange): LspRange {
  return {
    start: {
      line: Math.max(0, range.startLineNumber - 1),
      character: Math.max(0, range.startColumn - 1),
    },
    end: {
      line: Math.max(0, range.endLineNumber - 1),
      character: Math.max(0, range.endColumn - 1),
    },
  };
}

export function fromLspRange(monaco: typeof Monaco, range: LspRange): Monaco.IRange {
  return new monaco.Range(
    range.start.line + 1,
    range.start.character + 1,
    range.end.line + 1,
    range.end.character + 1,
  );
}

export function fromLspLocation(
  monaco: typeof Monaco,
  location: LspLocation,
): MonacoLocation {
  return {
    uri: monaco.Uri.parse(location.uri),
    range: fromLspRange(monaco, location.range),
  };
}
