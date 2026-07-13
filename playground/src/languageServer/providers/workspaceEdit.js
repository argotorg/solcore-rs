export function workspaceEditTargetsAreCurrent(
  changes,
  expectedVersions,
  getModel,
) {
  let hasEdits = false;
  for (const [uri, edits] of Object.entries(changes)) {
    if (edits.length === 0) {
      continue;
    }
    hasEdits = true;
    const model = getModel(uri);
    if (
      !model ||
      expectedVersions.get(uri) !== model.getVersionId()
    ) {
      return false;
    }
  }
  return hasEdits;
}
