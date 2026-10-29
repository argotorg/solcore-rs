import assert from "node:assert/strict";
import { mkdtemp, mkdir, rename, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import test from "node:test";
import { checkTutorial } from "../scripts/check-tutorial.mjs";
import { loadProjects } from "../scripts/projects.mjs";

const entry = { id: "hello", name: "Hello", entry: "greetings/Hello.sol", files: ["greetings/Hello.sol"] };
const route = "../projects/hello.json?file=greetings%2FHello.sol&tab=run";

async function fixture(t, chapter) {
  const root = await mkdtemp(join(tmpdir(), "solcore-tutorial-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = join(root, "solcore-tutorial/src");
  const examples = join(root, "solcore-tutorial/examples");
  await mkdir(source, { recursive: true });
  await mkdir(join(examples, "hello/greetings"), { recursive: true });
  await writeFile(join(source, "SUMMARY.md"), "# Summary\n\n- [Hello](hello.md)\n");
  await writeFile(join(source, "hello.md"), chapter);
  await writeFile(join(examples, "catalog.json"), JSON.stringify([entry]));
  await writeFile(join(examples, "hello/greetings/Hello.sol"), "function main() returns (word) { return 42; }\n");
  return root;
}

test("tutorial checks its own sources and encoded project paths without the Playground registry", async (t) => {
  const root = await fixture(t, `# Hello\n\n[Run](${route})\n\n\`\`\`solidity\n{{#include ../examples/hello/greetings/Hello.sol}}\n\`\`\`\n`);
  assert.deepEqual(await checkTutorial({ root }), []);
  const [project] = await loadProjects(root);
  assert.equal(project.version, 1);
  assert.match(project.files[0].content, /return 42/);
});

test("tutorial rejects a renamed chapter, deleted source, and copied Solcore snippet", async (t) => {
  const root = await fixture(t, "[Next](renamed.md)\n\n```solidity\n{{#include ../../missing.sol}}\n```\n\n```sol\nfunction old() {}\n```\n");
  await writeFile(join(root, "solcore-tutorial/src/SUMMARY.md"), "- [Missing](deleted.md)\n");
  const errors = await checkTutorial({ root });
  assert.equal(errors.length, 4, errors.join("\n"));
  assert.ok(errors.some((error) => error.includes("deleted.md")));
  assert.ok(errors.some((error) => error.includes("renamed.md")));
  assert.ok(errors.some((error) => error.includes("missing include: ../../missing.sol")));
  assert.ok(errors.some((error) => error.includes("instead of copying code")));
});

test("tutorial rejects unknown projects, stale files, malformed URLs, and public example routes", async (t) => {
  const root = await fixture(t, [
    `[Unknown](${route.replace("hello.json", "old-project.json")})`,
    `[Wrong file](${route.replace("greetings%2FHello.sol", "Old.sol")})`,
    "[Malformed](../projects/%zz.json)",
    "[Old](https://playground.solcore.soliditylang.org/#/examples/contract-output)",
  ].join("\n"));
  const errors = await checkTutorial({ root });
  assert.equal(errors.length, 4, errors.join("\n"));
  assert.ok(errors.some((error) => error.includes("unknown tutorial project: old-project")));
  assert.ok(errors.some((error) => error.includes("unknown file Old.sol")));
  assert.ok(errors.some((error) => error.includes("malformed local link")));
  assert.ok(errors.some((error) => error.includes("tutorial-owned project")));
});

test("tutorial links accept function signatures and check reference and HTML links", async (t) => {
  const root = await fixture(t, [
    `[Call](${route}&function=answer(uint256))`,
    `[Reference]: ${route.replace("hello.json", "missing.json")}`,
    `<a href="${route.replace("greetings%2FHello.sol", "Missing.sol")}">Example</a>`,
  ].join("\n"));
  const errors = await checkTutorial({ root });
  assert.equal(errors.length, 2, errors.join("\n"));
  assert.ok(errors.some((error) => error.includes("unknown tutorial project: missing")));
  assert.ok(errors.some((error) => error.includes("unknown file Missing.sol")));
});

test("source listings must be owned by a tested tutorial project", async (t) => {
  const root = await fixture(t, "```sol\n{{#include ../../Playground.sol}}\n```\n\n```solidity\n{{#include ../../copied-source.txt}}\n```\n");
  await writeFile(join(root, "Playground.sol"), "function main() returns (word) { return 42; }\n");
  await writeFile(join(root, "copied-source.txt"), "function main() returns (word) { return 42; }\n");
  const errors = await checkTutorial({ root });
  assert.equal(errors.length, 2, errors.join("\n"));
  assert.ok(errors.every((error) => error.includes("not owned and tested by the tutorial")));
});

test("catalog rejects duplicate IDs and invalid source paths before reading files", async (t) => {
  const root = await fixture(t, "# Hello");
  const catalog = join(root, "solcore-tutorial/examples/catalog.json");
  await writeFile(catalog, JSON.stringify([entry, entry]));
  await assert.rejects(loadProjects(root), /duplicate tutorial project id/);
  await writeFile(catalog, JSON.stringify([{ ...entry, entry: "../outside.sol", files: ["../outside.sol"] }]));
  await assert.rejects(loadProjects(root), /relative file path/);
});

test("the shipped tutorial points to owned source projects and readable chapters", async () => {
  assert.deepEqual(await checkTutorial(), []);
});

test("catalog cannot outsource an entire project directory through a symlink", async (t) => {
  const root = await fixture(t, "# Hello");
  const project = join(root, "solcore-tutorial/examples/hello");
  const elsewhere = join(root, "other-project");
  await rename(project, elsewhere);
  await symlink(elsewhere, project, "junction");
  await assert.rejects(loadProjects(root), /escapes tutorial ownership/);
});
