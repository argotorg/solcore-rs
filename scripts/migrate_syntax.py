#!/usr/bin/env python3
"""Migrate classic Solcore source files to the Solidity-style surface syntax.

The migration is deliberately lexical rather than a collection of unrestricted
regular-expression substitutions.  Comments, strings, and complete ``assembly``
blocks are opaque tokens, so words such as ``function`` or ``data`` inside them
are never rewritten.

The command accepts files and directories.  Directories are searched
recursively for ``.sol`` and ``.solc`` files.

    scripts/migrate_syntax.py path/to/file.solc path/to/fixtures
    scripts/migrate_syntax.py --check path/to/fixtures
    scripts/migrate_syntax.py --classic-bare-imports path/to/classic/sources

The rewrite is idempotent: running it again on migrated input produces no
further changes.  It intentionally leaves ``export`` declarations alone because
new_syntax.md does not choose a replacement export/re-export surface.

Pass ``--classic-bare-imports`` when the input still uses Classic Solcore's
``import M;`` namespace semantics.  That spelling becomes
``import * as M from M;`` before the remaining syntax migration; without the
flag, canonical Core ``import M;`` keeps its open-import meaning.

Classic prefix-dot enum constructors are qualified from their declaration
owner when the leaf has one unambiguous owner in the CLI input
(``.Some(...)`` becomes ``Option.Some(...)``).  Ambiguous or unresolved
prefix-dot constructors stop the migration with a diagnostic instead of
silently producing source that the new parser rejects.

Bare same-name constructors are also qualified when their owner is
unambiguous (``enum Point { Point(...) }`` makes term and pattern uses become
``Point.Point(...)``).  A source that deliberately tests a rejected
unqualified spelling can opt out of these passes with this comment:

    // migrate-syntax: keep-unqualified-constructor

Likewise, a negative fixture that deliberately exercises any rejected Classic
surface can opt out of the complete rewrite with:

    // migrate-syntax: keep-legacy-negative
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys
from typing import Iterable, Mapping, Sequence


TRIVIA = {"ws", "comment"}
WORD_RE = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
NUMBER_RE = re.compile(r"(?:0[xX][0-9A-Fa-f]+|[0-9]+)")
SOLCORE_LITERAL_HINT_RE = re.compile(
    r"(?:"
    r"\bfunction\s+[A-Za-z_$][A-Za-z0-9_$]*\s*(?:<[^>{};]*>)?\s*\("
    r"|\b(?:contract|interface|library)\s+[A-Za-z_$][A-Za-z0-9_$]*\s+\{"
    r"|\b(?:enum|trait)\s+[A-Za-z_$][A-Za-z0-9_$.]*(?:\s*<[^>{};]*>)?\s+\{"
    r"|\bdata\s+[A-Za-z_$][^;{}\n]*=\s*[^;\n]+;"
    r"|\balias\s+[A-Za-z_$][^;{}\n]*=\s*[^;\n]+;"
    r"|\bclass\s+[A-Za-z_$][^;{}\n]*:"
    r"[A-Za-z_$][A-Za-z0-9_$.]*(?:\([^;{}\n]*\))?\s+\{"
    r"|\bimpl(?:\s*<[^>{};]*>)?\s+[A-Za-z_$][A-Za-z0-9_$.]*"
    r"(?:\s*<[^>{};]*>)?\s+\{"
    r"|\binstance\s+[A-Za-z_$][^;{}\n]*:"
    r"[A-Za-z_$][A-Za-z0-9_$.]*(?:\([^;{}\n]*\))?\s+\{"
    r"|\b(?:import|export|pragma)\s+(?:[@*{A-Za-z_$])[^;\n]*;"
    r"|\bmatch\s*\("
    r")",
    re.DOTALL,
)
MULTI_SYMBOLS = (
    ">>>=",
    "<<=",
    ">>=",
    "->",
    "=>",
    ":=",
    "==",
    "!=",
    "<=",
    ">=",
    "&&",
    "||",
    "+=",
    "-=",
    "^=",
    "&=",
    "|=",
    "%=",
    "**",
    "<<",
    ">>",
)
MODIFIERS = {
    "public",
    "external",
    "internal",
    "private",
    "payable",
    "pure",
    "view",
}
PRAGMA_NAMES = {
    "no-coverage-condition": "noCoverageCondition",
    "no-patterson-condition": "noPattersonCondition",
    "no-bounded-variable-condition": "noBoundVariableCondition",
    "no-bound-variable-condition": "noBoundVariableCondition",
    "no-generic-instance-for": "noGenericInstanceFor",
}
LOCATIONS = {"memory", "storage", "calldata"}
BUILTIN_TYPE_NAMES = {
    "address",
    "bool",
    "byte",
    "bytes",
    "bytes1",
    "bytes2",
    "bytes4",
    "bytes8",
    "bytes16",
    "bytes20",
    "bytes32",
    "int",
    "integer",
    "string",
    "uint",
    "unit",
    "word",
}
KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER = (
    "migrate-syntax: keep-unqualified-constructor"
)
KEEP_LEGACY_NEGATIVE_MARKER = "migrate-syntax: keep-legacy-negative"
KEEP_RUST_FILE_MARKER = "migrate-syntax: keep-rust-file"
BUILTIN_CONSTRUCTORS = {"true", "false", "pair", "inl", "inr"}


@dataclass(frozen=True)
class Token:
    kind: str
    text: str
    start: int
    end: int


def _scan_quoted(source: str, start: int, quote: str) -> int:
    i = start + 1
    while i < len(source):
        if source[i] == "\\":
            i += 2
        elif source[i] == quote:
            return i + 1
        else:
            i += 1
    return len(source)


def _scan_block_comment(source: str, start: int) -> int:
    end = source.find("*/", start + 2)
    return len(source) if end < 0 else end + 2


def _skip_trivia_raw(source: str, start: int) -> int:
    i = start
    while i < len(source):
        if source[i].isspace():
            i += 1
        elif source.startswith("//", i):
            end = source.find("\n", i + 2)
            i = len(source) if end < 0 else end + 1
        elif source.startswith("/*", i):
            i = _scan_block_comment(source, i)
        else:
            break
    return i


def _scan_assembly(source: str, start: int, word_end: int) -> int | None:
    brace = _skip_trivia_raw(source, word_end)
    if brace >= len(source) or source[brace] != "{":
        return None
    depth = 0
    i = brace
    while i < len(source):
        if source.startswith("//", i):
            end = source.find("\n", i + 2)
            i = len(source) if end < 0 else end + 1
        elif source.startswith("/*", i):
            i = _scan_block_comment(source, i)
        elif source[i] in {'"', "'"}:
            i = _scan_quoted(source, i, source[i])
        elif source[i] == "{":
            depth += 1
            i += 1
        elif source[i] == "}":
            depth -= 1
            i += 1
            if depth == 0:
                return i
        else:
            i += 1
    return len(source)


def lex(source: str) -> list[Token]:
    tokens: list[Token] = []
    i = 0
    while i < len(source):
        start = i
        if source[i].isspace():
            i += 1
            while i < len(source) and source[i].isspace():
                i += 1
            tokens.append(Token("ws", source[start:i], start, i))
            continue
        if source.startswith("//", i):
            end = source.find("\n", i + 2)
            i = len(source) if end < 0 else end
            tokens.append(Token("comment", source[start:i], start, i))
            continue
        if source.startswith("/*", i):
            i = _scan_block_comment(source, i)
            tokens.append(Token("comment", source[start:i], start, i))
            continue
        if source[i] in {'"', "'"}:
            i = _scan_quoted(source, i, source[i])
            tokens.append(Token("string", source[start:i], start, i))
            continue
        word = WORD_RE.match(source, i)
        if word:
            i = word.end()
            text = word.group(0)
            if text == "assembly":
                assembly_end = _scan_assembly(source, start, i)
                if assembly_end is not None:
                    i = assembly_end
                    tokens.append(Token("assembly", source[start:i], start, i))
                    continue
            tokens.append(Token("word", text, start, i))
            continue
        number = NUMBER_RE.match(source, i)
        if number:
            i = number.end()
            tokens.append(Token("number", number.group(0), start, i))
            continue
        symbol = next(
            (candidate for candidate in MULTI_SYMBOLS if source.startswith(candidate, i)),
            None,
        )
        if symbol is not None:
            i += len(symbol)
            tokens.append(Token("symbol", symbol, start, i))
            continue
        i += 1
        tokens.append(Token("symbol", source[start:i], start, i))
    return tokens


def significant(source: str) -> list[Token]:
    return [token for token in lex(source) if token.kind not in TRIVIA]


def replace_spans(source: str, replacements: Iterable[tuple[int, int, str]]) -> str:
    result = source
    ordered = sorted(replacements, key=lambda item: (item[0], item[1]), reverse=True)
    last_start = len(source) + 1
    for start, end, replacement in ordered:
        if end > last_start:
            raise ValueError(f"overlapping migration replacements around byte {start}")
        result = result[:start] + replacement + result[end:]
        last_start = start
    return result


def comments_in(source: str, start: int, end: int) -> list[str]:
    return [
        token.text
        for token in lex(source[start:end])
        if token.kind == "comment"
    ]


OPEN_TO_CLOSE = {"(": ")", "[": "]", "{": "}", "<": ">"}
CLOSE_TO_OPEN = {close: open_ for open_, close in OPEN_TO_CLOSE.items()}


def matching_index(tokens: Sequence[Token], open_index: int) -> int | None:
    opener = tokens[open_index].text
    closer = OPEN_TO_CLOSE.get(opener)
    if closer is None:
        return None
    depth = 0
    for index in range(open_index, len(tokens)):
        text = tokens[index].text
        if text == opener:
            depth += 1
        elif opener == "<" and text and set(text) == {">"}:
            # The lexer preserves shift tokens, so nested generic closers may
            # arrive as one ``>>`` token.
            depth -= len(text)
            if depth <= 0:
                return index
        elif text == closer:
            depth -= 1
            if depth == 0:
                return index
    return None


def _depth_step(stack: list[str], text: str, *, angles: bool = True) -> None:
    if text in {"(", "[", "{"} or (angles and text == "<"):
        stack.append(text)
    elif angles and text and set(text) == {">"}:
        for _ in text:
            if stack and stack[-1] == "<":
                stack.pop()
    elif text in {")", "]", "}"} or (angles and text == ">"):
        expected = CLOSE_TO_OPEN.get(text)
        if stack and stack[-1] == expected:
            stack.pop()


def find_top(tokens: Sequence[Token], needle: str, *, angles: bool = True) -> int | None:
    stack: list[str] = []
    for index, token in enumerate(tokens):
        if not stack and token.text == needle:
            return index
        _depth_step(stack, token.text, angles=angles)
    return None


def find_top_any(
    tokens: Sequence[Token], needles: set[str], *, angles: bool = True
) -> int | None:
    stack: list[str] = []
    for index, token in enumerate(tokens):
        if not stack and token.text in needles:
            return index
        _depth_step(stack, token.text, angles=angles)
    return None


def split_top(
    tokens: Sequence[Token], separator: str, *, angles: bool = True
) -> list[list[Token]]:
    result: list[list[Token]] = []
    start = 0
    stack: list[str] = []
    for index, token in enumerate(tokens):
        if not stack and token.text == separator:
            result.append(list(tokens[start:index]))
            start = index + 1
        else:
            _depth_step(stack, token.text, angles=angles)
    result.append(list(tokens[start:]))
    return result


def is_wrapped(tokens: Sequence[Token], opener: str, closer: str) -> bool:
    if len(tokens) < 2 or tokens[0].text != opener:
        return False
    close_index = matching_index(tokens, 0)
    return close_index == len(tokens) - 1 and tokens[-1].text == closer


def join_tokens(tokens: Sequence[Token]) -> str:
    """Render a small non-type token sequence with stable, readable spacing."""
    out = ""
    previous = ""
    for token in tokens:
        text = token.text
        if not out:
            out = text
        elif text in {")", "]", "}", ">", ",", ";", "."}:
            out = out.rstrip() + text
        elif previous in {"(", "[", "{", "<", ".", "@"}:
            out += text
        elif text in {"(", "[", "<"}:
            out = out.rstrip() + text
        elif out.endswith(" "):
            out += text
        else:
            out += " " + text
        if text == ",":
            out += " "
        previous = text
    return out.strip()


def _qualified_name_end(tokens: Sequence[Token]) -> int:
    if not tokens or tokens[0].kind != "word":
        return 0
    index = 1
    while (
        index + 1 < len(tokens)
        and tokens[index].text == "."
        and tokens[index + 1].kind == "word"
    ):
        index += 2
    return index


def _expand_type_angle_closers(tokens: Sequence[Token]) -> list[Token]:
    """Split lexer shift tokens when they close nested generic arguments.

    The source lexer intentionally keeps ``>>`` intact for expressions.  In a
    type, however, a token such as the final ``>>`` in ``Outer<Inner<T>>`` is
    two delimiters.  Only split when every character can close an angle opener
    at the top of the delimiter stack, so fixed-array expressions such as
    ``T[8 >> 1]`` remain untouched.
    """

    expanded: list[Token] = []
    stack: list[str] = []
    for token in tokens:
        text = token.text
        if (
            len(text) > 1
            and set(text) == {">"}
            and len(stack) >= len(text)
            and all(item == "<" for item in stack[-len(text) :])
        ):
            for offset in range(len(text)):
                expanded.append(
                    Token(
                        token.kind,
                        ">",
                        token.start + offset,
                        token.start + offset + 1,
                    )
                )
                stack.pop()
            continue
        expanded.append(token)
        _depth_step(stack, text)
    return expanded


def render_type(tokens: Sequence[Token]) -> str:
    tokens = _expand_type_angle_closers(tokens)
    if not tokens:
        return ""

    arrow = find_top(tokens, "->")
    if arrow is not None:
        domain_tokens = tokens[:arrow]
        if is_wrapped(domain_tokens, "(", ")"):
            domain_parts = split_top(domain_tokens[1:-1], ",")
            domain = ", ".join(
                render_type(part) for part in domain_parts if part
            )
        else:
            domain = render_type(domain_tokens)
        result = render_type(tokens[arrow + 1 :])
        return f"function({domain}) returns ({result})"

    if tokens[0].text == "comptime":
        return "comptime " + render_type(tokens[1:])
    if tokens[0].text == "@":
        return "@" + render_type(tokens[1:])

    # Canonical function types must remain canonical on repeated migrations.
    # They differ from generic applications by using parentheses and a
    # `returns` clause, optionally separated by data-location/function
    # attributes.
    if len(tokens) >= 2 and tokens[0].text == "function" and tokens[1].text == "(":
        close_index = matching_index(tokens, 1)
        if close_index is not None:
            params = [
                render_type(part)
                for part in split_top(tokens[2:close_index], ",")
                if part
            ]
            cursor = close_index + 1
            attributes: list[str] = []
            while cursor < len(tokens) and tokens[cursor].text in MODIFIERS | LOCATIONS:
                attributes.append(tokens[cursor].text)
                cursor += 1
            result = ""
            if cursor < len(tokens) and tokens[cursor].text == "returns":
                if cursor + 1 < len(tokens) and tokens[cursor + 1].text == "(":
                    returns_close = matching_index(tokens, cursor + 1)
                    if returns_close is not None:
                        result_parts = [
                            render_type(part)
                            for part in split_top(
                                tokens[cursor + 2 : returns_close], ","
                            )
                            if part
                        ]
                        result = " returns (" + ", ".join(result_parts) + ")"
                        cursor = returns_close + 1
            if cursor == len(tokens):
                attributes_text = (
                    " " + " ".join(attributes) if attributes else ""
                )
                return (
                    "function("
                    + ", ".join(params)
                    + ")"
                    + attributes_text
                    + result
                )

    if is_wrapped(tokens, "(", ")"):
        elements = split_top(tokens[1:-1], ",")
        if len(elements) == 1 and not elements[0]:
            return "()"
        return "(" + ", ".join(render_type(element) for element in elements) + ")"

    name_end = _qualified_name_end(tokens)
    if name_end:
        name = "".join(token.text for token in tokens[:name_end])
        rest = tokens[name_end:]
        if rest and rest[0].text in {"(", "<"}:
            close = ")" if rest[0].text == "(" else ">"
            close_index = matching_index(rest, 0)
            if close_index is not None:
                arg_tokens = rest[1:close_index]
                mapping_arrow = (
                    find_top(arg_tokens, "=>")
                    if name == "mapping" and rest[0].text == "("
                    else None
                )
                args = (
                    [arg_tokens[:mapping_arrow], arg_tokens[mapping_arrow + 1 :]]
                    if mapping_arrow is not None
                    else split_top(arg_tokens, ",")
                )
                rendered_args = [render_type(arg) for arg in args if arg]
                suffix = rest[close_index + 1 :]
                if name == "mapping" and len(rendered_args) == 2:
                    base = f"mapping({rendered_args[0]} => {rendered_args[1]})"
                elif name in LOCATIONS and len(rendered_args) == 1:
                    base = f"{rendered_args[0]} {name}"
                else:
                    base = name + "<" + ", ".join(rendered_args) + ">"
                return base + _render_type_suffix(suffix)
        return name + _render_type_suffix(rest)

    return join_tokens(tokens)


def _render_type_suffix(tokens: Sequence[Token]) -> str:
    if not tokens:
        return ""
    if tokens[0].text == "[":
        close = matching_index(tokens, 0)
        if close is not None:
            inside = join_tokens(tokens[1:close])
            return "[" + inside + "]" + _render_type_suffix(tokens[close + 1 :])
    if len(tokens) == 1 and tokens[0].text in LOCATIONS:
        return " " + tokens[0].text
    return " " + join_tokens(tokens)


def render_return_type(tokens: Sequence[Token]) -> str:
    tokens = list(tokens)
    if is_wrapped(tokens, "(", ")"):
        elements = split_top(tokens[1:-1], ",")
        if len(elements) == 1 and not elements[0]:
            return ""
        return ", ".join(render_type(element) for element in elements)
    return render_type(tokens)


def render_trait_ref(tokens: Sequence[Token]) -> tuple[str, list[str]] | None:
    tokens = _expand_type_angle_closers(tokens)
    name_end = _qualified_name_end(tokens)
    if not name_end:
        return None
    name = "".join(token.text for token in tokens[:name_end])
    args: list[str] = []
    rest = tokens[name_end:]
    if rest and rest[0].text in {"(", "<"}:
        close_index = matching_index(rest, 0)
        if close_index is None:
            return None
        args = [
            render_type(part)
            for part in split_top(rest[1:close_index], ",")
            if part
        ]
        if close_index != len(rest) - 1:
            return None
    elif rest:
        return None
    return name, args


def render_predicate(tokens: Sequence[Token]) -> str | None:
    tokens = list(tokens)
    if is_wrapped(tokens, "(", ")"):
        tokens = tokens[1:-1]
    colon = find_top(tokens, ":")
    if colon is None:
        return None
    lhs = render_type(tokens[:colon])
    trait = render_trait_ref(tokens[colon + 1 :])
    if not lhs or trait is None:
        return None
    name, args = trait
    suffix = "<" + ", ".join(args) + ">" if args else ""
    return f"{lhs}: {name}{suffix}"


def render_predicates(tokens: Sequence[Token]) -> list[str]:
    tokens = list(tokens)
    if is_wrapped(tokens, "(", ")"):
        tokens = tokens[1:-1]
    result: list[str] = []
    for part in split_top(tokens, ","):
        if not part:
            continue
        predicate = render_predicate(part)
        if predicate is None:
            return []
        result.append(predicate)
    return result


def _statement_end(tokens: Sequence[Token], start: int) -> int | None:
    stack: list[str] = []
    for index in range(start, len(tokens)):
        text = tokens[index].text
        if not stack and text == ";":
            return index
        _depth_step(stack, text, angles=False)
    return None


def migrate_pragmas(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "pragma":
            continue
        end = _statement_end(tokens, index)
        if end is None or index + 1 >= end:
            continue
        body = tokens[index + 1 : end]
        if body and body[0].text == "solcore":
            continue
        name = body[0].text
        mapped = PRAGMA_NAMES.get(name)
        payload_start = 1
        if mapped is None:
            # Hyphenated legacy pragma names are lexed as
            # ``no``, ``-``, ``coverage``, ... rather than one token.
            for legacy_name, canonical_name in PRAGMA_NAMES.items():
                pieces = legacy_name.split("-")
                expected: list[str] = []
                for piece_index, piece in enumerate(pieces):
                    if piece_index:
                        expected.append("-")
                    expected.append(piece)
                if [token.text for token in body[: len(expected)]] == expected:
                    mapped = canonical_name
                    payload_start = len(expected)
                    break
        if mapped is None:
            # Solidity and abicoder pragmas already use their canonical namespace.
            continue
        payload = join_tokens(body[payload_start:])
        replacement = f"pragma solcore {mapped}"
        if payload:
            replacement += " " + payload
        replacement += ";"
        replacements.append((token.start, tokens[end].end, replacement))
    return replace_spans(source, replacements)


def migrate_imports(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "import":
            continue
        end = _statement_end(tokens, index)
        if end is None:
            continue
        body = list(tokens[index + 1 : end])
        if not body or body[0].kind == "string":
            continue
        # Already-canonical selective and namespace forms.
        if body[0].text in {"{", "*"}:
            continue

        brace = find_top(body, "{", angles=False)
        hiding = next(
            (i for i, item in enumerate(body) if item.text == "hiding"),
            None,
        )
        as_index = find_top(body, "as", angles=False)
        if brace is None and as_index is None and hiding is None:
            continue

        if brace is not None:
            # The old spelling has a dot immediately before the selector.
            if brace == 0 or body[brace - 1].text != ".":
                continue
            close = matching_index(body, brace)
            if close is None:
                continue
            path = join_tokens(body[: brace - 1])
            selected_parts = split_top(body[brace + 1 : close], ",", angles=False)
            selected = [join_tokens(part) for part in selected_parts if part]
            hidden: set[str] = set()
            if hiding is not None and hiding + 1 < len(body) and body[hiding + 1].text == "{":
                hidden_close = matching_index(body, hiding + 1)
                if hidden_close is not None:
                    hidden = {
                        join_tokens(part)
                        for part in split_top(
                            body[hiding + 2 : hidden_close], ",", angles=False
                        )
                        if part
                    }
            has_wildcard = "*" in selected
            names = [
                name for name in selected if name != "*" and name not in hidden
            ]
            if has_wildcard:
                if hidden:
                    raise ValueError(
                        "cannot migrate a wildcard import with `hiding`: "
                        "Core syntax has no wildcard-minus-names form; replace "
                        "it with an explicit selective import"
                    )
                # A wildcard selector without hiding opens the module's public
                # surface in canonical Core syntax.
                replacement = f"import {path};"
            elif names:
                replacement = f"import {{{', '.join(names)}}} from {path};"
            else:
                replacement = f"import {path};"
        elif as_index is not None:
            path = join_tokens(body[:as_index])
            alias = join_tokens(body[as_index + 1 :])
            if not path or not alias:
                continue
            replacement = f"import * as {alias} from {path};"
        else:
            continue
        replacements.append((token.start, tokens[end].end, replacement))
    return replace_spans(source, replacements)


def migrate_classic_bare_imports(
    source: str,
    path_limits: Mapping[str, int] | None = None,
) -> str:
    """Rewrite Classic bare namespace imports and their qualified uses.

    ``path_limits`` is used by repository migrations that contain a mixture of
    already-converted wildcard imports and Classic bare imports with the same
    path.  The public CLI omits it and converts every bare import in its input.
    """

    tokens = significant(source)
    limits = dict(path_limits) if path_limits is not None else None
    import_spans: list[tuple[int, int]] = []
    namespace_paths: list[tuple[list[str], str]] = []
    replacements: list[tuple[int, int, str]] = []

    for index, token in enumerate(tokens):
        if token.text != "import":
            continue
        end = _statement_end(tokens, index)
        if end is None:
            continue
        body = list(tokens[index + 1 : end])
        if not body:
            continue
        external = body[0].text == "@"
        cursor = 1 if external else 0
        if cursor >= len(body) or body[cursor].kind != "word":
            continue
        segments = [body[cursor].text]
        cursor += 1
        while cursor < len(body):
            if (
                body[cursor].text != "."
                or cursor + 1 >= len(body)
                or body[cursor + 1].kind != "word"
            ):
                break
            segments.append(body[cursor + 1].text)
            cursor += 2
        if cursor != len(body):
            continue
        path = ("@" if external else "") + ".".join(segments)
        if limits is not None:
            remaining = limits.get(path, 0)
            if remaining == 0:
                continue
            limits[path] = remaining - 1
        alias = segments[-1]
        replacements.append(
            (
                token.start,
                tokens[end].end,
                f"import * as {alias} from {path};",
            )
        )
        import_spans.append((token.start, tokens[end].end))
        visible = segments[1:] if external else segments
        namespace_paths.append((visible, alias))

    def inside_import(start: int, end: int) -> bool:
        return any(
            import_start <= start and end <= import_end
            for import_start, import_end in import_spans
        )

    seen_uses: set[tuple[int, int]] = set()
    for segments, alias in namespace_paths:
        if len(segments) < 2:
            continue
        width = len(segments) * 2 - 1
        for index in range(0, len(tokens) - width + 1):
            candidate = tokens[index : index + width]
            if any(
                candidate[offset * 2].kind != "word"
                or candidate[offset * 2].text != segment
                for offset, segment in enumerate(segments)
            ):
                continue
            if any(
                candidate[offset].text != "."
                for offset in range(1, width, 2)
            ):
                continue
            if index > 0 and tokens[index - 1].text == ".":
                continue
            start = candidate[0].start
            end = candidate[-1].end
            if inside_import(start, end) or (start, end) in seen_uses:
                continue
            seen_uses.add((start, end))
            replacements.append((start, end, alias))

    return replace_spans(source, replacements)


def _previous_boundary(tokens: Sequence[Token], index: int) -> int:
    cursor = index - 1
    while cursor >= 0 and tokens[cursor].text not in {";", "{", "}"}:
        cursor -= 1
    return cursor + 1


def _header_boundary(tokens: Sequence[Token], start: int) -> int | None:
    stack: list[str] = []
    for index in range(start, len(tokens)):
        text = tokens[index].text
        if not stack and text in {"{", ";"}:
            return index
        _depth_step(stack, text)
    return None


def _parse_forall_prefix(
    tokens: Sequence[Token],
) -> tuple[list[str], list[Token], bool]:
    tokens = list(tokens)
    forall = next((i for i, token in enumerate(tokens) if token.text == "forall"), None)
    if forall is None:
        rest = tokens
        if rest and rest[-1].text == "=>":
            rest = rest[:-1]
        return [], rest, False
    dot = find_top(tokens[forall + 1 :], ".", angles=False)
    if dot is None:
        return [], tokens, False
    dot += forall + 1
    binder = list(tokens[forall + 1 : dot])
    binder_constraints: list[Token] = []
    if find_top(binder, ":") is None:
        variables = [token.text for token in binder if token.kind == "word"]
    else:
        # Classic constrained forall spells predicates before the dot, e.g.
        # ``forall a:Eq, b:Show . function ...``.  The variables are implicit
        # in those predicates, so infer the lowercase leaves while excluding
        # named type/trait constructors and primitive types.
        variables = []
        for index, token in enumerate(binder):
            if token.kind != "word" or not token.text[:1].islower():
                continue
            if token.text in BUILTIN_TYPE_NAMES | LOCATIONS:
                continue
            if index + 1 < len(binder) and binder[index + 1].text in {"(", "<"}:
                continue
            if token.text not in variables:
                variables.append(token.text)
        binder_constraints = binder
    rest = tokens[dot + 1 :]
    if rest and rest[-1].text == "=>":
        rest = rest[:-1]
    if binder_constraints and rest:
        rest = [
            *binder_constraints,
            Token("symbol", ",", 0, 0),
            *rest,
        ]
    elif binder_constraints:
        rest = binder_constraints
    return variables, rest, True


def _with_preserved_comments(
    source: str, start: int, end: int, replacement: str
) -> str:
    comments = comments_in(source, start, end)
    if not comments:
        return replacement
    return "\n".join(comments) + "\n" + replacement


def migrate_data_declarations(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "data":
            continue
        end = _statement_end(tokens, index)
        if end is None or index + 1 >= end:
            continue
        body = list(tokens[index + 1 : end])
        if not body or body[0].kind != "word":
            continue
        name = body[0].text
        cursor = 1
        params: list[str] = []
        if cursor < len(body) and body[cursor].text == "(":
            close = matching_index(body, cursor)
            if close is None:
                continue
            params = [
                join_tokens(part)
                for part in split_top(body[cursor + 1 : close], ",")
                if part
            ]
            cursor = close + 1
        generic = "<" + ", ".join(params) + ">" if params else ""
        constructors: list[str] = []
        if cursor < len(body):
            if body[cursor].text != "=":
                continue
            ctor_tokens = body[cursor + 1 :]
            for ctor in split_top(ctor_tokens, "|"):
                if not ctor or ctor[0].kind != "word":
                    constructors = []
                    break
                ctor_name = ctor[0].text
                if len(ctor) == 1:
                    constructors.append(ctor_name)
                    continue
                if ctor[1].text != "(":
                    constructors = []
                    break
                close = matching_index(ctor, 1)
                if close != len(ctor) - 1:
                    constructors = []
                    break
                fields = [
                    render_type(part)
                    for part in split_top(ctor[2:close], ",")
                    if part
                ]
                constructors.append(
                    ctor_name + "(" + ", ".join(fields) + ")"
                )
        if cursor < len(body) and not constructors:
            # Do not "repair" a deliberately malformed data declaration.
            continue
        inside = ", ".join(constructors)
        replacement = f"enum {name}{generic} {{"
        if inside:
            replacement += " " + inside + " "
        replacement += "}"
        replacement = _with_preserved_comments(
            source, token.start, tokens[end].end, replacement
        )
        replacements.append((token.start, tokens[end].end, replacement))
    return replace_spans(source, replacements)


def _constructor_owner_candidates(
    tokens: Sequence[Token],
) -> tuple[dict[str, set[str]], set[int]]:
    """Collect constructor owners and their declaration tokens.

    Constructor use sites cannot be inferred safely from capitalization:
    declarations such as ``enum memory<a> { memory(word) }`` are valid source
    in the historical corpus.  Build the owner table structurally, exclude
    primitive constructors that intentionally remain unqualified, and keep
    complete enum/struct declarations out of the term rewrite.  Struct values
    use the implicit same-name constructor and therefore need qualification
    just like algebraic-data constructors.
    """

    candidates: dict[str, set[str]] = {}
    declarations: set[int] = set()
    for index, token in enumerate(tokens):
        if (
            token.text not in {"enum", "struct"}
            or index + 1 >= len(tokens)
            or tokens[index + 1].kind != "word"
        ):
            continue
        name = tokens[index + 1].text
        cursor = index + 2
        if cursor < len(tokens) and tokens[cursor].text == "<":
            close = matching_index(tokens, cursor)
            if close is None:
                continue
            cursor = close + 1
        if cursor >= len(tokens) or tokens[cursor].text != "{":
            continue
        close = matching_index(tokens, cursor)
        if close is None:
            continue
        declarations.update(range(index, close + 1))
        if token.text == "struct":
            if name not in BUILTIN_CONSTRUCTORS:
                candidates.setdefault(name, set()).add(name)
            continue
        constructors = split_top(tokens[cursor + 1 : close], ",")
        for constructor in constructors:
            if not (
                constructor
                and constructor[0].kind == "word"
                and (
                len(constructor) == 1
                or (len(constructor) > 1 and constructor[1].text == "(")
            )
            ):
                continue
            leaf = constructor[0].text
            if leaf != name or leaf in BUILTIN_CONSTRUCTORS:
                continue
            candidates.setdefault(leaf, set()).add(name)
    return candidates, declarations


def _dot_constructor_owner_candidates(
    tokens: Sequence[Token],
) -> dict[str, set[str]]:
    """Collect every enum constructor owner for Classic ``.Leaf`` uses.

    The older bare-constructor fallback intentionally considers only
    same-name constructors because an ordinary bare call can otherwise be
    indistinguishable from a function call.  A prefix dot is explicit Classic
    constructor syntax, so all enum variants are safe candidates.  Structs
    contribute their implicit same-name constructor as well.
    """

    candidates: dict[str, set[str]] = {}
    for index, token in enumerate(tokens):
        if (
            token.text not in {"enum", "struct"}
            or index + 1 >= len(tokens)
            or tokens[index + 1].kind != "word"
        ):
            continue
        owner = tokens[index + 1].text
        cursor = index + 2
        if cursor < len(tokens) and tokens[cursor].text == "<":
            close = matching_index(tokens, cursor)
            if close is None:
                continue
            cursor = close + 1
        if cursor >= len(tokens) or tokens[cursor].text != "{":
            continue
        close = matching_index(tokens, cursor)
        if close is None:
            continue
        if token.text == "struct":
            candidates.setdefault(owner, set()).add(owner)
            continue
        for constructor in split_top(tokens[cursor + 1 : close], ","):
            if not (
                constructor
                and constructor[0].kind == "word"
                and (
                    len(constructor) == 1
                    or (len(constructor) > 1 and constructor[1].text == "(")
                )
            ):
                continue
            candidates.setdefault(constructor[0].text, set()).add(owner)
    return candidates


def _unique_constructor_owners(
    candidates: Mapping[str, set[str]],
) -> dict[str, str]:
    return {
        leaf: next(iter(owners))
        for leaf, owners in candidates.items()
        if len(owners) == 1
    }


def _executable_regions(
    tokens: Sequence[Token],
) -> tuple[list[tuple[int, int]], set[int]]:
    """Return executable body ranges and nested declaration-header tokens."""

    bodies: list[tuple[int, int]] = []
    headers: set[int] = set()
    for index, token in enumerate(tokens):
        if token.text not in {"function", "constructor", "fallback", "lam"}:
            continue
        boundary = _header_boundary(tokens, index + 1)
        if boundary is None or tokens[boundary].text != "{":
            continue
        headers.update(range(index, boundary + 1))
        close = matching_index(tokens, boundary)
        if close is None:
            continue
        bodies.append((boundary + 1, close))
    return bodies, headers


def _declaration_surface_tokens(tokens: Sequence[Token]) -> set[int]:
    """Mark item syntax where an owner spelling is necessarily non-term."""

    marked: set[int] = set()
    statement_items = {"import", "export", "pragma", "alias", "type"}
    header_items = {
        "impl",
        "instance",
        "trait",
        "class",
        "contract",
        "interface",
        "library",
        "struct",
    }
    for index, token in enumerate(tokens):
        if token.text in statement_items:
            end = _statement_end(tokens, index)
            if end is not None:
                marked.update(range(index, end + 1))
        elif token.text in header_items:
            boundary = _header_boundary(tokens, index + 1)
            if boundary is not None:
                marked.update(range(index, boundary + 1))
    return marked


def _declared_callable_and_field_names(tokens: Sequence[Token]) -> set[str]:
    """Collect source-local term names that must beat constructor fallback."""

    names = {
        tokens[index + 1].text
        for index, token in enumerate(tokens[:-1])
        if token.text == "function" and tokens[index + 1].kind == "word"
    }
    for index in range(1, len(tokens) - 1):
        token = tokens[index]
        if token.kind != "word" or tokens[index + 1].text != ":":
            continue
        if tokens[index - 1].text in {"{", "}", ";"}:
            names.add(token.text)
    return names


def _declared_type_and_module_names(tokens: Sequence[Token]) -> set[str]:
    """Collect local namespaces that make a global constructor guess unsafe."""

    type_items = {
        "enum",
        "alias",
        "type",
        "struct",
        "contract",
        "interface",
        "library",
        "trait",
        "class",
    }
    names = {
        tokens[index + 1].text
        for index, token in enumerate(tokens[:-1])
        if token.text in type_items and tokens[index + 1].kind == "word"
    }
    for index, token in enumerate(tokens):
        if token.text != "import":
            continue
        end = _statement_end(tokens, index)
        if end is None:
            continue
        body = tokens[index + 1 : end]
        for cursor, item in enumerate(body[:-1]):
            if item.text == "as" and body[cursor + 1].kind == "word":
                names.add(body[cursor + 1].text)
        if body and body[0].kind == "word" and "from" not in {
            item.text for item in body
        }:
            names.add(body[0].text)
    return names


def _mark_type_region(
    tokens: Sequence[Token],
    start: int,
    limit: int,
    stops: set[str],
    marked: set[int],
) -> None:
    stack: list[str] = []
    for index in range(start, limit):
        text = tokens[index].text
        if not stack and text in stops:
            break
        marked.add(index)
        _depth_step(stack, text)


def _body_type_tokens(
    tokens: Sequence[Token], body_start: int, body_end: int
) -> set[int]:
    """Conservatively mark type-only regions inside an executable body."""

    marked: set[int] = set()
    for index in range(body_start, body_end):
        text = tokens[index].text
        if text == ":":
            # Local bindings and nested lambda parameters use name-first type
            # annotations.  Stop at the surrounding binding delimiter.
            _mark_type_region(
                tokens,
                index + 1,
                body_end,
                {"=", ":=", ";", ",", ")", "{"},
                marked,
            )
        elif text == "as":
            # ``as`` is exclusively a conversion and everything to its right
            # up to the enclosing expression delimiter is a type.
            _mark_type_region(
                tokens,
                index + 1,
                body_end,
                {
                    ";",
                    ",",
                    ")",
                    "]",
                    "}",
                    "{",
                    "+",
                    "-",
                    "*",
                    "/",
                    "%",
                    "==",
                    "!=",
                    "<=",
                    ">=",
                    "&&",
                    "||",
                },
                marked,
            )

    # Generic arguments are types even when they occur in a call expression.
    # Treat only balanced angle regions as such; a lone comparison remains an
    # expression and is therefore still migrated.
    for index in range(body_start, body_end):
        if tokens[index].text != "<":
            continue
        close = matching_index(tokens, index)
        if close is not None and close < body_end:
            marked.update(range(index + 1, close))
    return marked


def _body_shadowed_names(
    tokens: Sequence[Token], body_start: int, body_end: int, owners: set[str]
) -> set[str]:
    """Find direct local bindings that make an owner spelling ambiguous.

    This intentionally errs on the conservative side: if a parameter or
    direct ``let`` binding shadows an enum owner anywhere in the body, that
    owner is left untouched in the whole body instead of guessing which
    occurrences denoted the constructor.
    """

    shadowed: set[str] = set()
    brace = body_start - 1
    header_start = brace - 1
    while header_start >= 0 and tokens[header_start].text not in {
        "function",
        "constructor",
        "fallback",
        "lam",
        ";",
        "{",
        "}",
    }:
        header_start -= 1
    if header_start >= 0 and tokens[header_start].text in {
        "function",
        "constructor",
        "fallback",
        "lam",
    }:
        open_paren = next(
            (
                index
                for index in range(header_start + 1, brace)
                if tokens[index].text == "("
            ),
            None,
        )
        if open_paren is not None:
            close_paren = matching_index(tokens, open_paren)
            if close_paren is not None and close_paren < brace:
                for parameter in split_top(
                    tokens[open_paren + 1 : close_paren], ","
                ):
                    colon = find_top(parameter, ":")
                    binding = parameter if colon is None else parameter[:colon]
                    names = [token.text for token in binding if token.kind == "word"]
                    if names and names[-1] in owners:
                        shadowed.add(names[-1])

    for index in range(body_start, body_end - 1):
        if tokens[index].text != "let":
            continue
        cursor = index + 1
        if tokens[cursor].text == "comptime":
            cursor += 1
        stack: list[str] = []
        while cursor < body_end:
            text = tokens[cursor].text
            if not stack and text in {":", "=", ":=", ";"}:
                break
            if tokens[cursor].kind == "word" and text in owners:
                shadowed.add(text)
            _depth_step(stack, text, angles=False)
            cursor += 1

    for index in range(body_start, body_end):
        if tokens[index].text != "case":
            continue
        boundary = _header_boundary(tokens, index + 1)
        if boundary is None or boundary > body_end or tokens[boundary].text != "{":
            continue
        for cursor in range(index + 1, boundary):
            token = tokens[cursor]
            if token.kind != "word" or token.text not in owners:
                continue
            previous = tokens[cursor - 1].text
            following = tokens[cursor + 1].text
            if (
                previous not in {"case", "."}
                and following not in {"(", "."}
            ):
                shadowed.add(token.text)
    return shadowed


def migrate_qualified_constructors(
    source: str,
    global_owners: Mapping[str, str] | None = None,
) -> str:
    """Qualify term and pattern uses with local or globally unique owners."""

    if KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER in source:
        return source

    tokens = significant(source)
    local_candidates, declaration_tokens = _constructor_owner_candidates(tokens)
    local_owners = _unique_constructor_owners(local_candidates)
    constructor_owners = dict(global_owners or {})
    for namespace_name in _declared_type_and_module_names(tokens):
        constructor_owners.pop(namespace_name, None)
    # A declaration in this source is more precise than a repository-wide
    # spelling.  A locally ambiguous leaf must remain untouched.
    constructor_owners.update(local_owners)
    for leaf, candidates in local_candidates.items():
        if len(candidates) != 1:
            constructor_owners.pop(leaf, None)
    for term_name in _declared_callable_and_field_names(tokens):
        constructor_owners.pop(term_name, None)
    if not constructor_owners:
        return source
    constructor_leaves = set(constructor_owners)
    bodies, header_tokens = _executable_regions(tokens)
    nonterm_tokens = declaration_tokens | header_tokens | _declaration_surface_tokens(tokens)
    replacements: dict[int, tuple[int, int, str]] = {}
    body_tokens: set[int] = set()
    for body_start, body_end in bodies:
        body_tokens.update(range(body_start, body_end))
        type_tokens = _body_type_tokens(tokens, body_start, body_end)
        shadowed = _body_shadowed_names(
            tokens, body_start, body_end, constructor_leaves
        )
        for index in range(body_start, body_end):
            token = tokens[index]
            leaf = token.text
            if (
                token.kind != "word"
                or leaf not in constructor_owners
                or leaf in shadowed
                or index in nonterm_tokens
                or index in type_tokens
            ):
                continue
            previous = tokens[index - 1].text if index else ""
            following = tokens[index + 1].text if index + 1 < len(tokens) else ""
            if previous == "." or following == ".":
                continue
            if (
                leaf not in local_owners
                and following == "("
                and matching_index(tokens, index + 1) == index + 2
            ):
                # A cross-file `leaf()` is more likely a zero-argument
                # callable (notably `std.opcodes.address()`) than a payload
                # constructor.  Only a local declaration can disambiguate it.
                continue
            if (
                leaf not in local_owners
                and previous == "case"
                and following not in {"(", "as"}
            ):
                # A bare pattern identifier is ordinarily a new binding.  A
                # repository-wide constructor table does not provide enough
                # local type context to distinguish it from a nullary
                # constructor, so only source-local declarations may trigger
                # that rewrite.
                continue
            if (
                leaf not in local_owners
                and following not in {"(", "as"}
                and previous not in {"return", "=", ":=", "case"}
            ):
                # With only a cross-file owner table, a bare identifier in an
                # argument or nested pattern may be an ordinary binding (for
                # example `case Nat.Succ(m)`).  Limit nullary rewrites to
                # positions that explicitly introduce a value.
                continue
            owner = constructor_owners[leaf]
            replacements[index] = (
                token.start,
                token.end,
                f"{owner}.{leaf}",
            )

    # Negative parser fixtures can deliberately leave a function body
    # unbalanced, so no complete executable range exists.  Retain a narrow,
    # declaration-aware fallback for constructor calls in unequivocal
    # expression/pattern positions rather than "repairing" the malformed body.
    for index, token in enumerate(tokens):
        if (
            token.kind != "word"
            or token.text not in constructor_owners
            or index in body_tokens
            or index in nonterm_tokens
            or index + 1 >= len(tokens)
            or tokens[index + 1].text != "("
        ):
            continue
        previous = tokens[index - 1].text if index else ""
        if previous not in {"return", "=", ":=", "case"}:
            continue
        leaf = token.text
        if (
            leaf not in local_owners
            and matching_index(tokens, index + 1) == index + 2
        ):
            continue
        owner = constructor_owners[leaf]
        replacements[index] = (token.start, token.end, f"{owner}.{leaf}")
    return replace_spans(source, replacements.values())


_LEGACY_DOT_PREFIX_SYMBOLS = {
    "(",
    "[",
    "{",
    ",",
    ";",
    ":",
    "?",
    "=",
    ":=",
    "=>",
    "+",
    "-",
    "*",
    "/",
    "%",
    "**",
    "!",
    "==",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "&&",
    "||",
    "&",
    "|",
    "^",
    "<<",
    ">>",
}
_LEGACY_DOT_PREFIX_WORDS = {
    "case",
    "comptime",
    "default",
    "else",
    "if",
    "in",
    "let",
    "match",
    "not",
    "return",
}


def _is_legacy_dot_constructor(
    tokens: Sequence[Token], dot_index: int
) -> bool:
    """Distinguish prefix ``.Leaf`` from member or qualified access."""

    if (
        tokens[dot_index].text != "."
        or dot_index + 1 >= len(tokens)
        or tokens[dot_index + 1].kind != "word"
    ):
        return False
    if dot_index == 0:
        return True
    previous = tokens[dot_index - 1]
    if previous.kind == "word":
        return previous.text in _LEGACY_DOT_PREFIX_WORDS
    return previous.text in _LEGACY_DOT_PREFIX_SYMBOLS


def _source_line_column(source: str, offset: int) -> tuple[int, int]:
    line = source.count("\n", 0, offset) + 1
    line_start = source.rfind("\n", 0, offset) + 1
    return line, offset - line_start + 1


def migrate_legacy_dot_constructors(
    source: str,
    global_candidates: Mapping[str, set[str]] | None = None,
) -> str:
    """Rewrite Classic ``.Leaf`` constructors or reject unsafe guesses."""

    if KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER in source:
        return source

    tokens = significant(source)
    local_candidates = _dot_constructor_owner_candidates(tokens)
    candidates = {
        leaf: set(owners)
        for leaf, owners in (global_candidates or {}).items()
    }
    # A source-local declaration is more precise than the CLI-wide table,
    # while two local declarations with the same leaf remain ambiguous.
    candidates.update(
        {leaf: set(owners) for leaf, owners in local_candidates.items()}
    )

    replacements: list[tuple[int, int, str]] = []
    errors: list[str] = []
    for index, token in enumerate(tokens):
        if not _is_legacy_dot_constructor(tokens, index):
            continue
        leaf = tokens[index + 1].text
        owners = candidates.get(leaf, set())
        line, column = _source_line_column(source, token.start)
        location = f"line {line}, column {column}"
        if len(owners) == 1:
            # Insert the owner before the original dot instead of replacing
            # the whole span so comments between `.` and the leaf survive.
            owner = next(iter(owners))
            replacements.append((token.start, token.start, owner))
        elif owners:
            rendered = ", ".join(sorted(owners))
            errors.append(
                f"ambiguous legacy dot-constructor .{leaf} at {location}; "
                f"possible owners: {rendered}; qualify it explicitly"
            )
        else:
            errors.append(
                f"cannot resolve legacy dot-constructor .{leaf} at "
                f"{location}; include its enum declaration in this migration "
                "invocation or qualify it explicitly"
            )
    if errors:
        raise ValueError("; ".join(errors))
    return replace_spans(source, replacements)


def migrate_incomplete_data_heads(source: str) -> str:
    """Move malformed, unterminated ``data`` heads onto the new vocabulary.

    The fail corpus intentionally contains declarations without a terminating
    semicolon.  There is no complete declaration to faithfully rebuild, so keep
    the malformed tail intact while changing ``data Name(T)`` to
    ``enum Name<T>``.  The fixture remains erroneous for the same missing-body
    reason, but no longer exercises an unrelated removed keyword.
    """

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    brace_depth = 0
    for index, token in enumerate(tokens):
        if token.text == "}":
            brace_depth = max(0, brace_depth - 1)

        at_item_start = index == 0 or tokens[index - 1].text in {";", "}"}
        if (
            token.text == "data"
            and brace_depth == 0
            and at_item_start
            and index + 1 < len(tokens)
            and tokens[index + 1].kind == "word"
        ):
            replacements.append((token.start, token.end, "enum"))
            if index + 2 < len(tokens) and tokens[index + 2].text == "(":
                close = matching_index(tokens, index + 2)
                if close is not None:
                    replacements.append(
                        (tokens[index + 2].start, tokens[index + 2].end, "<")
                    )
                    replacements.append(
                        (tokens[close].start, tokens[close].end, ">")
                    )

        if token.text == "{":
            brace_depth += 1
    return replace_spans(source, replacements)


def migrate_aliases(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "type":
            continue
        end = _statement_end(tokens, index)
        if end is None or index + 1 >= end:
            continue
        body = list(tokens[index + 1 : end])
        equals = find_top(body, "=")
        if equals is None or not body or body[0].kind != "word":
            # ``type Name is T`` is already the new nominal value-type syntax.
            continue
        name = body[0].text
        cursor = 1
        params: list[str] = []
        if cursor < equals and body[cursor].text == "(":
            close = matching_index(body, cursor)
            if close is None or close >= equals:
                continue
            params = [
                join_tokens(part)
                for part in split_top(body[cursor + 1 : close], ",")
                if part
            ]
            cursor = close + 1
        if cursor != equals:
            continue
        generic = "<" + ", ".join(params) + ">" if params else ""
        rhs = render_type(body[equals + 1 :])
        if not rhs:
            continue
        replacement = f"alias {name}{generic} = {rhs};"
        replacement = _with_preserved_comments(
            source, token.start, tokens[end].end, replacement
        )
        replacements.append((token.start, tokens[end].end, replacement))
    return replace_spans(source, replacements)


def _head_predicate(tokens: Sequence[Token]) -> tuple[str, str, list[str]] | None:
    tokens = list(tokens)
    colon = find_top(tokens, ":")
    if colon is None:
        return None
    lhs = render_type(tokens[:colon])
    trait = render_trait_ref(tokens[colon + 1 :])
    if not lhs or trait is None:
        return None
    return lhs, trait[0], trait[1]


def migrate_classes(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "class":
            continue
        end = _header_boundary(tokens, index + 1)
        if end is None or tokens[end].text != "{":
            continue
        start_index = _previous_boundary(tokens, index)
        prefix = list(tokens[start_index:index])
        variables, constraints, had_forall = _parse_forall_prefix(prefix)
        if not had_forall:
            start_index = index
            constraints = []
        head = _head_predicate(tokens[index + 1 : end])
        if head is None:
            continue
        lhs, trait_name, trait_args = head
        if not variables:
            candidate_params = [lhs, *trait_args]
            if all(WORD_RE.fullmatch(param) for param in candidate_params):
                variables = candidate_params
        if not variables:
            continue
        predicates = render_predicates(constraints) if constraints else []
        if constraints and not predicates:
            continue
        replacement = f"trait {trait_name}<{', '.join(variables)}>"
        if predicates:
            replacement += " where " + ", ".join(predicates)
        replacement += " "
        start = tokens[start_index].start
        replacement = _with_preserved_comments(
            source, start, tokens[end].start, replacement
        )
        replacements.append((start, tokens[end].start, replacement))
    return replace_spans(source, replacements)


def migrate_instances(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "instance":
            continue
        end = _header_boundary(tokens, index + 1)
        if end is None or tokens[end].text != "{":
            continue
        start_index = _previous_boundary(tokens, index)
        prefix = list(tokens[start_index:index])
        is_default = any(item.text == "default" for item in prefix)
        prefix = [item for item in prefix if item.text != "default"]
        variables, constraints, had_forall = _parse_forall_prefix(prefix)
        recognized_prefix = had_forall or is_default or (
            constraints and constraints[-1].text == "=>"
        )
        if not recognized_prefix and prefix:
            # A predicate context without forall also ends in ``=>``; the
            # parser helper above strips it, so inspect the original prefix.
            recognized_prefix = any(item.text == "=>" for item in prefix)
        if not recognized_prefix:
            start_index = index
            constraints = []

        head_tokens = list(tokens[index + 1 : end])
        post_arrow = find_top(head_tokens, "=>")
        if post_arrow is not None:
            post_constraints = head_tokens[:post_arrow]
            head_tokens = head_tokens[post_arrow + 1 :]
            if constraints:
                constraints = [*constraints, Token("symbol", ",", 0, 0), *post_constraints]
            else:
                constraints = post_constraints

        head = _head_predicate(head_tokens)
        if head is None:
            continue
        lhs, trait_name, trait_args = head
        predicates = render_predicates(constraints) if constraints else []
        if constraints and not predicates:
            continue
        args = [lhs, *trait_args]
        replacement = "default impl" if is_default else "impl"
        if variables:
            replacement += "<" + ", ".join(variables) + ">"
        replacement += f" {trait_name}<{', '.join(args)}>"
        if predicates:
            replacement += " where " + ", ".join(predicates)
        replacement += " "
        start = tokens[start_index].start
        replacement = _with_preserved_comments(
            source, start, tokens[end].start, replacement
        )
        replacements.append((start, tokens[end].start, replacement))
    return replace_spans(source, replacements)


def render_params(tokens: Sequence[Token]) -> str:
    rendered: list[str] = []
    for part in split_top(tokens, ","):
        if not part:
            continue
        colon = find_top(part, ":")
        if colon is None:
            rendered.append(join_tokens(part))
            continue
        binding = join_tokens(part[:colon])
        ty = render_type(part[colon + 1 :])
        rendered.append(f"{binding}: {ty}")
    return ", ".join(rendered)


def _function_prefix(
    tokens: Sequence[Token], start_index: int, function_index: int
) -> tuple[list[str], list[Token], list[str], int]:
    prefix = list(tokens[start_index:function_index])
    modifiers = [token.text for token in prefix if token.text in MODIFIERS]
    without_modifiers = [token for token in prefix if token.text not in MODIFIERS]
    variables, constraints, had_forall = _parse_forall_prefix(without_modifiers)
    recognized = had_forall or bool(modifiers) or any(
        token.text == "=>" for token in without_modifiers
    )
    if not recognized:
        return [], [], [], function_index
    return variables, constraints, modifiers, start_index


def migrate_functions(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "function":
            continue
        if index + 2 >= len(tokens) or tokens[index + 1].kind != "word":
            continue
        name_token = tokens[index + 1]
        cursor = index + 2
        # A canonical generic function has already been migrated.
        if tokens[cursor].text == "<":
            continue
        if tokens[cursor].text != "(":
            continue
        close = matching_index(tokens, cursor)
        if close is None:
            continue
        end = _header_boundary(tokens, close + 1)
        if end is None:
            continue
        tail = list(tokens[close + 1 : end])
        if any(item.text in {"returns", "where"} for item in tail):
            continue

        candidate_start = _previous_boundary(tokens, index)
        variables, constraints, modifiers, start_index = _function_prefix(
            tokens, candidate_start, index
        )
        arrow = find_top(tail, "->")
        return_tokens: list[Token] = []
        if arrow is not None:
            return_tokens = tail[arrow + 1 :]
            # Modifiers after the parameter list are already canonical; retain
            # them if a partially migrated file still has an old return arrow.
            modifiers.extend(
                item.text for item in tail[:arrow] if item.text in MODIFIERS
            )
        elif tail:
            modifiers.extend(item.text for item in tail if item.text in MODIFIERS)

        predicates = render_predicates(constraints) if constraints else []
        if constraints and not predicates:
            continue
        params = render_params(tokens[cursor + 1 : close])
        replacement = f"function {name_token.text}"
        if variables:
            replacement += "<" + ", ".join(variables) + ">"
        replacement += f"({params})"
        if modifiers:
            replacement += " " + " ".join(dict.fromkeys(modifiers))
        if return_tokens:
            replacement += " returns (" + render_return_type(return_tokens) + ")"
        if predicates:
            replacement += " where " + ", ".join(predicates)
        replacement += " "

        start = tokens[start_index].start
        replacement = _with_preserved_comments(
            source, start, tokens[end].start, replacement
        )
        replacements.append((start, tokens[end].start, replacement))
    return replace_spans(source, replacements)


def migrate_lambdas(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "lam" or index + 1 >= len(tokens):
            continue
        open_index = index + 1
        if tokens[open_index].text != "(":
            continue
        close = matching_index(tokens, open_index)
        if close is None:
            continue
        end = _header_boundary(tokens, close + 1)
        if end is None or tokens[end].text != "{":
            continue
        tail = list(tokens[close + 1 : end])
        if any(item.text == "returns" for item in tail):
            continue
        arrow = find_top(tail, "->")
        params = render_params(tokens[open_index + 1 : close])
        replacement = f"lam ({params})"
        if arrow is not None:
            replacement += " returns (" + render_return_type(tail[arrow + 1 :]) + ")"
        replacement += " "
        replacements.append((token.start, tokens[end].start, replacement))
    return replace_spans(source, replacements)


def migrate_special_functions(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text not in {"constructor", "fallback"}:
            continue
        if index + 1 >= len(tokens) or tokens[index + 1].text != "(":
            continue
        open_index = index + 1
        close = matching_index(tokens, open_index)
        if close is None:
            continue
        end = _header_boundary(tokens, close + 1)
        if end is None:
            continue
        tail = list(tokens[close + 1 : end])
        if any(item.text == "returns" for item in tail):
            continue
        start_index = _previous_boundary(tokens, index)
        prefix = list(tokens[start_index:index])
        modifiers = [item.text for item in prefix if item.text in MODIFIERS]
        if prefix and len(modifiers) != len(prefix):
            start_index = index
            modifiers = []
        arrow = find_top(tail, "->")
        if arrow is not None:
            modifiers.extend(
                item.text for item in tail[:arrow] if item.text in MODIFIERS
            )
        else:
            modifiers.extend(item.text for item in tail if item.text in MODIFIERS)
        params = render_params(tokens[open_index + 1 : close])
        replacement = f"{token.text}({params})"
        if modifiers:
            replacement += " " + " ".join(dict.fromkeys(modifiers))
        if arrow is not None:
            replacement += " returns (" + render_return_type(tail[arrow + 1 :]) + ")"
        replacement += " "
        replacements.append(
            (tokens[start_index].start, tokens[end].start, replacement)
        )
    return replace_spans(source, replacements)


def migrate_incomplete_arrows(source: str) -> str:
    """Rewrite return arrows in deliberately incomplete function headers.

    Complete declarations are handled by ``migrate_functions`` and
    ``migrate_lambdas``.  This fallback is for negative parser fixtures whose
    missing ``)``/body/semicolon prevents those structural passes from finding
    a whole header.  The original structural error is retained.
    """

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "->" or index + 1 >= len(tokens):
            continue
        stack: list[str] = []
        end = index + 1
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if cursor > index + 1 and not stack and text in {"{", "}", ";"}:
                break
            _depth_step(stack, text)
            end = cursor + 1
        return_tokens = list(tokens[index + 1 : end])
        if not return_tokens:
            continue
        rendered = render_return_type(return_tokens)
        replacements.append(
            (token.start, tokens[end - 1].end, f"returns ({rendered})")
        )
    return replace_spans(source, replacements)


def migrate_let_types(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "let":
            continue
        stack: list[str] = []
        colon: int | None = None
        end: int | None = None
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if not stack and text == ":":
                colon = cursor
            if not stack and text in {"=", ";"}:
                end = cursor
                break
            _depth_step(stack, text, angles=False)
        if colon is None or end is None or colon >= end:
            continue
        binding_tokens = list(tokens[index + 1 : colon])
        type_tokens = list(tokens[colon + 1 : end])
        if not type_tokens:
            continue
        comptime = type_tokens[0].text == "comptime"
        if comptime:
            type_tokens = type_tokens[1:]
        already_comptime = binding_tokens and binding_tokens[0].text == "comptime"
        binding = join_tokens(binding_tokens)
        ty = render_type(type_tokens)
        if not binding or not ty:
            continue
        replacement = "let "
        if comptime and not already_comptime:
            replacement += "comptime "
        replacement += f"{binding}: {ty}"
        if tokens[end].text == "=":
            replacement += " "
        replacements.append((token.start, tokens[end].start, replacement))
    return replace_spans(source, replacements)


def migrate_field_types(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != ":" or index == 0 or tokens[index - 1].kind != "word":
            continue
        name_index = index - 1
        before = tokens[name_index - 1].text if name_index else None
        if before not in {None, "{", "}", ";"}:
            continue
        end = _header_boundary(tokens, index + 1)
        if end is None or tokens[end].text not in {"=", ";"}:
            continue
        ty = render_type(tokens[index + 1 : end])
        if not ty:
            continue
        replacements.append((token.end, tokens[end].start, " " + ty))
    return replace_spans(source, replacements)


def _match_arm_starts(tokens: Sequence[Token], start: int, end: int) -> list[int]:
    result: list[int] = []
    stack: list[str] = []
    for index in range(start, end):
        text = tokens[index].text
        if not stack and text == "|":
            # A real arm separator has a top-level fat arrow before the next
            # statement terminator or candidate arm.
            probe_stack: list[str] = []
            for probe in range(index + 1, end):
                probe_text = tokens[probe].text
                if not probe_stack and probe_text == "=>":
                    result.append(index)
                    break
                if not probe_stack and probe_text in {";", "|"}:
                    break
                _depth_step(probe_stack, probe_text, angles=False)
        _depth_step(stack, text, angles=False)
    return result


def migrate_one_match(source: str) -> tuple[str, bool]:
    tokens = significant(source)
    candidates: list[tuple[int, int, int]] = []
    for index, token in enumerate(tokens):
        if token.text != "match":
            continue
        brace = _header_boundary(tokens, index + 1)
        if brace is None or tokens[brace].text != "{":
            continue
        close = matching_index(tokens, brace)
        if close is None:
            continue
        already_parenthesized = (
            index + 1 < brace
            and tokens[index + 1].text == "("
            and matching_index(tokens, index + 1) == brace - 1
        )
        if already_parenthesized and any(
            item.text in {"case", "default"} for item in tokens[brace + 1 : close]
        ):
            continue
        candidates.append((index, brace, close))
    if not candidates:
        return source, False

    # Last old match first.  Nested matches are therefore canonical before an
    # enclosing arm body is wrapped.
    index, brace, close = candidates[-1]
    arm_starts = _match_arm_starts(tokens, brace + 1, close)
    if not arm_starts:
        return source, False
    leading = source[tokens[brace].end : tokens[arm_starts[0]].start].strip()
    rendered_arms: list[str] = []
    for position, arm_start in enumerate(arm_starts):
        arm_end = arm_starts[position + 1] if position + 1 < len(arm_starts) else close
        arrow_relative = find_top(tokens[arm_start + 1 : arm_end], "=>", angles=False)
        if arrow_relative is None:
            return source, False
        arrow = arm_start + 1 + arrow_relative
        pattern_tokens = list(tokens[arm_start + 1 : arrow])
        body_start = tokens[arrow].end
        body_end = tokens[arm_end].start
        body = source[body_start:body_end].strip()
        patterns = [part for part in split_top(pattern_tokens, ",", angles=False) if part]
        is_default = bool(patterns) and all(
            len(part) == 1 and part[0].text == "_" for part in patterns
        )
        if is_default:
            head = "default"
        elif len(patterns) > 1:
            head = "case (" + ", ".join(join_tokens(part) for part in patterns) + ")"
        else:
            head = "case " + join_tokens(pattern_tokens)
        # Keep arm boundaries on their own lines.  Appending the next arm or
        # closing brace to a ``//`` comment would silently comment it out.
        rendered_arms.append(f"{head} {{\n{body}\n}}")
    scrutinee = source[tokens[index].end : tokens[brace].start].strip()
    if (
        scrutinee.startswith("(")
        and scrutinee.endswith(")")
        and tokens[index + 1].text == "("
        and matching_index(tokens, index + 1) == brace - 1
    ):
        scrutinee = source[tokens[index + 1].end : tokens[brace - 1].start].strip()
    replacement = (
        f"match ({scrutinee}) {{\n"
        + (leading + "\n" if leading else "")
        + "\n".join(rendered_arms)
        + "\n}"
    )
    return (
        source[: tokens[index].start] + replacement + source[tokens[close].end :],
        True,
    )


def migrate_matches(source: str) -> str:
    # Each iteration retokenizes because wrapping an arm changes brace nesting.
    for _ in range(10_000):
        source, changed = migrate_one_match(source)
        if not changed:
            return source
    raise RuntimeError("match migration did not converge")


def remove_match_trailing_semicolons(source: str) -> str:
    """Drop the classic expression terminator from canonical match statements."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "match" or index + 1 >= len(tokens):
            continue
        if tokens[index + 1].text != "(":
            continue
        scrutinee_close = matching_index(tokens, index + 1)
        if (
            scrutinee_close is None
            or scrutinee_close + 1 >= len(tokens)
            or tokens[scrutinee_close + 1].text != "{"
        ):
            continue
        body_close = matching_index(tokens, scrutinee_close + 1)
        if (
            body_close is not None
            and body_close + 1 < len(tokens)
            and tokens[body_close + 1].text == ";"
        ):
            semicolon = tokens[body_close + 1]
            replacements.append((semicolon.start, semicolon.end, ""))
    return replace_spans(source, replacements)


def migrate_if_expressions(source: str) -> str:
    for _ in range(10_000):
        tokens = significant(source)
        chosen: tuple[int, int, int, int] | None = None
        for index in range(len(tokens) - 1, -1, -1):
            if tokens[index].text != "if":
                continue
            stack: list[str] = []
            then_index: int | None = None
            else_index: int | None = None
            for cursor in range(index + 1, len(tokens)):
                text = tokens[cursor].text
                if not stack and text == "then":
                    then_index = cursor
                    break
                if not stack and text in {"{", ";"}:
                    break
                _depth_step(stack, text, angles=False)
            if then_index is None:
                continue
            stack = []
            for cursor in range(then_index + 1, len(tokens)):
                text = tokens[cursor].text
                if not stack and text == "else":
                    else_index = cursor
                    break
                _depth_step(stack, text, angles=False)
            if else_index is None:
                continue
            stack = []
            end_index = len(tokens)
            for cursor in range(else_index + 1, len(tokens)):
                text = tokens[cursor].text
                if not stack and text in {";", ",", ")", "]", "}", "then", "else"}:
                    end_index = cursor
                    break
                _depth_step(stack, text, angles=False)
            if end_index == else_index + 1:
                continue
            chosen = (index, then_index, else_index, end_index)
            break
        if chosen is None:
            return source
        index, then_index, else_index, end_index = chosen
        condition = source[tokens[index].end : tokens[then_index].start].strip()
        then_expr = source[tokens[then_index].end : tokens[else_index].start].strip()
        end_pos = (
            tokens[end_index].start if end_index < len(tokens) else len(source)
        )
        else_expr = source[tokens[else_index].end : end_pos].strip()
        replacement = f"({condition} ? {then_expr} : {else_expr})"
        source = source[: tokens[index].start] + replacement + source[end_pos:]
    raise RuntimeError("if-expression migration did not converge")


def migrate_condition_parentheses(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text not in {"if", "while"} or index + 1 >= len(tokens):
            continue
        if tokens[index + 1].text == "(":
            continue
        boundary = _header_boundary(tokens, index + 1)
        if boundary is None or tokens[boundary].text != "{":
            continue
        condition = source[token.end : tokens[boundary].start].strip()
        if not condition or "then" in {item.text for item in tokens[index + 1 : boundary]}:
            continue
        replacements.append(
            (token.end, tokens[boundary].start, f" ({condition}) ")
        )
    return replace_spans(source, replacements)


def _inside_function_parameter_list(tokens: Sequence[Token], colon_index: int) -> bool:
    depth = 0
    open_index: int | None = None
    for index in range(colon_index - 1, -1, -1):
        text = tokens[index].text
        if text == ")":
            depth += 1
        elif text == "(":
            if depth == 0:
                open_index = index
                break
            depth -= 1
    if open_index is None or open_index == 0:
        return False
    close_index = matching_index(tokens, open_index)
    if (
        close_index is not None
        and close_index + 1 < len(tokens)
        and tokens[close_index + 1].text == "returns"
    ):
        # Keep parameter spelling intact in malformed-signature recovery
        # fixtures that intentionally omit the leading `function` keyword.
        return True
    before = tokens[open_index - 1].text
    if before in {"lam", "constructor", "fallback", "returns"}:
        return True
    if (
        tokens[open_index - 1].kind == "word"
        and open_index >= 2
        and tokens[open_index - 2].text == "function"
    ):
        return True
    # ``function name<T>(...)``: walk only across the immediately preceding
    # generic argument list, not across arbitrary calls in a function body.
    if before == ">":
        angle_depth = 1
        scan = open_index - 2
        while scan >= 0:
            if tokens[scan].text == ">":
                angle_depth += 1
            elif tokens[scan].text == "<":
                angle_depth -= 1
                if angle_depth == 0:
                    return (
                        scan >= 2
                        and tokens[scan - 1].kind == "word"
                        and tokens[scan - 2].text == "function"
                    )
            scan -= 1
    return False


def _is_ternary_colon(tokens: Sequence[Token], colon_index: int) -> bool:
    stack: list[str] = []
    for index in range(colon_index - 1, -1, -1):
        text = tokens[index].text
        if text in {")", "]", "}"}:
            stack.append(text)
        elif text in {"(", "[", "{"}:
            if stack:
                stack.pop()
            else:
                break
        elif not stack and text == "?":
            return True
        elif not stack and text in {";", "=", "return", "case"}:
            break
    return False


def migrate_expression_annotations(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != ":" or index == 0 or index + 1 >= len(tokens):
            continue
        if tokens[index + 1].text in {"#", "?"}:
            # Rust-style formatting placeholders can appear in diagnostic
            # strings handed to the migrator by an embedding tool.
            continue
        if tokens[index + 1].kind != "word" and tokens[index + 1].text not in {
            "(",
            "@",
        }:
            continue
        if _inside_function_parameter_list(tokens, index):
            continue
        if _is_ternary_colon(tokens, index):
            continue
        # Let binding types and field declarations are canonical uses of colon.
        statement_start = index - 1
        while statement_start >= 0 and tokens[statement_start].text not in {
            ";",
            "{",
            "}",
        }:
            statement_start -= 1
        statement_prefix = {
            item.text for item in tokens[statement_start + 1 : index]
        }
        if statement_prefix & {"export", "import", "pragma"}:
            continue
        if "where" in statement_prefix:
            continue
        if "let" in statement_prefix:
            let_index = next(
                cursor
                for cursor in range(statement_start + 1, index)
                if tokens[cursor].text == "let"
            )
            equals_before_colon = any(
                tokens[cursor].text == "="
                for cursor in range(let_index + 1, index)
            )
            if not equals_before_colon:
                continue
        previous = tokens[index - 1]
        if (
            previous.kind == "word"
            and statement_start + 1 == index - 1
        ):
            # A name-first field declaration begins the statement and ends at
            # ``=`` or ``;``.  Its colon is canonical rather than an expression
            # annotation.
            field_end = _header_boundary(tokens, index + 1)
            if field_end is not None and tokens[field_end].text in {"=", ";"}:
                continue
        if previous.kind == "word" and index >= 2 and tokens[index - 2].text in {
            "trait",
            "impl",
        }:
            continue

        stack: list[str] = []
        end = index + 1
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if cursor > index + 1 and not stack and text in {
                ",",
                ";",
                ")",
                "]",
                "}",
                "+",
                "-",
                "*",
                "/",
                "==",
                "!=",
                "&&",
                "||",
            }:
                break
            _depth_step(stack, text)
            end = cursor + 1
        type_tokens = list(tokens[index + 1 : end])
        if not type_tokens:
            continue
        rendered = render_type(type_tokens)
        if not rendered:
            continue
        replacements.append(
            (token.start, tokens[end - 1].end, " as " + rendered)
        )
    return replace_spans(source, replacements)


def migrate_source(
    source: str,
    global_constructor_owners: Mapping[str, str] | None = None,
    global_dot_constructor_candidates: Mapping[str, set[str]] | None = None,
) -> str:
    if KEEP_LEGACY_NEGATIVE_MARKER in source:
        return source
    passes = (
        migrate_pragmas,
        migrate_imports,
        migrate_data_declarations,
        migrate_incomplete_data_heads,
        migrate_aliases,
        migrate_classes,
        migrate_instances,
        migrate_functions,
        migrate_lambdas,
        migrate_special_functions,
        migrate_let_types,
        migrate_incomplete_arrows,
        migrate_field_types,
        migrate_matches,
        remove_match_trailing_semicolons,
        migrate_if_expressions,
        migrate_condition_parentheses,
        migrate_expression_annotations,
    )
    for _ in range(8):
        before = source
        for migration in passes:
            source = migration(source)
        source = migrate_legacy_dot_constructors(
            source, global_dot_constructor_candidates
        )
        source = migrate_qualified_constructors(
            source, global_constructor_owners
        )
        if source == before:
            return source
    raise RuntimeError("syntax migration did not reach a fixed point")


def collect_global_constructor_owners(sources: Iterable[str]) -> dict[str, str]:
    """Build the unambiguous constructor table shared by one CLI invocation."""

    merged: dict[str, set[str]] = {}
    for source in sources:
        canonical = migrate_incomplete_data_heads(
            migrate_data_declarations(source)
        )
        candidates, _ = _constructor_owner_candidates(significant(canonical))
        for leaf, owners in candidates.items():
            merged.setdefault(leaf, set()).update(owners)
    return _unique_constructor_owners(merged)


def collect_global_dot_constructor_candidates(
    sources: Iterable[str],
) -> dict[str, set[str]]:
    """Build the CLI-wide owner table used for Classic ``.Leaf`` syntax."""

    merged: dict[str, set[str]] = {}
    for source in sources:
        canonical = migrate_incomplete_data_heads(
            migrate_data_declarations(source)
        )
        candidates = _dot_constructor_owner_candidates(significant(canonical))
        for leaf, owners in candidates.items():
            merged.setdefault(leaf, set()).update(owners)
    return merged


def source_paths(arguments: Sequence[str]) -> list[Path]:
    paths: set[Path] = set()
    for argument in arguments:
        path = Path(argument)
        if path.is_dir():
            paths.update(path.rglob("*.sol"))
            paths.update(path.rglob("*.solc"))
        elif path.suffix in {".sol", ".solc"}:
            paths.add(path)
        else:
            raise ValueError(f"not a Solcore source path: {path}")
    return sorted(paths)


def rust_source_paths(arguments: Sequence[str]) -> list[Path]:
    paths: set[Path] = set()
    for argument in arguments:
        path = Path(argument)
        if path.is_dir():
            paths.update(path.rglob("*.rs"))
        elif path.suffix == ".rs":
            paths.add(path)
        elif path.suffix in {".sol", ".solc"}:
            # In --rust-strings mode an explicit Solcore source is an owner
            # table seed, not an edit target.
            continue
        else:
            raise ValueError(f"not a Rust source path: {path}")
    return sorted(paths)


def owner_source_paths(arguments: Sequence[str]) -> list[Path]:
    """Find Solcore sources used to seed a cross-file constructor table."""

    paths: set[Path] = set()
    for argument in arguments:
        path = Path(argument)
        if path.is_dir():
            paths.update(path.rglob("*.sol"))
            paths.update(path.rglob("*.solc"))
        elif path.suffix in {".sol", ".solc"}:
            paths.add(path)
    return sorted(paths)


def _rust_block_comment_end(source: str, start: int) -> int:
    """Return the end of a possibly nested Rust block comment."""

    depth = 1
    cursor = start + 2
    while cursor < len(source) and depth:
        if source.startswith("/*", cursor):
            depth += 1
            cursor += 2
        elif source.startswith("*/", cursor):
            depth -= 1
            cursor += 2
        else:
            cursor += 1
    return cursor


def _rust_char_end(source: str, start: int) -> int | None:
    """Recognize a Rust character literal without mistaking lifetimes for one."""

    cursor = start + 1
    if cursor >= len(source) or source[cursor] in {"\r", "\n", "'"}:
        return None
    if source[cursor] == "\\":
        cursor += 1
        if cursor >= len(source):
            return None
        if source[cursor] == "u" and cursor + 1 < len(source) and source[cursor + 1] == "{":
            close = source.find("}", cursor + 2)
            if close < 0:
                return None
            cursor = close + 1
        elif source[cursor] == "x":
            cursor += 3
        else:
            cursor += 1
    else:
        cursor += 1
    return cursor + 1 if cursor < len(source) and source[cursor] == "'" else None


def _rust_raw_literal(
    source: str, start: int
) -> tuple[int, int, int] | None:
    """Return `(body_start, body_end, literal_end)` for a Rust raw string."""

    if start and (source[start - 1].isalnum() or source[start - 1] == "_"):
        return None
    marker = next(
        (
            candidate
            for candidate in ("br", "cr", "r")
            if source.startswith(candidate, start)
        ),
        None,
    )
    if marker is None:
        return None
    cursor = start + len(marker)
    while cursor < len(source) and source[cursor] == "#":
        cursor += 1
    if cursor >= len(source) or source[cursor] != '"':
        return None
    hashes = source[start + len(marker) : cursor]
    body_start = cursor + 1
    terminator = '"' + hashes
    body_end = source.find(terminator, body_start)
    if body_end < 0:
        return body_start, len(source), len(source)
    return body_start, body_end, body_end + len(terminator)


def _rust_ordinary_literal(
    source: str, start: int
) -> tuple[int, int, int] | None:
    """Return `(body_start, body_end, literal_end)` for a Rust string."""

    if start and (source[start - 1].isalnum() or source[start - 1] == "_"):
        return None
    quote = start
    if source.startswith(("b\"", "c\""), start):
        quote += 1
    elif source[start] != '"':
        return None
    literal_end = _scan_quoted(source, quote, '"')
    body_end = max(quote + 1, literal_end - 1)
    return quote + 1, body_end, literal_end


def _decode_rust_ordinary_body(body: str) -> str | None:
    """Decode the JSON-compatible subset of Rust ordinary string escapes.

    Embedded test programs overwhelmingly use ``\n``, quotes, and ordinary
    backslash escapes, all shared with JSON.  Returning ``None`` for a
    Rust-specific escape lets the caller retain the older lexical fallback
    instead of guessing at string contents.
    """

    try:
        decoded = json.loads('"' + body + '"')
    except (json.JSONDecodeError, UnicodeDecodeError):
        return None
    return decoded if isinstance(decoded, str) else None


def _encode_rust_ordinary_body(body: str) -> str:
    return json.dumps(body, ensure_ascii=False)[1:-1]


def _rust_solcore_literals(source: str) -> list[tuple[int, int, bool]]:
    """Locate Rust string bodies that look like embedded Solcore programs.

    The boolean result field is true for a raw string. Ordinary strings are
    decoded before detection so an escaped newline terminates a Solcore line
    comment and does not hide the rest of the embedded program.
    """

    literals: list[tuple[int, int, bool]] = []
    cursor = 0
    while cursor < len(source):
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", cursor):
            cursor = _rust_block_comment_end(source, cursor)
            continue
        if source[cursor] == "'":
            char_end = _rust_char_end(source, cursor)
            if char_end is not None:
                cursor = char_end
                continue

        literal = _rust_raw_literal(source, cursor)
        is_raw = literal is not None
        if not is_raw:
            literal = _rust_ordinary_literal(source, cursor)
        if literal is None:
            cursor += 1
            continue

        body_start, body_end, literal_end = literal
        body = source[body_start:body_end]
        detected_body = (
            body
            if is_raw
            else (_decode_rust_ordinary_body(body) or body)
        )
        if SOLCORE_LITERAL_HINT_RE.search(detected_body) is not None:
            literals.append((body_start, body_end, is_raw))
        cursor = literal_end
    return literals


def _rust_solcore_literal_spans(source: str) -> list[tuple[int, int]]:
    return [
        (body_start, body_end)
        for body_start, body_end, _ in _rust_solcore_literals(source)
    ]


def migrate_rust_strings(
    source: str,
    global_constructor_owners: Mapping[str, str] | None = None,
    global_dot_constructor_candidates: Mapping[str, set[str]] | None = None,
    *,
    classic_bare_imports: bool = False,
) -> str:
    """Migrate Solcore programs embedded in Rust string literals.

    Literals are rewritten only when they contain a language-surface keyword,
    leaving unrelated regex, snapshot, and prose literals byte-for-byte
    unchanged.  Ordinary strings are transformed in their escaped spelling so
    the surrounding Rust source and existing escapes stay intact.
    """

    if KEEP_RUST_FILE_MARKER in source:
        return source

    replacements: list[tuple[int, int, str]] = []
    for body_start, body_end, is_raw in _rust_solcore_literals(source):
        encoded_body = source[body_start:body_end]
        decoded_body = (
            encoded_body
            if is_raw
            else _decode_rust_ordinary_body(encoded_body)
        )
        body = decoded_body if decoded_body is not None else encoded_body
        if classic_bare_imports:
            body = migrate_classic_bare_imports(body)
        migrated = migrate_source(
            body,
            global_constructor_owners,
            global_dot_constructor_candidates,
        )
        if decoded_body is not None and not is_raw:
            migrated = _encode_rust_ordinary_body(migrated)
        if migrated != encoded_body:
            replacements.append((body_start, body_end, migrated))

    return replace_spans(source, replacements)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="report files that need migration without writing them",
    )
    parser.add_argument(
        "--rust-strings",
        action="store_true",
        help=(
            "migrate Solcore programs inside Rust string literals; Solcore "
            "files under directory arguments seed the shared constructor table"
        ),
    )
    parser.add_argument(
        "--classic-bare-imports",
        action="store_true",
        help=(
            "treat every `import M;` in the selected Classic sources as a "
            "namespace import and rewrite it to `import * as M from M;`"
        ),
    )
    parser.add_argument(
        "paths",
        nargs="+",
        help="source files or directories (mode selects the edited file kind)",
    )
    args = parser.parse_args()
    try:
        paths = (
            rust_source_paths(args.paths)
            if args.rust_strings
            else source_paths(args.paths)
        )
    except ValueError as error:
        parser.error(str(error))

    failures: list[tuple[Path, Exception]] = []
    originals: dict[Path, str] = {}
    for path in paths:
        try:
            originals[path] = path.read_text()
        except Exception as error:
            failures.append((path, error))

    owner_sources: list[str] = []
    if args.rust_strings:
        for path in owner_source_paths(args.paths):
            try:
                source = path.read_text()
                owner_sources.append(
                    migrate_classic_bare_imports(source)
                    if args.classic_bare_imports
                    else source
                )
            except Exception as error:
                failures.append((path, error))
        for original in originals.values():
            for start, end, is_raw in _rust_solcore_literals(original):
                encoded_source = original[start:end]
                decoded_source = (
                    encoded_source
                    if is_raw
                    else _decode_rust_ordinary_body(encoded_source)
                )
                source = (
                    decoded_source
                    if decoded_source is not None
                    else encoded_source
                )
                owner_sources.append(
                    migrate_classic_bare_imports(source)
                    if args.classic_bare_imports
                    else source
                )
    else:
        owner_sources.extend(
            migrate_classic_bare_imports(source)
            if args.classic_bare_imports
            else source
            for source in originals.values()
        )

    try:
        global_constructor_owners = collect_global_constructor_owners(
            owner_sources
        )
        global_dot_constructor_candidates = (
            collect_global_dot_constructor_candidates(owner_sources)
        )
    except Exception as error:
        print(f"error: failed to build constructor owner table: {error}", file=sys.stderr)
        return 2

    changed: list[Path] = []
    for path, original in originals.items():
        try:
            prepared = (
                migrate_classic_bare_imports(original)
                if args.classic_bare_imports and not args.rust_strings
                else original
            )
            migrated = (
                migrate_rust_strings(
                    original,
                    global_constructor_owners,
                    global_dot_constructor_candidates,
                    classic_bare_imports=args.classic_bare_imports,
                )
                if args.rust_strings
                else migrate_source(
                    prepared,
                    global_constructor_owners,
                    global_dot_constructor_candidates,
                )
            )
        except Exception as error:  # Continue so a corpus run reports every issue.
            failures.append((path, error))
            continue
        if migrated == original:
            continue
        changed.append(path)
        if not args.check:
            path.write_text(migrated)

    action = "need migration" if args.check else "migrated"
    print(f"{len(changed)} file(s) {action}; {len(paths)} file(s) examined")
    for path in changed:
        print(path)
    for path, error in failures:
        print(f"error: {path}: {error}", file=sys.stderr)
    if failures:
        return 2
    if args.check and changed:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
