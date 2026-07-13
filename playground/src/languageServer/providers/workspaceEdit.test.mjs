import assert from "node:assert/strict";
import test from "node:test";

import { workspaceEditTargetsAreCurrent } from "./workspaceEdit.js";

const changes = { "file:///main/main.solc": [{ newText: "renamed" }] };

test("workspace edit rejects a model changed after the request", () => {
  const expected = new Map([["file:///main/main.solc", 7]]);
  const getModel = () => ({ getVersionId: () => 8 });
  assert.equal(workspaceEditTargetsAreCurrent(changes, expected, getModel), false);
});

test("workspace edit accepts unchanged target models", () => {
  const expected = new Map([["file:///main/main.solc", 7]]);
  const getModel = () => ({ getVersionId: () => 7 });
  assert.equal(workspaceEditTargetsAreCurrent(changes, expected, getModel), true);
});
