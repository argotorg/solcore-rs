import assert from "node:assert/strict";
import { join } from "node:path";
import test from "node:test";
import { rebasePlaygroundLinks } from "../scripts/build.mjs";

const bookRoot = join(process.cwd(), "book");
const view = "?file=option.sol&amp;tab=run&amp;view=tests";
const route = `?project=../projects/option.json#/project${view}`;
const link = `<a href="../projects/option.json${view}">Try it</a>`;

test("standalone tutorial links select a generic bundled project editor", () => {
  const page = join(bookRoot, "modules.html");
  assert.equal(rebasePlaygroundLinks(link, page, bookRoot), `<a href="playground/index.html${route}">Try it</a>`);
});

test("nested chapters and print output resolve the same manifest under a deployment subpath", () => {
  for (const [chapter, expected] of [
    ["print.html", "playground/"],
    ["advanced/modules.html", "../playground/"],
    ["advanced/traits/generics.html", "../../playground/"],
  ]) {
    const page = join(bookRoot, chapter);
    const depth = chapter.split("/").length;
    const chapterLink = link.replace("../projects", `${"../".repeat(depth)}projects`);
    const html = rebasePlaygroundLinks(chapterLink, page, bookRoot);
    const href = html.match(/href="([^"]+)"/)[1].replaceAll("&amp;", "&");
    assert.ok(href.startsWith(expected));
    const resolved = new URL(href, `https://example.org/repo/tutorial/${chapter}`);
    assert.equal(resolved.href, `https://example.org/repo/tutorial/playground/index.html${route.replaceAll("&amp;", "&")}`);
    assert.equal(new URL(resolved.searchParams.get("project"), resolved).href,
      "https://example.org/repo/tutorial/projects/option.json");
  }
});

test("rewriting preserves unrelated links, source text, and already rewritten links", () => {
  const html = [
    '<a href="https://doc.rust-lang.org/book/">Rust Book</a>',
    '<a href="https://playground.solcore.soliditylang.org/">Playground home</a>',
    `<a href="playground/index.html${route}">Ready</a>`,
    '<a href="traits.html">Next</a>',
    '<code>../projects/option.json</code>',
  ].join("\n");
  assert.equal(rebasePlaygroundLinks(html, join(bookRoot, "index.html"), bookRoot), html);
});

test("rewriting handles quote styles, encoded signatures, and links without view parameters", () => {
  const single = "<a href='../projects/trait.json?function=toggle%28%29'>Call</a>";
  const plain = '<a href="../projects/contract-output.json">Hello</a>';
  const html = rebasePlaygroundLinks(`${link}\n${single}\n${plain}`, join(bookRoot, "index.html"), bookRoot);
  assert.equal(html, `<a href="playground/index.html${route}">Try it</a>\n<a href='playground/index.html?project=../projects/trait.json#/project?function=toggle%28%29'>Call</a>\n<a href="playground/index.html?project=../projects/contract-output.json#/project">Hello</a>`);
});

test("print output rewrites mdBook-prefixed links from nested source chapters", () => {
  const html = `<a href="advanced/traits/../../../projects/option.json${view}">Try it</a>`;
  assert.equal(rebasePlaygroundLinks(html, join(bookRoot, "print.html"), bookRoot),
    `<a href="playground/index.html${route}">Try it</a>`);
});
