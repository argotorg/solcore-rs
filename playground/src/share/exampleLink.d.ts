export declare const EXAMPLE_PARAM: string;

export function readSharedExampleId(search: string): string | null;

export interface ExampleView {
  file?: string;
  selection?: string;
  tab?: string;
  view?: string;
  contract?: string;
  function?: string;
  test?: string;
}
export function buildExampleLink(href: string, id: string, view?: ExampleView): string;
export function readExampleView(hash: string): ExampleView;

export function readExampleRoute(hash: string): string | null;
