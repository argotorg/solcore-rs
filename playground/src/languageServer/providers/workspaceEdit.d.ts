interface VersionedModel {
  getVersionId(): number;
}

export function workspaceEditTargetsAreCurrent<T>(
  changes: Record<string, T[]>,
  expectedVersions: ReadonlyMap<string, number>,
  getModel: (uri: string) => VersionedModel | null,
): boolean;
