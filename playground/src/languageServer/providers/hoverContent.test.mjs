import assert from "node:assert/strict";
import test from "node:test";

import { markdownForHoverContents } from "./hoverContent.js";

test("hover arrays preserve both signature and documentation", () => {
  assert.deepEqual(
    markdownForHoverContents([
      { language: "solcore", value: "function value() returns (word)" },
      "Returns the current value.",
    ]),
    [
      { value: "```solcore\nfunction value() returns (word)\n```" },
      { value: "Returns the current value." },
    ],
  );
});
