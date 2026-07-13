export interface LspMarkupContent {
  kind: string;
  value: string;
}

export interface LspMarkedString {
  language: string;
  value: string;
}

export type LspHoverContent = LspMarkedString | LspMarkupContent | string;

export function markdownForHoverContents(
  contents: LspHoverContent | LspHoverContent[],
): Array<{ value: string }>;
