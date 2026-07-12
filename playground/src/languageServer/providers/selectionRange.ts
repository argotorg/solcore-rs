import type * as Monaco from "monaco-editor";
import { SOLCORE_LANGUAGE_ID } from "../../monaco/solc-language";
import { fromLspRange, toLspPosition } from "../conversions";
import type { LspClient } from "../lspClient";
import type { LspPosition, LspRange } from "../protocol";

const CANCELLED = Symbol("selection range request cancelled");
const MAX_CHAIN_LENGTH = 10_000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isLspPosition(value: unknown): value is LspPosition {
  return (
    isRecord(value) &&
    isNonNegativeInteger(value.line) &&
    isNonNegativeInteger(value.character)
  );
}

function isLspRange(value: unknown): value is LspRange {
  return isRecord(value) && isLspPosition(value.start) && isLspPosition(value.end);
}

function comparePositions(left: LspPosition, right: LspPosition): number {
  return left.line - right.line || left.character - right.character;
}

function containsPosition(range: LspRange, position: LspPosition): boolean {
  return (
    comparePositions(range.start, position) <= 0 &&
    comparePositions(position, range.end) <= 0
  );
}

function containsRange(outer: LspRange, inner: LspRange): boolean {
  return (
    comparePositions(outer.start, inner.start) <= 0 &&
    comparePositions(inner.end, outer.end) <= 0
  );
}

function equalRanges(left: LspRange, right: LspRange): boolean {
  return (
    comparePositions(left.start, right.start) === 0 &&
    comparePositions(left.end, right.end) === 0
  );
}

function isRangeInModel(model: Monaco.editor.ITextModel, range: LspRange): boolean {
  if (comparePositions(range.start, range.end) > 0) {
    return false;
  }

  const lineCount = model.getLineCount();
  for (const position of [range.start, range.end]) {
    if (
      position.line >= lineCount ||
      position.character > model.getLineLength(position.line + 1)
    ) {
      return false;
    }
  }

  return true;
}

function flattenSelectionRange(
  monaco: typeof Monaco,
  model: Monaco.editor.ITextModel,
  value: unknown,
  requestedPosition: LspPosition,
): Monaco.languages.SelectionRange[] | null {
  const ranges: Monaco.languages.SelectionRange[] = [];
  const visited = new Set<unknown>();
  let current: unknown = value;
  let previous: LspRange | undefined;

  while (current !== undefined && current !== null) {
    if (
      !isRecord(current) ||
      visited.has(current) ||
      ranges.length >= MAX_CHAIN_LENGTH
    ) {
      return null;
    }
    visited.add(current);

    if (!isLspRange(current.range) || !isRangeInModel(model, current.range)) {
      return null;
    }

    const range = current.range;
    if (
      !containsPosition(range, requestedPosition) ||
      (previous !== undefined &&
        (!containsRange(range, previous) || equalRanges(range, previous)))
    ) {
      return null;
    }

    ranges.push({ range: fromLspRange(monaco, range) });
    previous = range;
    current = current.parent;
  }

  return ranges.length > 0 ? ranges : null;
}

export function registerSelectionRange(
  monaco: typeof Monaco,
  client: LspClient,
): Monaco.IDisposable {
  return monaco.languages.registerSelectionRangeProvider(SOLCORE_LANGUAGE_ID, {
    async provideSelectionRanges(model, positions, token) {
      if (token.isCancellationRequested) {
        return null;
      }

      let cancelRequest: (() => void) | undefined;
      const cancellation = new Promise<typeof CANCELLED>((resolve) => {
        cancelRequest = () => resolve(CANCELLED);
      });
      const cancellationListener = token.onCancellationRequested(() => cancelRequest?.());

      try {
        if (token.isCancellationRequested) {
          return null;
        }

        const requestedPositions = positions.map(toLspPosition);
        const response = await Promise.race([
          client.request<unknown>("textDocument/selectionRange", {
            textDocument: {
              uri: model.uri.toString(),
            },
            positions: requestedPositions,
          }),
          cancellation,
        ]);

        if (response === CANCELLED || token.isCancellationRequested) {
          return null;
        }
        if (!Array.isArray(response) || response.length !== requestedPositions.length) {
          return null;
        }

        const result: Monaco.languages.SelectionRange[][] = [];
        for (let index = 0; index < response.length; index += 1) {
          const chain = flattenSelectionRange(
            monaco,
            model,
            response[index],
            requestedPositions[index],
          );
          if (chain === null) {
            return null;
          }
          result.push(chain);
        }

        return result;
      } catch {
        return null;
      } finally {
        cancellationListener.dispose();
        cancelRequest = undefined;
      }
    },
  });
}
