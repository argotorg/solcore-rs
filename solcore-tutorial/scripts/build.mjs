import { cp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, extname, join, posix, relative, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { checkTutorial } from "./check-tutorial.mjs";
import { checkTutorialBundle } from "./check-tutorial-bundle.mjs";
import { loadProjects } from "./projects.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const MDBOOK_VERSION = "0.4.52";
const installCommand = `cargo install mdbook --version ${MDBOOK_VERSION} --locked`;

/** Link to the bundled entry file, without assuming a domain or deployment path. */
export function rebasePlaygroundLinks(html, htmlFile, bookRoot) {
  const playgroundPath = relative(dirname(htmlFile), join(bookRoot, "playground/index.html")).split(sep).join("/");
  const chapterDirectory = relative(bookRoot, dirname(htmlFile)).split(sep).join("/");
  return html.replace(
    /\bhref=(['"])([^'"]+)\1/g,
    (match, quote, href) => {
      if (/^(?:[a-z][a-z\d+.-]*:|\/|#)/i.test(href)) return match;
      const [path, view = ""] = href.split(/\?(.*)/s);
      // mdBook prefixes nested chapter links in print.html; normalize those
      // paths as well as ordinary chapter-relative references.
      const project = posix.normalize(posix.join(chapterDirectory, path))
        .match(/^\.\.\/projects\/([a-z0-9-]+)\.json$/);
      if (!project) return match;
      return `href=${quote}${playgroundPath}?project=../projects/${project[1]}.json#/project${view ? `?${view}` : ""}${quote}`;
    },
  );
}

async function rewriteHtml(bookRoot) {
  for (const entry of await readdir(bookRoot, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile() || extname(entry.name) !== ".html") continue;
    const path = join(entry.parentPath, entry.name);
    const before = await readFile(path, "utf8");
    const after = rebasePlaygroundLinks(before, path, bookRoot);
    if (after !== before) await writeFile(path, after);
  }
}

function runMdbook(args, options) {
  const result = spawnSync("mdbook", args, options);
  if (result.error?.code === "ENOENT") {
    throw new Error(`mdbook was not found. Install the pinned version with: ${installCommand}`);
  }
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const reason = result.signal ? `signal ${result.signal}` : `exit code ${result.status}`;
    throw new Error(`mdbook ${args[0]} failed with ${reason}`);
  }
  return result;
}

export async function buildTutorial(root = repositoryRoot) {
  const projects = await loadProjects(root);
  const errors = await checkTutorial({ root, projects });
  if (errors.length) throw new Error(errors.join("\n"));

  const destination = join(root, "solcore-tutorial/book");
  // Avoid retaining projects or runtime assets removed from the curriculum.
  await rm(destination, { recursive: true, force: true });
  runMdbook(["build", join(root, "solcore-tutorial"), "--dest-dir", destination], { stdio: "inherit" });
  await rewriteHtml(destination);
  await mkdir(join(destination, "projects"), { recursive: true });
  for (const project of projects) {
    await writeFile(join(destination, "projects", `${project.id}.json`), `${JSON.stringify(project, null, 2)}\n`);
  }
  // The reusable app knows only the generic project format, never this book.
  const { buildPlayground } = await import("../../playground/scripts/build.mjs");
  await buildPlayground({ outDir: join(destination, "playground"), base: "./" });
  await cp(join(root, "LICENSE"), join(destination, "LICENSE"));
  await checkTutorialBundle(destination);

  console.log(`Self-contained tutorial built at ${destination}. Serve or distribute this entire directory.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2);
    if (args.length) throw new Error("Usage: node scripts/build.mjs");
    await buildTutorial();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
