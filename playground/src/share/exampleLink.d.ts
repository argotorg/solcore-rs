export declare const EXAMPLE_PARAM: string;
export declare const NAVIGATION_FIELDS: readonly ["file", "tab", "view", "contract", "function", "test", "backend"];

export function readSharedExampleId(search: string): string | null;

export interface ExampleView {
  file?: string;
  selection?: string;
  tab?: string;
  view?: string;
  contract?: string;
  function?: string;
  test?: string;
  backend?: string;
}
export interface ExampleLocation {
  id: string;
  view: ExampleView;
}

export function readExampleLocation(hash: string): ExampleLocation | null;
export function buildExampleLink(href: string, id: string, view?: ExampleView): string;
export function readExampleView(hash: string): ExampleView;

export function readExampleRoute(hash: string): string | null;
