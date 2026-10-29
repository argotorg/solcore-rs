"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { declaresPragmaSolcore } = require("../pragma");

test("a leading pragma solcore marks a Solcore file", () => {
  assert.equal(declaresPragmaSolcore("pragma solcore;\n\ncontract C {}\n"), true);
});

test("leading comments and whitespace are skipped", () => {
  const source = "// SPDX-License-Identifier: MIT\n/* header\n */\n  pragma  solcore ;\n";
  assert.equal(declaresPragmaSolcore(source), true);
});

test("Solidity files and misplaced pragmas are not Solcore", () => {
  assert.equal(declaresPragmaSolcore("// SPDX-License-Identifier: MIT\npragma solidity ^0.8.0;\n"), false);
  assert.equal(declaresPragmaSolcore("import * from std;\npragma solcore;\n"), false);
  assert.equal(declaresPragmaSolcore("pragma solcore-extra;\n"), false);
  assert.equal(declaresPragmaSolcore("/* unterminated pragma solcore;"), false);
  assert.equal(declaresPragmaSolcore(""), false);
});

test("a byte-order mark is not whitespace", () => {
  assert.equal(declaresPragmaSolcore("﻿pragma solcore;\n"), false);
});
