/**
 * Shareable example links.
 *
 * Links refer to examples bundled with the deployed Playground: `https://host/#/examples/trait`.
 */

export const EXAMPLE_PARAM = "example";

/** Reads the example id from a `location.search` string, if present. */
export function readSharedExampleId(search) {
  if (typeof search !== "string" || search.length === 0) {
    return null;
  }

  const value = new URLSearchParams(search).get(EXAMPLE_PARAM);
  if (value === null) {
    return null;
  }

  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/** Builds the shareable link for an example id, based on the current href. */
export function buildExampleLink(href, id, view = {}) {
  const url = new URL(href);
  url.searchParams.delete(EXAMPLE_PARAM);
  const params = new URLSearchParams();
  for (const key of ["file", "tab", "view", "contract", "function", "test", "selection"]) {
    if (view[key]) params.set(key, view[key]);
  }
  url.hash = `/examples/${encodeURIComponent(id)}${params.size ? `?${params}` : ""}`;
  return url.toString();
}

/** Reads the single example route; malformed and unrelated hashes are ignored. */
export function readExampleRoute(hash) {
  const match = /^#\/examples\/([^/?]+)(?:\?.*)?$/.exec(hash);
  if (!match) return null;
  try {
    return decodeURIComponent(match[1]);
  } catch {
    return null;
  }
}

/** Optional view fields; callers validate names against the loaded workspace. */
export function readExampleView(hash) {
  const params = new URLSearchParams(hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "");
  return Object.fromEntries(["file", "tab", "view", "contract", "function", "test", "selection"]
    .map((key) => [key, params.get(key) || undefined]));
}
