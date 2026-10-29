import { readFile, realpath } from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { validateProjectManifest } from "../../playground/src/project/loadProject.js";

export const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** Read curriculum-owned sources once for book checks, manifests, and tests. */
export async function loadProjects(root = repositoryRoot) {
  const source = join(root, "solcore-tutorial/examples");
  const sourceRoot = await realpath(source);
  const catalog = JSON.parse(await readFile(join(source, "catalog.json"), "utf8"));
  if (!Array.isArray(catalog) || !catalog.length) throw new Error("The tutorial catalog must list at least one project.");
  const ids = new Set();
  const projects = [];
  for (const item of catalog) {
    if (!item || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(item.id) || ids.has(item.id)) {
      throw new Error(`Invalid or duplicate tutorial project id: ${item?.id}`);
    }
    ids.add(item.id);
    // Validate virtual paths before reading them from disk.
    const project = validateProjectManifest({ ...item, version: 1,
      files: Array.isArray(item.files) ? item.files.map((path) => ({ path, content: "" })) : item.files });
    const directory = await realpath(join(source, project.id));
    const projectRelative = relative(sourceRoot, directory);
    if (projectRelative === ".." || projectRelative.startsWith(`..${sep}`)) {
      throw new Error(`Project directory escapes tutorial ownership: ${project.id}`);
    }
    for (const file of project.files) {
      const actual = await realpath(join(directory, file.path));
      const rel = relative(directory, actual);
      if (rel === ".." || rel.startsWith(`..${sep}`)) throw new Error(`Project source escapes its directory: ${file.path}`);
      if (!file.path.endsWith(".sol")) throw new Error(`Expected a Solcore source: ${file.path}`);
      file.content = await readFile(actual, "utf8");
    }
    projects.push(project);
  }
  return projects;
}
