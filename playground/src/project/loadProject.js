function record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nonempty(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function validPath(path) {
  return nonempty(path) && !/[\\\x00-\x1f\x7f:#?%]/.test(path)
    && path.split("/").every((part) => part.length > 0 && part.trim() === part
      && ![".", "..", "__proto__", "prototype", "constructor"].includes(part));
}

/** Validate a self-contained project; source paths are relative virtual files. */
export function validateProjectManifest(value) {
  if (!record(value) || value.version !== 1) throw new Error("Unsupported project format: expected version 1.");
  if (!nonempty(value.id) || !nonempty(value.name)) throw new Error("The project needs a nonempty id and name.");
  if (value.description !== undefined && typeof value.description !== "string") throw new Error("The project description must be text.");
  if (!validPath(value.entry)) throw new Error("The project entry must be a relative file path.");
  if (!Array.isArray(value.files) || value.files.length === 0) throw new Error("The project needs at least one source file.");
  const paths = new Set();
  const files = value.files.map((file) => {
    if (!record(file) || !validPath(file.path) || typeof file.content !== "string") {
      throw new Error("Each project file needs a valid relative path and text content.");
    }
    if (paths.has(file.path)) throw new Error(`Duplicate project file: ${file.path}`);
    paths.add(file.path);
    return { path: file.path, content: file.content };
  });
  if (!paths.has(value.entry)) throw new Error("The project entry is not present in its files.");
  return { version: 1, id: value.id, name: value.name, description: value.description ?? "", entry: value.entry, files };
}

export function resolveProjectUrl(reference, documentUrl) {
  if (!nonempty(reference)) throw new Error("The project URL is empty.");
  const page = new URL(documentUrl);
  const url = new URL(reference, page);
  if (!["http:", "https:"].includes(url.protocol) || url.origin !== page.origin || url.username || url.password) {
    throw new Error("Projects must be loaded from an HTTP(S) URL on the same origin as the playground.");
  }
  url.hash = "";
  return url.href;
}

/** Fetch before initializing the workspace so a failed project cannot overwrite it. */
export async function loadProject(documentUrl, fetcher = fetch) {
  const page = new URL(documentUrl);
  if (!page.searchParams.has("project")) return null;
  const url = resolveProjectUrl(page.searchParams.get("project"), page.href);
  let response;
  try {
    // Do not follow a redirect that could send a project request off-origin.
    response = await fetcher(url, { credentials: "same-origin", redirect: "error" });
  } catch {
    throw new Error("Could not load the project. Check its URL and connection; redirects are not supported.");
  }
  if (response.url && new URL(response.url).origin !== page.origin) throw new Error("The project response came from another origin.");
  if (!response.ok) throw new Error(`Could not load the project (HTTP ${response.status}).`);
  let data;
  try { data = await response.json(); }
  catch { throw new Error("The project file is not valid JSON."); }
  return { url, manifest: validateProjectManifest(data) };
}
