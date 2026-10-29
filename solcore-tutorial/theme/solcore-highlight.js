// Extend mdBook's bundled Highlight.js; no CDN or additional runtime is needed.
hljs.registerLanguage("solcore", (hljs) => ({
  name: "Solcore",
  aliases: ["solidity", "sol"],
  keywords: {
    keyword: "import from export as contract constructor function returns public payable fallback "
      + "let if else for return break continue match case default enum type trait impl where "
      + "comptime lam assembly data class forall instance switch leave pragma",
    type: "word uint256 bool address string",
    literal: "true false",
  },
  contains: [
    hljs.C_LINE_COMMENT_MODE,
    hljs.C_BLOCK_COMMENT_MODE,
    hljs.QUOTE_STRING_MODE,
    hljs.C_NUMBER_MODE,
  ],
}));

// mdBook loads additional-js after book.js has visited the code blocks.
// Highlight the newly registered language without touching other languages.
for (const block of document.querySelectorAll(
  "pre code.language-solidity, pre code.language-solcore, pre code.language-sol",
)) {
  hljs.highlightBlock(block);
}
