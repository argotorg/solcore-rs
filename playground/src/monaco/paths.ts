export const MAIN_WORKSPACE_URI_PREFIX = "file:///main/";

export function uriForWorkspacePath(path: string): string {
  return `${MAIN_WORKSPACE_URI_PREFIX}${path
    .split("/")
    .map((part) => encodeURIComponent(part))
    .join("/")}`;
}

export function workspacePathFromUri(uri: string): string | null {
  if (!uri.startsWith(MAIN_WORKSPACE_URI_PREFIX)) {
    return null;
  }

  return uri
    .slice(MAIN_WORKSPACE_URI_PREFIX.length)
    .split("/")
    .map((part) => decodeURIComponent(part))
    .join("/");
}
