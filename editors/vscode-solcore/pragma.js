"use strict";

// The compiler requires `pragma solcore;` as the first declaration of every
// source file. Matching the compiler's lexer, only these characters are
// whitespace, so a byte-order mark is not skipped.
const WHITESPACE = new Set([" ", "\t", "\n", "\r", "\f"]);
const DECLARATION = /pragma[ \t\n\r\f]+solcore[ \t\n\r\f]*;/y;

/**
 * Returns whether `text` starts with `pragma solcore;` after leading whitespace
 * and comments. This is how Solcore `.sol` files are told apart from Solidity
 * files, which share the extension.
 */
function declaresPragmaSolcore(text) {
  let index = 0;
  for (;;) {
    while (index < text.length && WHITESPACE.has(text[index])) {
      index += 1;
    }
    if (text.startsWith("//", index)) {
      const end = text.indexOf("\n", index);
      if (end === -1) return false;
      index = end + 1;
    } else if (text.startsWith("/*", index)) {
      const end = text.indexOf("*/", index + 2);
      if (end === -1) return false;
      index = end + 2;
    } else {
      DECLARATION.lastIndex = index;
      return DECLARATION.test(text);
    }
  }
}

module.exports = { declaresPragmaSolcore };
