import assert from "node:assert/strict";
import test from "node:test";

import { buildExampleLink, readExampleRoute, readSharedExampleId } from "./exampleLink.js";

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
