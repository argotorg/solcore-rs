import { readFile, readdir, stat } from "node:fs/promises";
import { dirname, extname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { loadProjects } from "./projects.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function markdownFiles(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await markdownFiles(path));
    else if (entry.isFile() && extname(path) === ".md") files.push(path);
  }
  return files.sort();
}

async function isFile(path) {
  try { return (await stat(path)).isFile(); }
  catch (error) {
    if (error.code === "ENOENT" || error.code === "ENOTDIR") return false;
    throw error;
  }
}

// Handle balanced parentheses in ordinary inline Markdown URLs, including
// function signatures. Reference links and HTML hrefs are checked as well.
function links(markdown) {
  const found = [];
  for (const match of markdown.matchAll(/!?\[[^\]\n]*\]\(/g)) {
    const start = match.index + match[0].length;
    if (markdown[start] === "<") {
      const end = markdown.indexOf(">", start);
      if (end !== -1) found.push(markdown.slice(start + 1, end));
      continue;
    }
    let depth = 1;
    let end = start;
    for (; end < markdown.length; end += 1) {
      const character = markdown[end];
      if (character === "\\") { end += 1; continue; }
      if (character === "(") depth += 1;
      if (character === ")" && --depth === 0) break;
      if (/\s/.test(character)) break;
    }
    found.push(markdown.slice(start, end));
  }
  for (const match of markdown.matchAll(/^\s*\[[^\]]+\]:\s*<?([^\s>]+)>?/gm)) found.push(match[1]);
  for (const match of markdown.matchAll(/\bhref=["']([^"']+)["']/g)) found.push(match[1].replaceAll("&amp;", "&"));
  return found;
}

/** Validate local book dependencies and routes; never request external URLs. */
export async function checkTutorial({ root = repositoryRoot, projects } = {}) {
  const source = join(root, "solcore-tutorial/src");
  const registry = projects ?? await loadProjects(root);
  const projectsById = new Map(registry.map((project) => [project.id, project]));
  const testedSources = new Set(registry.flatMap((project) => project.files.map((file) =>
    resolve(root, "solcore-tutorial/examples", project.id, file.path))));
  const errors = [];
  const report = (file, message) => errors.push(`${relative(root, file)}: ${message}`);
  const summary = join(source, "SUMMARY.md");
  if (!await isFile(summary)) return [`${relative(root, summary)}: missing table of contents`];

  for (const file of await markdownFiles(source)) {
    const markdown = await readFile(file, "utf8");
    for (const match of markdown.matchAll(/\{\{#include\s+([^}\n]+)\}\}/g)) {
      const target = match[1].trim().split(":")[0];
      const included = resolve(dirname(file), target);
      if (!await isFile(included)) report(file, `missing include: ${target}`);
      else if (included.endsWith(".sol") && !testedSources.has(included)) {
        report(file, `source include is not owned and tested by the tutorial: ${target}`);
      }
    }
    for (const match of markdown.matchAll(/^```(?:sol|solcore|solidity)\s*\n([\s\S]*?)^```\s*$/gm)) {
      if (!/^\s*\{\{#include\s+[^}\n]+\}\}\s*$/.test(match[1])) {
        report(file, "Solcore code blocks must include a tested source file instead of copying code");
      } else {
        const target = match[1].match(/\{\{#include\s+([^}\n]+)\}\}/)[1].trim().split(":")[0];
        const included = resolve(dirname(file), target);
        if (!included.endsWith(".sol") && await isFile(included) && !testedSources.has(included)) {
          report(file, `source include is not owned and tested by the tutorial: ${target}`);
        }
      }
    }

    // Code samples are not links. Includes above are checked before removing
    // fenced examples so documentation about URL syntax can use text fences.
    const prose = markdown.replace(/^```[^\n]*\n[\s\S]*?^```\s*$/gm, "");
    for (const target of links(prose)) {
      if (!target || target.startsWith("#")) continue;
      if (/^[a-z][a-z\d+.-]*:/i.test(target) || target.startsWith("//")) {
        let url;
        try { url = new URL(target); }
        catch { report(file, `malformed URL: ${target}`); continue; }
        if (url.origin === "https://playground.solcore.soliditylang.org" && url.hash.startsWith("#/examples")) {
          report(file, `exercise must use a tutorial-owned project: ${target}`);
        }
      } else {
        const path = target.split(/[?#]/)[0];
        let decoded;
        try { decoded = decodeURIComponent(path); }
        catch { report(file, `malformed local link: ${target}`); continue; }
        const projectLink = decoded.match(/^(?:\.\.\/)+projects\/([^/]+)\.json$/);
        if (projectLink) {
          const project = projectsById.get(projectLink[1]);
          if (!project) { report(file, `unknown tutorial project: ${projectLink[1]}`); continue; }
          const params = new URL(target, "https://tutorial.invalid/src/").searchParams;
          if (params.has("file") && !project.files.some((item) => item.path === params.get("file"))) {
            report(file, `unknown file ${params.get("file")} in tutorial project ${project.id}`);
          }
          continue;
        }
        if (!await isFile(resolve(dirname(file), decoded))) report(file, `missing local link: ${target}`);
      }
    }
  }
  return errors;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const errors = await checkTutorial();
  if (errors.length) {
    console.error(errors.join("\n"));
    process.exitCode = 1;
  } else {
    console.log("Tutorial chapter links, tested source includes, and project routes are valid.");
  }
}
