export interface ProjectManifest {
  version: 1;
  id: string;
  name: string;
  description: string;
  entry: string;
  files: Array<{ path: string; content: string }>;
}
export interface LoadedProject { url: string; manifest: ProjectManifest }
export declare function validateProjectManifest(value: unknown): ProjectManifest;
export declare function resolveProjectUrl(reference: string | null, documentUrl: string): string;
export declare function loadProject(documentUrl: string, fetcher?: typeof fetch): Promise<LoadedProject | null>;
