import { cp } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build, preview } from "vite";

const playgroundRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Build a portable editor/runtime bundle for standalone use or embedding. */
export async function buildPlayground({ outDir = join(playgroundRoot, "dist"), base } = {}) {
  await build({
    configFile: join(playgroundRoot, "vite.config.ts"),
    root: playgroundRoot,
    ...(base === undefined ? {} : { base }),
    publicDir: false,
    build: { outDir, emptyOutDir: true },
  });
  // WASM and workers are emitted by Vite. solc is the only unbundled runtime.
  await cp(join(playgroundRoot, "public/solc"), join(outDir, "solc"), { recursive: true });
}

/** Preview a built static site using the same tooling as the editor bundle. */
export function previewStaticSite({ outDir, port = 4173 }) {
  return preview({
    configFile: false,
    root: dirname(resolve(outDir)),
    base: "/",
    appType: "mpa",
    build: { outDir: resolve(outDir) },
    preview: { host: "127.0.0.1", port, strictPort: true, headers: { "Cache-Control": "no-cache" } },
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await buildPlayground();
}
