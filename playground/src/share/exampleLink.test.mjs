import assert from "node:assert/strict";
import test from "node:test";

import { buildExampleLink, buildProjectLink, readExampleLocation, readExampleRoute, readExampleView, readProjectView, readSharedExampleId } from "./exampleLink.js";

test("reads the example id from a query string", () => {
  assert.equal(readSharedExampleId("?example=trait"), "trait");
  assert.equal(readSharedExampleId("?foo=1&example=mini-nft"), "mini-nft");
});

test("ignores a missing, empty, or blank example id", () => {
  assert.equal(readSharedExampleId(""), null);
  assert.equal(readSharedExampleId("?foo=1"), null);
  assert.equal(readSharedExampleId("?example="), null);
  assert.equal(readSharedExampleId("?example=%20%20"), null);
});

test("builds a link that keeps the deployment subpath", () => {
  assert.equal(
    buildExampleLink("https://example.org/solcore-rs/", "hello"),
    "https://example.org/solcore-rs/#/examples/hello",
  );
});

test("building a link replaces the legacy query and hash", () => {
  assert.equal(
    buildExampleLink("https://example.org/?example=hello#frag", "generics"),
    "https://example.org/#/examples/generics",
  );
});

test("reads example routes and rejects unrelated or malformed hashes", () => {
  assert.equal(readExampleRoute("#/examples/trait"), "trait");
  assert.equal(readExampleRoute("#/examples/a%20b"), "a b");
  for (const hash of ["", "#trait", "#/examples/", "#/examples/a/b", "#/examples/%zz"]) {
    assert.equal(readExampleRoute(hash), null);
  }
});

test("links preserve other queries and encode the example id", () => {
  const link = buildExampleLink("https://example.org/app/?theme=dark&example=hello", "a/b");
  assert.equal(link, "https://example.org/app/?theme=dark#/examples/a%2Fb");
  assert.equal(readExampleRoute(new URL(link).hash), "a/b");
});

test("view links round-trip filenames and overloaded function signatures", () => {
  const view = { file: "folder/a b.sol", tab: "run", view: "tests", contract: "C", function: "add(uint256,uint256)", test: "folder/a b.sol:42", selection: "12:3-15:8", backend: "sonatina" };
  const url = new URL(buildExampleLink("https://example.org/app/?keep=1", "trait", view));
  assert.equal(readExampleRoute(url.hash), "trait");
  assert.deepEqual(readExampleLocation(url.hash), { id: "trait", view });
  assert.deepEqual(readExampleView(url.hash), view);
  assert.equal(url.search, "?keep=1");
});

test("missing, empty and unknown view fields do not invent selections", () => {
  const view = readExampleView("#/examples/trait?file=&unrelated=42");
  assert.ok(Object.values(view).every((value) => value === undefined));
  assert.equal(buildExampleLink("https://example.org/", "trait", view), "https://example.org/#/examples/trait");
  assert.equal(readExampleRoute("#/examples/%zz?file=Hello.sol"), null);
});

test("external project view links preserve the manifest URL and embedded mode", () => {
  const href = buildProjectLink('https://example.org/editor/?embed=1&project=../projects/a.json&example=old', {
    file: 'lib/a b.sol', tab: 'run', view: 'tests', function: 'add(uint256,uint256)',
  });
  const url = new URL(href);
  assert.equal(url.searchParams.get('project'), '../projects/a.json');
  assert.equal(url.searchParams.get('embed'), '1');
  assert.equal(url.searchParams.has('example'), false);
  assert.equal(readProjectView(url.hash).file, 'lib/a b.sol');
  assert.equal(readProjectView(url.hash).function, 'add(uint256,uint256)');
  assert.equal(readExampleRoute(url.hash), null);
  for (const hash of ['#/examples/contract-output', '#/project/extra', '#project', '#/projects']) {
    assert.equal(readProjectView(hash), null);
  }
});
