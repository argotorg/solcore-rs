import { createHash } from "node:crypto";
import { readFile, readdir, stat } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { SOLJSON_FILE, SOLJSON_SHA256 } from "../../playground/src/compiler/solcRelease.js";
import { validateProjectManifest } from "../../playground/src/project/loadProject.js";

async function filesIn(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await filesIn(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

/** Catch incomplete distribution directories and links that escape the bundle. */
export async function checkTutorialBundle(bookRoot) {
  bookRoot = resolve(bookRoot);
  const files = await filesIn(bookRoot);
  const names = files.map((file) => relative(bookRoot, file).replaceAll("\\", "/"));
  for (const required of ["index.html", "playground/index.html", "theme/tutorial.js", "theme/solcore-highlight.js", "theme/tutorial.css", "LICENSE"]) {
    if (!names.includes(required)) throw new Error(`Tutorial bundle is missing ${required}`);
  }
  for (const resource of [/^playground\/assets\/solcore_wasm_bg-.*\.wasm$/, /^playground\/assets\/solcore_lsp_bg-.*\.wasm$/]) {
    if (!names.some((name) => resource.test(name))) throw new Error(`Tutorial bundle is missing runtime asset ${resource}`);
  }
  if (names.some((name) => name.startsWith("playground/tutorial/"))) {
    throw new Error("Tutorial bundle recursively includes another tutorial");
  }
  const solc = await readFile(join(bookRoot, "playground/solc", SOLJSON_FILE));
  if (createHash("sha256").update(solc).digest("hex") !== SOLJSON_SHA256) {
    throw new Error("Tutorial bundle has a missing or incorrect solc runtime");
  }
  const projectNames = names.filter((name) => /^projects\/[^/]+\.json$/.test(name));
  if (!projectNames.length) throw new Error("Tutorial bundle is missing project manifests");
  for (const name of projectNames) {
    const project = validateProjectManifest(JSON.parse(await readFile(join(bookRoot, name), "utf8")));
    if (name !== `projects/${project.id}.json`) throw new Error(`Project id does not match its filename: ${name}`);
  }
  for (const file of files.filter((path) => path.endsWith(".html"))) {
    const html = await readFile(file, "utf8");
    for (const match of html.matchAll(/\b(?:href|src)=["']([^"']+)["']/g)) {
      const href = match[1].replaceAll("&amp;", "&");
      if (/^https:\/\/playground\.solcore\.soliditylang\.org\/#\/examples\//.test(href)) {
        throw new Error(`${relative(bookRoot, file)} still sends an exercise to the public playground`);
      }
      if (!href || href.startsWith("#") || /^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(href)) continue;
      const path = decodeURIComponent(href.split(/[?#]/)[0]);
      const target = resolve(dirname(file), path);
      if (path.startsWith("/") || relative(bookRoot, target).startsWith("..")) {
        throw new Error(`${relative(bookRoot, file)} depends on a path outside the tutorial: ${href}`);
      }
      try {
        const info = await stat(target);
        if (info.isDirectory()) await stat(join(target, "index.html"));
      } catch {
        throw new Error(`${relative(bookRoot, file)} links to a missing bundled file: ${href}`);
      }
      const url = new URL(href, "https://bundle.invalid/");
      if (url.searchParams.has("project")) {
        const manifest = url.searchParams.get("project");
        const expected = /^\.\.\/projects\/([a-z0-9-]+)\.json$/.exec(manifest);
        if (!expected || !projectNames.includes(`projects/${expected[1]}.json`)) {
          throw new Error(`${relative(bookRoot, file)} links to a missing bundled project: ${href}`);
        }
        if (!url.hash.startsWith("#/project")) throw new Error(`Invalid project view route: ${href}`);
      }
    }
  }
  console.log(`Tutorial distribution verified (${files.length} files, local compiler, LSP, and solc).`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    await checkTutorialBundle(process.argv[2] ?? fileURLToPath(new URL("../book", import.meta.url)));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
