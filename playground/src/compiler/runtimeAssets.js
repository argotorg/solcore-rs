/** Resolve public assets against the document, never the generated worker URL. */
export function runtimeAssetUrl(base, path, documentUrl) {
  return new URL(`${base}${path}`, documentUrl).href;
}
