export interface SolcBinding {
  version: string;
  compile(input: string): string;
}

export function bindSolc(soljson: unknown): SolcBinding;
