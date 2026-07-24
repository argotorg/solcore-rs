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

Batch writes are failure-atomic for migration errors, I/O failures, and
interrupts.  Selected files must not be modified concurrently by another
process: the command revalidates bytes and file identities before and after
the commit, but no portable filesystem primitive can combine an in-place
metadata-preserving write with exclusion of uncooperative writers.

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
import locale
import os
from pathlib import Path
import re
import sys
import tempfile
from typing import BinaryIO, Iterable, Mapping, Sequence


TRIVIA = {"ws", "comment"}
WORD_RE = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
NUMBER_RE = re.compile(r"(?:0[xX][0-9A-Fa-f]+|[0-9]+)")
MULTI_SYMBOLS = (
    "<<=",
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
FUNCTION_TYPE_VISIBILITIES = {"internal", "external"}
FUNCTION_TYPE_MUTABILITIES = {"pure", "view", "payable"}
FUNCTION_TYPE_GENERAL_BOUNDARIES = {
    ",",
    ";",
    ":",
    "=",
    "->",
    ")",
    "]",
    "}",
    ">",
    ">=",
    ">>",
}
FUNCTION_TYPE_CONVERSION_BOUNDARIES = {
    ",",
    ";",
    ":",
    ")",
    "]",
    "}",
    "=>",
    "as",
    "?",
    "**",
    "*",
    "/",
    "%",
    "+",
    "-",
    "<<",
    ">>",
    "&",
    "^",
    "|",
    "<",
    ">",
    "<=",
    ">=",
    "==",
    "!=",
    "&&",
    "||",
    "=",
    "+=",
    "-=",
    "^=",
    "&=",
    "|=",
    "%=",
}
FUNCTION_TYPE_ARROW_BOUNDARIES = (
    FUNCTION_TYPE_GENERAL_BOUNDARIES | {"where", "{"}
)
FUNCTION_TYPE_PROXY_BOUNDARIES = (
    FUNCTION_TYPE_GENERAL_BOUNDARIES
    | FUNCTION_TYPE_CONVERSION_BOUNDARIES
    | {
        "(",
        "[",
        ".",
        "=",
        "+=",
        "-=",
        "^=",
        "&=",
        "|=",
        "%=",
    }
)
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


@dataclass(frozen=True)
class FunctionTypeSuffix:
    visibility_index: int | None
    mutability_index: int | None
    returns_open: int | None
    returns_close: int | None
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
    depth = 1
    cursor = start + 2
    while cursor < len(source):
        if source.startswith("/*", cursor):
            depth += 1
            cursor += 2
        elif source.startswith("*/", cursor):
            depth -= 1
            cursor += 2
            if depth == 0:
                return cursor
        else:
            cursor += 1
    return len(source)


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


def _split_type_angle_operator_tokens(
    tokens: Sequence[Token],
) -> list[Token]:
    """Split ``>=`` only when its leading ``>`` closes a type argument list."""

    result: list[Token] = []
    stack: list[str] = []
    for token in tokens:
        if token.text == ">=" and stack and stack[-1] == "<":
            result.append(
                Token(token.kind, ">", token.start, token.start + 1)
            )
            result.append(
                Token(token.kind, "=", token.start + 1, token.end)
            )
            stack.pop()
            continue
        result.append(token)
        _depth_step(stack, token.text)
    return result


def has_comment_marker(source: str, marker: str) -> bool:
    return any(
        token.kind == "comment" and marker in token.text
        for token in lex(source)
    )


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


def _separate_following_type_token(
    source: str,
    end: int,
    replacement: str,
) -> str:
    """Keep a rendered generic closer separate from a following operator."""

    if (
        replacement.endswith(">")
        and end < len(source)
        and source[end] in {">", "="}
    ):
        return replacement + " "
    return replacement


def _type_span_needs_operator_separation(
    source: str,
    type_tokens: Sequence[Token],
    tail: Sequence[Token],
    end: int,
) -> bool:
    return (
        bool(type_tokens)
        and end < len(tail)
        and type_tokens[-1].text == ">"
        and tail[end].text == "="
        and type_tokens[-1].end == tail[end].start
        and source[type_tokens[-1].start : tail[end].end] == ">="
    )


def _reject_dangling_type_comparison(
    tokens: Sequence[Token],
    end: int,
) -> None:
    """Reject an unmatched ``>`` that has no expression on its right."""

    if (
        end < len(tokens)
        and tokens[end].text == ">"
        and (
            end + 1 >= len(tokens)
            or tokens[end + 1].text in {",", ";", ")", "]", "}"}
        )
    ):
        raise ValueError(
            "cannot migrate malformed type delimiters near `>`"
        )


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
        elif opener == "<" and (
            (text and set(text) == {">"}) or text == ">="
        ):
            # The lexer preserves shift tokens, so nested generic closers may
            # arrive as one ``>>`` token. A ``>=`` token can also contribute
            # its leading character when a generic close is adjacent to `=`.
            depth -= 1 if text == ">=" else len(text)
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


def _type_angle_opens(
    tokens: Sequence[Token],
    index: int,
) -> bool:
    """Return whether a top-level ``<`` starts a generic type argument list."""

    if (
        index == 0
        or tokens[index].text != "<"
        or (
            tokens[index - 1].kind != "word"
            and tokens[index - 1].text not in {">", ">>"}
        )
    ):
        return False
    close = matching_index(tokens, index)
    return close is not None and not any(
        token.text in {";", "{", "}"}
        for token in tokens[index + 1 : close]
    )


def _type_boundary_opens_delimiter(
    tokens: Sequence[Token],
    start: int,
    index: int,
    *,
    allow_array_suffix: bool,
    forced_type_application: bool = False,
) -> bool:
    text = tokens[index].text
    if text == "<":
        return _type_angle_opens(tokens, index)
    if text == "(":
        follows_classic_arrow = (
            tokens[index - 1].text in LOCATIONS
            and find_top(tokens[start:index], "->") is not None
        )
        return (
            forced_type_application
            or index == start
            or follows_classic_arrow
            or (
                tokens[index - 1].kind == "word"
                and tokens[index - 1].text
                not in (
                    LOCATIONS
                    | FUNCTION_TYPE_VISIBILITIES
                    | FUNCTION_TYPE_MUTABILITIES
                )
            )
            or tokens[index - 1].text in {"@", "->"}
        )
    if text != "[":
        return False
    if not allow_array_suffix:
        return False
    close = matching_index(tokens, index)
    if close is None:
        return False
    length = tokens[index + 1 : close]
    return not length or (
        len(length) == 1 and length[0].kind == "number"
    )


def _type_expression_end(
    tokens: Sequence[Token],
    start: int,
    boundaries: set[str],
    *,
    word_boundaries: set[str] | None = None,
    allow_array_suffix: bool = True,
    forced_type_application_opens: set[int] | None = None,
) -> int:
    """Find the end of a complete type embedded in an expression.

    Expression operators terminate the type at depth zero, except that a
    qualified name followed by a balanced ``<...>`` opens generic arguments.
    """

    stack: list[str] = []
    end = start
    for cursor in range(start, len(tokens)):
        text = tokens[cursor].text
        if not stack:
            qualified_name_dot = (
                text == "."
                and cursor > start
                and cursor + 1 < len(tokens)
                and tokens[cursor - 1].kind == "word"
                and tokens[cursor - 1].text
                not in (
                    LOCATIONS
                    | FUNCTION_TYPE_VISIBILITIES
                    | FUNCTION_TYPE_MUTABILITIES
                )
                and tokens[cursor + 1].kind == "word"
            )
            if (
                tokens[cursor].kind == "word"
                and word_boundaries is not None
                and text in word_boundaries
            ):
                break
            if (
                text in boundaries
                and not qualified_name_dot
                and not _type_boundary_opens_delimiter(
                    tokens,
                    start,
                    cursor,
                    allow_array_suffix=allow_array_suffix,
                    forced_type_application=(
                        forced_type_application_opens is not None
                        and cursor in forced_type_application_opens
                    ),
                )
            ):
                break
        _depth_step(stack, text)
        end = cursor + 1
    return end


def _parse_function_type_suffix(
    tokens: Sequence[Token],
    start: int,
) -> tuple[FunctionTypeSuffix | None, int | None]:
    """Parse the fixed-order qualifier/return suffix after `function(...)`."""

    cursor = start
    visibility_index = None
    mutability_index = None
    returns_open = None
    returns_close = None

    if (
        cursor < len(tokens)
        and tokens[cursor].text in FUNCTION_TYPE_VISIBILITIES
    ):
        visibility_index = cursor
        cursor += 1
    if (
        cursor < len(tokens)
        and tokens[cursor].text in FUNCTION_TYPE_MUTABILITIES
    ):
        mutability_index = cursor
        cursor += 1
    if cursor < len(tokens) and tokens[cursor].text == "returns":
        returns_index = cursor
        if cursor + 1 >= len(tokens) or tokens[cursor + 1].text != "(":
            return None, returns_index
        returns_open = cursor + 1
        returns_close = matching_index(tokens, returns_open)
        if returns_close is None:
            return None, returns_index
        cursor = returns_close + 1

    return (
        FunctionTypeSuffix(
            visibility_index,
            mutability_index,
            returns_open,
            returns_close,
            cursor,
        ),
        None,
    )


def _function_type_outer_suffix_end(
    tokens: Sequence[Token],
    start: int,
    *,
    allow_arrays: bool = True,
) -> tuple[int, int | None, int | None]:
    """Consume the array suffixes and optional outer location of a type."""

    cursor = start
    if not allow_arrays and cursor < len(tokens) and tokens[cursor].text == "[":
        return cursor, None, None
    while cursor < len(tokens) and tokens[cursor].text == "[":
        close = matching_index(tokens, cursor)
        if close is None:
            return cursor, None, cursor
        length_tokens = tokens[cursor + 1 : close]
        if length_tokens:
            if (
                len(length_tokens) != 1
                or length_tokens[0].kind != "number"
            ):
                # A proxy expression may use this bracket as a postfix index.
                # Leave it unconsumed so the source-context validator can
                # distinguish that expression boundary from a type suffix.
                return cursor, None, None
            spelling = length_tokens[0].text
            if spelling.startswith("0X"):
                return cursor, None, cursor + 1
            base = 16 if spelling.lower().startswith("0x") else 10
            digits = spelling[2:] if base == 16 else spelling
            length = int(digits, base)
            if length == 0 or length > (1 << 64) - 1:
                return cursor, None, cursor + 1
        cursor = close + 1
    location_index = None
    if cursor < len(tokens) and tokens[cursor].text in LOCATIONS:
        location_index = cursor
        cursor += 1
    return cursor, location_index, None


def _validated_type_suffix_end(
    tokens: Sequence[Token],
    start: int,
    *,
    label: str,
) -> int:
    """Validate arrays followed by at most one outer data location."""

    end, location_index, error_index = _function_type_outer_suffix_end(
        tokens, start
    )
    if error_index is not None:
        raise ValueError(
            f"noncanonical {label} suffix near "
            f"`{tokens[error_index].text}`"
        )
    if end != len(tokens):
        offending = (
            location_index
            if (
                location_index is not None
                and tokens[end].text == "["
            )
            else end
        )
        raise ValueError(
            f"noncanonical {label} suffix near "
            f"`{tokens[offending].text}`"
        )
    return end


def _function_type_tail_error_index(
    tokens: Sequence[Token],
    suffix: FunctionTypeSuffix,
    *,
    allowed_word_boundaries: set[str] | None = None,
    allow_postfix_index_boundary: bool = False,
) -> tuple[int, int | None]:
    end, location_index, suffix_error = _function_type_outer_suffix_end(
        tokens,
        suffix.end,
        allow_arrays=not allow_postfix_index_boundary,
    )
    if suffix_error is not None:
        return end, suffix_error
    if end >= len(tokens):
        return end, None
    if (
        location_index is not None
        and tokens[end].text in {"[", "returns"}
    ):
        if (
            allow_postfix_index_boundary
            and tokens[end].text == "["
        ):
            return end, None
        return end, location_index
    if (
        tokens[end].kind == "word"
        and (
            allowed_word_boundaries is None
            or tokens[end].text not in allowed_word_boundaries
        )
    ):
        return end, end
    return end, None


def _function_type_prefix_context(
    tokens: Sequence[Token],
    function_index: int,
) -> tuple[str | None, bool, int]:
    cursor = function_index - 1
    outermost_prefix = None
    while cursor >= 0 and tokens[cursor].text in {"@", "comptime"}:
        outermost_prefix = tokens[cursor].text
        cursor -= 1
    predecessor = tokens[cursor].text if cursor >= 0 else None
    return predecessor, outermost_prefix == "@", cursor + 1


def _function_type_initializer_expression(
    tokens: Sequence[Token],
    prefix_index: int,
) -> bool:
    start = _previous_boundary(tokens, prefix_index)
    if start < len(tokens) and tokens[start].text in {"alias", "type"}:
        return False
    stack: list[str] = []
    for token in tokens[start:prefix_index]:
        if not stack and token.text == "=":
            return True
        _depth_step(stack, token.text)
    return False


def _proxy_prefix_is_expression(
    tokens: Sequence[Token],
    prefix_index: int,
    body_contexts: Sequence[tuple[int, int, set[int]]],
) -> bool:
    containing_bodies = [
        type_tokens
        for body_start, body_end, type_tokens in body_contexts
        if body_start <= prefix_index < body_end
    ]
    if containing_bodies:
        return any(
            prefix_index not in type_tokens
            for type_tokens in containing_bodies
        )
    return _function_type_initializer_expression(tokens, prefix_index)


def _proxy_expression_type_ranges(
    source: str,
    tokens: Sequence[Token],
    body_contexts: Sequence[tuple[int, int, set[int]]],
) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    covered_until = 0
    for index, token in enumerate(tokens):
        if (
            token.start < covered_until
            or token.text != "@"
            or index + 1 >= len(tokens)
            or not _proxy_prefix_is_expression(
                tokens, index, body_contexts
            )
        ):
            continue
        type_tail = _split_type_angle_operator_tokens(
            tokens[index + 1 :]
        )
        (
            call_boundary,
            forced_type_application_open,
        ) = _proxy_call_boundary(source, type_tail)
        scan_tail = (
            type_tail[:call_boundary]
            if call_boundary is not None
            else type_tail
        )
        end = _type_expression_end(
            scan_tail,
            0,
            (FUNCTION_TYPE_PROXY_BOUNDARIES - {"->"})
            | {"else", "then", "{"},
            word_boundaries={"as", "else", "then"},
            allow_array_suffix=False,
            forced_type_application_opens=(
                {forced_type_application_open}
                if forced_type_application_open is not None
                else None
            ),
        )
        if end <= 0:
            continue
        ranges.append(
            (scan_tail[0].start, scan_tail[end - 1].end)
        )
        covered_until = scan_tail[end - 1].end
    return ranges


def _function_type_control_words(
    tokens: Sequence[Token],
    prefix_index: int,
) -> set[str]:
    start = _previous_boundary(tokens, prefix_index)
    return {
        token.text
        for token in tokens[start:prefix_index]
        if token.text in {"for", "if", "match", "then", "while"}
    }


def _function_type_control_tail(
    tokens: Sequence[Token],
    prefix_index: int,
) -> str | None:
    start = _previous_boundary(tokens, prefix_index)
    return next(
        (
            token.text
            for token in reversed(tokens[start:prefix_index])
            if token.text in {"if", "match", "then", "else", "while"}
        ),
        None,
    )


def _function_type_is_mapping_key(
    tokens: Sequence[Token],
    function_index: int,
    separator_index: int,
) -> bool:
    for open_index in range(function_index - 1, -1, -1):
        if tokens[open_index].text != "(":
            continue
        close_index = matching_index(tokens, open_index)
        if close_index is None or close_index <= separator_index:
            continue
        if (
            open_index == 0
            or tokens[open_index - 1].text != "mapping"
            or (
                open_index >= 2
                and tokens[open_index - 2].text == "."
            )
        ):
            continue
        separator = find_top(
            tokens[open_index + 1 : close_index], "=>"
        )
        return (
            separator is not None
            and open_index + 1 + separator == separator_index
        )
    return False


def _function_type_source_tail_error_index(
    tokens: Sequence[Token],
    function_index: int,
    end: int,
    predecessor: str | None,
    proxy_expression: bool,
    annotation_expression: bool,
    allow_expression_block: bool,
    allowed_word_boundaries: set[str] | None,
) -> int | None:
    if end >= len(tokens):
        return None
    if (
        tokens[end].kind == "word"
        and allowed_word_boundaries is not None
        and tokens[end].text in allowed_word_boundaries
    ):
        return None
    if (
        tokens[end].text == "=>"
        and _function_type_is_mapping_key(tokens, function_index, end)
    ):
        return None
    if annotation_expression:
        allowed = FUNCTION_TYPE_CONVERSION_BOUNDARIES | {"->"}
    elif predecessor == "as":
        allowed = FUNCTION_TYPE_CONVERSION_BOUNDARIES | {"->"}
    elif predecessor == "->":
        allowed = FUNCTION_TYPE_ARROW_BOUNDARIES
    elif proxy_expression:
        allowed = FUNCTION_TYPE_PROXY_BOUNDARIES
    else:
        allowed = FUNCTION_TYPE_GENERAL_BOUNDARIES
    if (
        (proxy_expression or annotation_expression or predecessor == "as")
        and allow_expression_block
        and tokens[end].text == "{"
    ):
        return None
    return None if tokens[end].text in allowed else end


def _validate_type_delimiters(tokens: Sequence[Token]) -> None:
    """Reject malformed type spans before rendering can discard delimiters."""

    stack: list[str] = []
    for index, token in enumerate(tokens):
        text = token.text
        if text in {"(", "[", "{"}:
            stack.append(text)
            continue
        if text == "<":
            if not stack or stack[-1] != "[":
                stack.append(text)
            continue
        if text and set(text) == {">"}:
            if stack and stack[-1] == "[":
                # Shift/comparison operators inside an array-length
                # expression are not generic closers.
                continue
            for _ in text:
                if not stack or stack[-1] != "<":
                    raise ValueError(
                        "cannot migrate malformed type delimiters near "
                        f"`{text}`"
                    )
                stack.pop()
            continue
        if text not in {")", "]", "}"}:
            continue
        expected = CLOSE_TO_OPEN[text]
        if not stack or stack[-1] != expected:
            raise ValueError(
                "cannot migrate malformed type delimiters near "
                f"`{text}`"
            )
        stack.pop()
    if stack:
        raise ValueError(
            "cannot migrate malformed type delimiters: unclosed "
            f"`{stack[-1]}`"
        )


def _type_list_error_index(
    tokens: Sequence[Token],
    start: int,
    end: int,
    *,
    reject_named_entries: bool = False,
) -> int | None:
    """Find a leading/interior empty list item or a forbidden name colon."""

    stack: list[str] = []
    has_item = False
    for index in range(start, end):
        text = tokens[index].text
        if not stack and text == ",":
            if not has_item:
                return index
            has_item = False
            continue
        if reject_named_entries and text == ":":
            return index
        has_item = True
        _depth_step(stack, text)
    return None


def render_type(tokens: Sequence[Token]) -> str:
    tokens = _split_type_angle_operator_tokens(tokens)
    if not tokens:
        return ""
    _validate_type_delimiters(tokens)
    tokens = _expand_type_angle_closers(tokens)

    # Classic ``comptime`` consumes a complete type, including an arrow.
    if tokens[0].text == "comptime":
        inner_error: ValueError | None = None
        for split in range(len(tokens), 1, -1):
            try:
                inner = render_type(tokens[1:split])
                suffix_end = _validated_type_suffix_end(
                    tokens[split:], 0, label="comptime type"
                )
            except ValueError as error:
                if inner_error is None:
                    inner_error = error
                continue
            return (
                "comptime "
                + inner
                + _render_type_suffix(tokens[split:suffix_end + split])
            )
        if inner_error is not None:
            raise inner_error
        raise ValueError("comptime type is missing its inner type")

    arrow = find_top(tokens, "->")
    if arrow is not None:
        domain_tokens = tokens[:arrow]
        result_tokens = tokens[arrow + 1 :]
        if not domain_tokens or not result_tokens:
            missing = "domain" if not domain_tokens else "result"
            raise ValueError(
                "cannot migrate malformed Classic type arrow: "
                f"missing {missing} type"
            )
        # Classic arrow types always have one domain type.  A parenthesized
        # tuple is therefore one parameter, including ``()`` and ``(T)``;
        # unwrapping it here would silently change function arity.
        domain = render_type(domain_tokens)
        result = render_type(result_tokens)
        return f"function({domain}) returns ({result})"

    if (
        tokens[0].text == "@"
        and len(tokens) >= 4
        and tokens[1].text in LOCATIONS
        and tokens[2].text == "("
    ):
        close = matching_index(tokens, 2)
        if close == len(tokens) - 1:
            argument_tokens = tokens[3:close]
            arguments = split_top(argument_tokens, ",")
            trailing_comma = (
                bool(argument_tokens)
                and argument_tokens[-1].text == ","
            )
            rendered_arguments = [
                render_type(argument)
                for argument in arguments
                if argument
            ]
            arguments_text = ", ".join(rendered_arguments)
            if trailing_comma and len(rendered_arguments) > 1:
                arguments_text += ","
            return (
                "@"
                + tokens[1].text
                + "<"
                + arguments_text
                + ">"
            )

    if tokens[0].text == "@":
        return "@" + render_type(tokens[1:])

    # Canonical function types must remain canonical on repeated migrations.
    # Their qualifier order is fixed; array suffixes and one outer data
    # location apply only after the complete function type.
    if len(tokens) >= 2 and tokens[0].text == "function" and tokens[1].text == "(":
        close_index = matching_index(tokens, 1)
        if close_index is None:
            raise ValueError("unterminated canonical function type")
        params_error = _type_list_error_index(
            tokens,
            2,
            close_index,
            reject_named_entries=True,
        )
        if params_error is not None:
            raise ValueError(
                "noncanonical function type parameter list near "
                f"`{tokens[params_error].text}`"
            )
        params = [
            render_type(part)
            for part in split_top(tokens[2:close_index], ",")
            if part
        ]
        params_text = ", ".join(params)
        if (
            params
            and tokens[close_index - 1].text == ","
        ):
            params_text += ","
        suffix, error_index = _parse_function_type_suffix(
            tokens, close_index + 1
        )
        if error_index is not None:
            raise ValueError(
                "noncanonical function type qualifier sequence near "
                f"`{tokens[error_index].text}`"
            )
        assert suffix is not None
        end, tail_error = _function_type_tail_error_index(
            tokens, suffix
        )
        if tail_error is not None:
            raise ValueError(
                "noncanonical function type qualifier sequence near "
                f"`{tokens[tail_error].text}`"
            )
        if end != len(tokens):
            raise ValueError(
                "noncanonical function type suffix near "
                f"`{tokens[end].text}`"
            )
        qualifiers = [
            tokens[index].text
            for index in (
                suffix.visibility_index,
                suffix.mutability_index,
            )
            if index is not None
        ]
        qualifiers_text = (
            " " + " ".join(qualifiers) if qualifiers else ""
        )
        result = ""
        if suffix.returns_open is not None:
            assert suffix.returns_close is not None
            returns_error = _type_list_error_index(
                tokens,
                suffix.returns_open + 1,
                suffix.returns_close,
                reject_named_entries=True,
            )
            if returns_error is not None:
                raise ValueError(
                    "noncanonical function type return list near "
                    f"`{tokens[returns_error].text}`"
                )
            result_parts = [
                render_type(part)
                for part in split_top(
                    tokens[
                        suffix.returns_open + 1 : suffix.returns_close
                    ],
                    ",",
                )
                if part
            ]
            result_text = ", ".join(result_parts)
            if (
                result_parts
                and tokens[suffix.returns_close - 1].text == ","
            ):
                result_text += ","
            result = " returns (" + result_text + ")"
        return (
            "function("
            + params_text
            + ")"
            + qualifiers_text
            + result
            + _render_type_suffix(tokens[suffix.end:end])
        )

    if tokens[0].text == "(":
        wrapped_close = matching_index(tokens, 0)
        if wrapped_close is not None and wrapped_close < len(tokens) - 1:
            suffix_end, _, suffix_error = (
                _function_type_outer_suffix_end(
                    tokens, wrapped_close + 1
                )
            )
            if suffix_error is not None:
                raise ValueError(
                    "noncanonical wrapped type suffix near "
                    f"`{tokens[suffix_error].text}`"
                )
            if suffix_end == len(tokens):
                return (
                    render_type(tokens[: wrapped_close + 1])
                    + _render_type_suffix(
                        tokens[wrapped_close + 1 : suffix_end]
                    )
                )

    if is_wrapped(tokens, "(", ")"):
        tuple_error = _type_list_error_index(
            tokens, 1, len(tokens) - 1
        )
        if tuple_error is not None:
            raise ValueError(
                "noncanonical tuple type near "
                f"`{tokens[tuple_error].text}`"
            )
        elements = split_top(tokens[1:-1], ",")
        if len(elements) == 1 and not elements[0]:
            return "()"
        rendered_elements = [
            render_type(element) for element in elements if element
        ]
        elements_text = ", ".join(rendered_elements)
        if (
            rendered_elements
            and tokens[-2].text == ","
        ):
            elements_text += ","
        return "(" + elements_text + ")"

    name_end = _qualified_name_end(tokens)
    if name_end:
        name = "".join(token.text for token in tokens[:name_end])
        rest = tokens[name_end:]
        if rest and rest[0].text in {"(", "<"}:
            close = ")" if rest[0].text == "(" else ">"
            close_index = matching_index(rest, 0)
            if close_index is not None:
                arg_tokens = rest[1:close_index]
                args_error = _type_list_error_index(
                    rest, 1, close_index
                )
                if args_error is not None:
                    raise ValueError(
                        "noncanonical type argument list near "
                        f"`{rest[args_error].text}`"
                    )
                mapping_arrow = (
                    find_top(arg_tokens, "=>")
                    if name == "mapping" and rest[0].text == "("
                    else None
                )
                if name == "mapping" and rest[0].text == "(":
                    if mapping_arrow is not None:
                        extra_arrow = find_top(
                            arg_tokens[mapping_arrow + 1 :], "=>"
                        )
                        comma = find_top(arg_tokens, ",")
                        malformed_mapping = (
                            not arg_tokens[:mapping_arrow]
                            or not arg_tokens[mapping_arrow + 1 :]
                            or extra_arrow is not None
                            or comma is not None
                        )
                        args = [
                            arg_tokens[:mapping_arrow],
                            arg_tokens[mapping_arrow + 1 :],
                        ]
                    else:
                        args = split_top(arg_tokens, ",")
                        if args and not args[-1]:
                            args = args[:-1]
                        malformed_mapping = (
                            len(args) != 2
                            or any(not arg for arg in args)
                        )
                    if malformed_mapping:
                        raise ValueError(
                            "cannot migrate malformed mapping type: "
                            "expected exactly `mapping(Key, Value)` or "
                            "`mapping(Key => Value)`"
                        )
                else:
                    args = split_top(arg_tokens, ",")
                rendered_args = [render_type(arg) for arg in args if arg]
                args_text = ", ".join(rendered_args)
                if (
                    rendered_args
                    and arg_tokens
                    and arg_tokens[-1].text == ","
                ):
                    args_text += ","
                suffix = rest[close_index + 1 :]
                if (
                    name == "mapping"
                    and rest[0].text == "("
                    and len(rendered_args) == 2
                ):
                    base = f"mapping({rendered_args[0]} => {rendered_args[1]})"
                elif (
                    name in LOCATIONS
                    and rest[0].text == "("
                    and len(rendered_args) == 1
                ):
                    rendered_inner_tokens = significant(rendered_args[0])
                    if (
                        rendered_inner_tokens
                        and (
                            rendered_inner_tokens[0].text == "comptime"
                            or rendered_inner_tokens[-1].text in LOCATIONS
                        )
                    ):
                        base = f"{name}<{rendered_args[0]}>"
                    else:
                        base = f"{rendered_args[0]} {name}"
                else:
                    base = name + "<" + args_text + ">"
                suffix_end = _validated_type_suffix_end(
                    suffix, 0, label="type"
                )
                return base + _render_type_suffix(suffix[:suffix_end])
        suffix_end = _validated_type_suffix_end(
            rest, 0, label="type"
        )
        return name + _render_type_suffix(rest[:suffix_end])

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


def render_return_item(tokens: Sequence[Token]) -> str:
    tokens = list(tokens)
    colon = find_top(tokens, ":")
    if colon is None:
        return render_type(tokens)
    binding = join_tokens(tokens[:colon])
    ty = render_type(tokens[colon + 1 :])
    return f"{binding}: {ty}"


def render_return_type(tokens: Sequence[Token]) -> str:
    tokens = list(tokens)
    if is_wrapped(tokens, "(", ")"):
        elements = split_top(tokens[1:-1], ",")
        if len(elements) == 1 and not elements[0]:
            return ""
        return ", ".join(render_return_item(element) for element in elements)
    return render_return_item(tokens)


def render_return_clause_items(tokens: Sequence[Token]) -> str:
    """Render the contents of an existing canonical `returns (...)` clause."""

    return ", ".join(
        render_return_item(part)
        for part in split_top(tokens, ",")
        if part
    )


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


def render_trait_ref_text(tokens: Sequence[Token]) -> str | None:
    trait = render_trait_ref(tokens)
    if trait is None:
        return None
    name, args = trait
    suffix = "<" + ", ".join(args) + ">" if args else ""
    return name + suffix


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


def reject_string_imports(source: str) -> None:
    """Reject Solidity path strings, which have no canonical Core spelling."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if token.text != "import":
            continue
        end = _statement_end(tokens, index)
        if end is None:
            continue
        path = next(
            (
                item
                for item in tokens[index + 1 : end]
                if item.kind == "string"
            ),
            None,
        )
        if path is None:
            continue
        line, column = _source_line_column(source, path.start)
        raise ValueError(
            "cannot migrate string import at "
            f"line {line}, column {column}: Core imports use dotted "
            "module names; replace the path string with its resolved module"
        )


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
        replacement = _with_preserved_comments(
            source,
            token.start,
            tokens[end].end,
            replacement,
        )
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
        replacement = _with_preserved_comments(
            source,
            token.start,
            tokens[end].end,
            replacement,
        )
        replacements.append((token.start, tokens[end].end, replacement))
    return replace_spans(source, replacements)


def _classic_brace_context(
    tokens: Sequence[Token],
) -> tuple[dict[int, int], list[int | None]]:
    """Return matching braces and the innermost block around each token."""

    pairs: dict[int, int] = {}
    enclosing: list[int | None] = [None] * len(tokens)
    stack: list[int] = []
    for index, token in enumerate(tokens):
        if token.text == "}" and stack:
            open_index = stack.pop()
            pairs[open_index] = index
            pairs[index] = open_index
        enclosing[index] = stack[-1] if stack else None
        if token.text == "{":
            stack.append(index)
    return pairs, enclosing


def _classic_callable_regions(
    tokens: Sequence[Token], brace_pairs: Mapping[int, int]
) -> list[tuple[int, int, int | None, int | None]]:
    """Return body and parameter-list bounds for function-like declarations."""

    regions: list[tuple[int, int, int | None, int | None]] = []
    for index, token in enumerate(tokens):
        if token.text not in {"function", "constructor", "fallback", "lam"}:
            continue
        body_open = _header_boundary(tokens, index + 1)
        if (
            body_open is None
            or tokens[body_open].text != "{"
            or body_open not in brace_pairs
        ):
            continue
        param_open = next(
            (
                cursor
                for cursor in range(index + 1, body_open)
                if tokens[cursor].text == "("
            ),
            None,
        )
        param_close = (
            matching_index(tokens, param_open) if param_open is not None else None
        )
        if param_close is not None and param_close >= body_open:
            param_open = None
            param_close = None
        regions.append(
            (body_open, brace_pairs[body_open], param_open, param_close)
        )
    return regions


def _classic_binding_names(
    tokens: Sequence[Token], namespace_roots: set[str]
) -> set[str]:
    """Collect binding identifiers from a parameter or tuple binding pattern."""

    return {
        token.text
        for token in tokens
        if token.kind == "word"
        and token.text != "comptime"
        and token.text in namespace_roots
    }


def _classic_pattern_binding_names(
    tokens: Sequence[Token], namespace_roots: set[str]
) -> set[str]:
    """Collect binders without mistaking qualified constructor paths for them."""

    result: set[str] = set()
    for index, token in enumerate(tokens):
        if token.kind != "word" or token.text not in namespace_roots:
            continue
        previous = tokens[index - 1].text if index else ""
        following = tokens[index + 1].text if index + 1 < len(tokens) else ""
        if previous == "." or following in {".", "("}:
            continue
        result.add(token.text)
    return result


def _classic_type_only_tokens(tokens: Sequence[Token]) -> set[int]:
    """Mark explicit type regions where term bindings cannot shadow modules."""

    marked: set[int] = set()
    stops = {
        "=",
        ":=",
        ";",
        ",",
        ")",
        "]",
        "}",
        "{",
        "=>",
        "?",
        ":",
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
    }
    for index, token in enumerate(tokens):
        if token.text == ":" and not _is_ternary_colon(tokens, index):
            _mark_type_region(tokens, index + 1, len(tokens), stops, marked)
        elif token.text == "as":
            _mark_type_region(tokens, index + 1, len(tokens), stops, marked)
    return marked


def _classic_shadow_ranges(
    tokens: Sequence[Token], namespace_roots: set[str]
) -> dict[str, list[tuple[int, int]]]:
    """Map term bindings to the token ranges where they shadow import roots."""

    if not namespace_roots:
        return {}

    ranges: dict[str, list[tuple[int, int]]] = {
        name: [] for name in namespace_roots
    }
    brace_pairs, enclosing = _classic_brace_context(tokens)
    callables = _classic_callable_regions(tokens, brace_pairs)

    def add(names: Iterable[str], start: int, end: int) -> None:
        if start >= end:
            return
        for name in names:
            ranges[name].append((start, end))

    # Parameters shadow a namespace throughout their function or lambda body.
    for body_open, body_close, param_open, param_close in callables:
        if param_open is None or param_close is None:
            continue
        for parameter in split_top(tokens[param_open + 1 : param_close], ","):
            colon = find_top(parameter, ":", angles=False)
            binding = parameter if colon is None else parameter[:colon]
            add(
                _classic_binding_names(binding, namespace_roots),
                body_open + 1,
                body_close,
            )

    # A contract field is visible in every executable body in that contract.
    for index, token in enumerate(tokens):
        if token.text not in {"contract", "interface", "library"}:
            continue
        contract_open = _header_boundary(tokens, index + 1)
        if (
            contract_open is None
            or tokens[contract_open].text != "{"
            or contract_open not in brace_pairs
        ):
            continue
        contract_close = brace_pairs[contract_open]
        field_names: set[str] = set()
        field_initializers: list[tuple[int, int]] = []
        for cursor in range(contract_open + 1, contract_close - 1):
            if (
                enclosing[cursor] != contract_open
                or tokens[cursor].kind != "word"
                or tokens[cursor + 1].text != ":"
            ):
                continue
            previous = tokens[cursor - 1].text
            if previous not in {"{", "}", ";"}:
                continue
            field_end = _statement_end(tokens, cursor)
            if field_end is None or field_end >= contract_close:
                continue
            if tokens[cursor].text in namespace_roots:
                field_names.add(tokens[cursor].text)
            equals = find_top(tokens[cursor + 2 : field_end], "=", angles=False)
            if equals is not None:
                field_initializers.append((cursor + 2 + equals + 1, field_end))
        if not field_names:
            continue
        for body_open, body_close, _, _ in callables:
            if contract_open < body_open < body_close < contract_close:
                add(field_names, body_open + 1, body_close)
        for initializer_start, initializer_end in field_initializers:
            add(field_names, initializer_start, initializer_end)

    # `let` bindings begin after their initializer and end with their lexical
    # block. A binding in a for initializer instead ends with that loop body.
    for_loops: list[tuple[int, int, int]] = []
    for index, token in enumerate(tokens):
        if token.text != "for" or index + 1 >= len(tokens):
            continue
        paren_open = index + 1
        if tokens[paren_open].text != "(":
            continue
        paren_close = matching_index(tokens, paren_open)
        if (
            paren_close is None
            or paren_close + 1 >= len(tokens)
            or tokens[paren_close + 1].text != "{"
        ):
            continue
        body_close = brace_pairs.get(paren_close + 1)
        if body_close is not None:
            for_loops.append((paren_open, paren_close, body_close))

    for index, token in enumerate(tokens):
        if token.text != "let":
            continue
        for_loop = next(
            (
                loop
                for loop in for_loops
                if loop[0] < index < loop[1]
            ),
            None,
        )
        declaration_end: int | None = None
        stack: list[str] = []
        scan_end = for_loop[1] + 1 if for_loop is not None else len(tokens)
        for cursor in range(index + 1, scan_end):
            text = tokens[cursor].text
            if not stack and (
                text in {",", ";"}
                or (for_loop is not None and cursor == for_loop[1])
            ):
                declaration_end = cursor
                break
            _depth_step(stack, text, angles=False)
        if declaration_end is None:
            continue
        binding_end = find_top_any(
            tokens[index + 1 : declaration_end],
            {":", "=", ":="},
            angles=False,
        )
        binding = tokens[
            index + 1 :
            declaration_end if binding_end is None else index + 1 + binding_end
        ]
        names = _classic_binding_names(binding, namespace_roots)
        if not names:
            continue
        if for_loop is not None:
            scope_end = for_loop[2]
        else:
            block_open = enclosing[index]
            scope_end = (
                brace_pairs.get(block_open, len(tokens))
                if block_open is not None
                else len(tokens)
            )
        add(names, declaration_end + 1, scope_end)

    # Canonical match binders are scoped to their case body.
    for index, token in enumerate(tokens):
        if token.text != "case":
            continue
        body_open = _header_boundary(tokens, index + 1)
        if (
            body_open is None
            or tokens[body_open].text != "{"
            or body_open not in brace_pairs
        ):
            continue
        add(
            _classic_pattern_binding_names(
                tokens[index + 1 : body_open], namespace_roots
            ),
            body_open + 1,
            brace_pairs[body_open],
        )

    # Classic `| pattern => expression` binders end at the next match arm.
    for index, token in enumerate(tokens):
        if token.text != "match":
            continue
        match_open = _header_boundary(tokens, index + 1)
        if (
            match_open is None
            or tokens[match_open].text != "{"
            or match_open not in brace_pairs
        ):
            continue
        match_close = brace_pairs[match_open]
        arms = _match_arm_starts(tokens, match_open + 1, match_close)
        for arm_offset, arm_start in enumerate(arms):
            arm_end = (
                arms[arm_offset + 1]
                if arm_offset + 1 < len(arms)
                else match_close
            )
            arrow = find_top(
                tokens[arm_start + 1 : arm_end], "=>", angles=False
            )
            if arrow is None:
                continue
            arrow += arm_start + 1
            add(
                _classic_pattern_binding_names(
                    tokens[arm_start + 1 : arrow], namespace_roots
                ),
                arrow + 1,
                arm_end,
            )

    return ranges


def migrate_classic_bare_imports(
    source: str,
    path_limits: Mapping[str, int] | None = None,
) -> str:
    """Rewrite Classic bare namespace imports and their qualified uses.

    ``path_limits`` is used by repository migrations that contain a mixture of
    already-converted wildcard imports and Classic bare imports with the same
    path.  The public CLI omits it and converts every bare import in its input.
    Term bindings shadow the first path segment only within their lexical
    scope; explicit type regions continue to use the imported namespace.
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
        replacement = _with_preserved_comments(
            source,
            token.start,
            tokens[end].end,
            f"import * as {alias} from {path};",
        )
        replacements.append((token.start, tokens[end].end, replacement))
        import_spans.append((token.start, tokens[end].end))
        visible = segments[1:] if external else segments
        namespace_paths.append((visible, alias))

    def inside_import(start: int, end: int) -> bool:
        return any(
            import_start <= start and end <= import_end
            for import_start, import_end in import_spans
        )

    shadow_ranges = _classic_shadow_ranges(
        tokens,
        {segments[0] for segments, _ in namespace_paths if len(segments) >= 2},
    )
    type_only_tokens = _classic_type_only_tokens(tokens)

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
            if index not in type_only_tokens and any(
                scope_start <= index < scope_end
                for scope_start, scope_end in shadow_ranges[segments[0]]
            ):
                continue
            seen_uses.add((start, end))
            replacement = _with_preserved_comments(
                source,
                start,
                end,
                alias,
            )
            replacements.append((start, end, replacement))

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


def reject_solidity_call_options(source: str) -> None:
    """Reject call options before their colons look like type annotations."""

    tokens = significant(source)
    option_names = {"gas", "salt", "value"}
    for open_index, token in enumerate(tokens):
        if token.text != "{":
            continue
        close_index = matching_index(tokens, open_index)
        if (
            close_index is None
            or close_index + 1 >= len(tokens)
            or tokens[close_index + 1].text != "("
        ):
            continue
        for part in split_top(
            tokens[open_index + 1 : close_index], ",", angles=False
        ):
            colon = find_top(part, ":", angles=False)
            if (
                colon is None
                or colon == 0
                or part[colon - 1].kind != "word"
                or part[colon - 1].text not in option_names
            ):
                continue
            line, column = _source_line_column(
                source, part[colon - 1].start
            )
            raise ValueError(
                "cannot migrate Solidity call options at "
                f"line {line}, column {column}: Core syntax does not "
                "support `{value: ...}`, `{gas: ...}`, or `{salt: ...}`; "
                "use the explicit standard-library call operation"
            )


def reject_contract_inheritance(source: str) -> None:
    """Reject Classic inheritance that Core cannot preserve automatically."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if token.text not in {"contract", "interface", "library"}:
            continue
        if index + 1 >= len(tokens) or tokens[index + 1].kind != "word":
            continue
        inheritance = index + 2
        if (
            inheritance < len(tokens)
            and tokens[inheritance].text == "<"
        ):
            generic_close = matching_index(tokens, inheritance)
            if generic_close is None:
                continue
            inheritance = generic_close + 1
        if (
            inheritance >= len(tokens)
            or tokens[inheritance].text != "is"
        ):
            continue
        line, column = _source_line_column(
            source, tokens[inheritance].start
        )
        raise ValueError(
            f"cannot migrate {token.text} inheritance at line {line}, "
            f"column {column}: Core syntax does not support `is` base "
            "clauses; replace inheritance and base-constructor calls with "
            "composition or traits"
        )


def reject_noncanonical_proxy_comptime(source: str) -> None:
    """Reject ``@comptime T`` where ``@`` occurs inside a type."""

    tokens = significant(source)
    executable_bodies = _executable_regions(tokens)[0]
    body_contexts = [
        (
            body_start,
            body_end,
            _body_type_tokens(tokens, body_start, body_end),
        )
        for body_start, body_end in executable_bodies
    ]
    expression_type_ranges = _proxy_expression_type_ranges(
        source, tokens, body_contexts
    )
    for index in range(len(tokens) - 1):
        if (
            tokens[index].text != "@"
            or tokens[index + 1].text != "comptime"
        ):
            continue
        if not any(
            start <= tokens[index].start < end
            for start, end in expression_type_ranges
        ) and _proxy_prefix_is_expression(
            tokens, index, body_contexts
        ):
            continue
        line, column = _source_line_column(
            source, tokens[index + 1].start
        )
        raise ValueError(
            "cannot migrate noncanonical proxy type at "
            f"line {line}, column {column}: write `comptime @T`, not "
            "`@comptime T`; `@comptime T` is only valid as a proxy "
            "expression"
        )


def reject_malformed_mapping_types(source: str) -> None:
    """Reject malformed Solidity-style mapping argument lists."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if (
            token.text != "mapping"
            or index + 1 >= len(tokens)
            or tokens[index + 1].text != "("
            or (index > 0 and tokens[index - 1].text == ".")
        ):
            continue
        close = matching_index(tokens, index + 1)
        if close is None:
            continue
        arguments = tokens[index + 2 : close]
        arrow = find_top(arguments, "=>")
        if arrow is None:
            continue
        comma = find_top(arguments, ",")
        if (
            not arguments[:arrow]
            or not arguments[arrow + 1 :]
            or find_top(arguments[arrow + 1 :], "=>") is not None
            or comma is not None
        ):
            offending_index = (
                index + 2 + comma
                if comma is not None
                else index + 2 + arrow
            )
            line, column = _source_line_column(
                source, tokens[offending_index].start
            )
            raise ValueError(
                "cannot migrate malformed mapping type at "
                f"line {line}, column {column}: expected exactly "
                "`mapping(Key => Value)`"
            )


def reject_noncanonical_function_type_qualifiers(source: str) -> None:
    """Reject function-type qualifiers that cannot be safely reordered."""

    tokens = significant(source)
    executable_bodies = _executable_regions(tokens)[0]
    body_contexts = [
        (
            body_start,
            body_end,
            _body_type_tokens(tokens, body_start, body_end),
        )
        for body_start, body_end in executable_bodies
    ]
    for index, token in enumerate(tokens):
        if (
            token.text != "function"
            or index + 1 >= len(tokens)
            or tokens[index + 1].text != "("
        ):
            continue
        params_close = matching_index(tokens, index + 1)
        if params_close is None:
            continue
        error_index = _type_list_error_index(
            tokens,
            index + 2,
            params_close,
            reject_named_entries=True,
        )
        if error_index is None:
            for open_index in range(index - 1, -1, -1):
                if tokens[open_index].text != "<":
                    continue
                close_index = matching_index(tokens, open_index)
                if close_index is None or close_index < index:
                    continue
                error_index = _type_list_error_index(
                    tokens, open_index + 1, close_index
                )
                if error_index is not None:
                    break
        if error_index is None:
            for part in split_top(
                tokens[index + 2 : params_close], ","
            ):
                if part:
                    try:
                        render_type(part)
                    except ValueError as error:
                        line, column = _source_line_column(
                            source, part[0].start
                        )
                        raise ValueError(
                            "cannot migrate noncanonical function type "
                            f"qualifier or nested type at line {line}, "
                            f"column {column}: {error}"
                        ) from error
        suffix = None
        if error_index is None:
            suffix, error_index = _parse_function_type_suffix(
                tokens, params_close + 1
            )
        if (
            suffix is not None
            and error_index is None
            and suffix.returns_open is not None
        ):
            assert suffix.returns_close is not None
            error_index = _type_list_error_index(
                tokens,
                suffix.returns_open + 1,
                suffix.returns_close,
                reject_named_entries=True,
            )
            if error_index is None:
                for part in split_top(
                    tokens[
                        suffix.returns_open + 1 : suffix.returns_close
                    ],
                    ",",
                ):
                    if part:
                        try:
                            render_type(part)
                        except ValueError as error:
                            line, column = _source_line_column(
                                source, part[0].start
                            )
                            raise ValueError(
                                "cannot migrate noncanonical function type "
                                f"qualifier or nested type at line {line}, "
                                f"column {column}: {error}"
                            ) from error
        if suffix is not None and error_index is None:
            (
                predecessor,
                has_proxy_prefix,
                prefix_index,
            ) = _function_type_prefix_context(tokens, index)
            proxy_expression = False
            if has_proxy_prefix:
                proxy_expression = _proxy_prefix_is_expression(
                    tokens, prefix_index, body_contexts
                )
            if any(
                item.text == "comptime"
                for item in tokens[prefix_index:index]
            ):
                comptime_tokens = _split_type_angle_operator_tokens(
                    tokens[prefix_index:]
                )
                comptime_end = _type_expression_end(
                    comptime_tokens,
                    0,
                    (
                        FUNCTION_TYPE_PROXY_BOUNDARIES
                        if (
                            proxy_expression
                            or predecessor in {"as", ":"}
                        )
                        else FUNCTION_TYPE_GENERAL_BOUNDARIES
                    )
                    | {"=>", "else", "then", "{"},
                    word_boundaries={"as", "else", "then", "where"},
                )
                try:
                    render_type(comptime_tokens[:comptime_end])
                except ValueError:
                    pass
                else:
                    continue
            control_words = (
                _function_type_control_words(tokens, prefix_index)
                if proxy_expression or predecessor in {"as", ":"}
                else set()
            )
            control_tail = _function_type_control_tail(
                tokens, prefix_index
            )
            annotation_expression = (
                predecessor == ":"
                and prefix_index > 0
                and _is_expression_annotation_colon(
                    tokens,
                    prefix_index - 1,
                    executable_bodies,
                )
            )
            if predecessor == "->":
                allowed_word_boundaries = {"where"}
            elif predecessor == "as" or annotation_expression:
                allowed_word_boundaries = {"as"}
            elif proxy_expression:
                allowed_word_boundaries = {"as"}
            else:
                allowed_word_boundaries = None
            if proxy_expression or predecessor == "as" or annotation_expression:
                if "if" in control_words:
                    allowed_word_boundaries.add("then")
                if "then" in control_words:
                    allowed_word_boundaries.add("else")
            end, error_index = _function_type_tail_error_index(
                tokens,
                suffix,
                allowed_word_boundaries=allowed_word_boundaries,
                allow_postfix_index_boundary=proxy_expression,
            )
            if error_index is None:
                error_index = _function_type_source_tail_error_index(
                    tokens,
                    index,
                    end,
                    predecessor,
                    proxy_expression,
                    annotation_expression,
                    control_tail in {"if", "match", "while"},
                    allowed_word_boundaries,
                )
        if error_index is None:
            continue
        offending = tokens[error_index]
        line, column = _source_line_column(source, offending.start)
        raise ValueError(
            "cannot migrate noncanonical function type qualifier "
            f"`{offending.text}` at line {line}, column {column}: expected "
            "`function(...)`, optional `internal` or `external`, optional "
            "`pure`, `view`, or `payable`, optional `returns (...)`, array "
            "suffixes, then at most one outer `memory`, `storage`, or "
            "`calldata`"
        )


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


def _realign_type_application_delimiters(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    # Classic generic applications use `Name(...)`, while the new spelling is
    # `Name<...>`. Nested canonical closes may also lex as one `>>` token.
    # Structural alignment keeps comments with the corresponding delimiters.
    for old_open, token in enumerate(before):
        if (
            token.text not in {"(", "<"}
            or old_open == 0
            or before[old_open - 1].kind != "word"
            or (
                token.text == "("
                and old_open >= 2
                and before[old_open - 2].text == "function"
            )
            or old_open - 1 not in aligned
        ):
            continue
        new_name = aligned[old_open - 1]
        new_open = new_name + 1
        if new_open >= len(after) or after[new_open].text != "<":
            continue
        old_close = matching_index(before, old_open)
        new_close = matching_index(after, new_open)
        if old_close is None or new_close is None:
            continue
        for old_index, new_index in list(aligned.items()):
            if (
                old_index in {old_open, old_close}
                or new_index == new_open
                or (
                    new_index == new_close
                    and before[old_index].text not in {")", ">", ">>"}
                )
            ):
                aligned.pop(old_index)
        aligned[old_open] = new_open
        aligned[old_close] = new_close


def _realign_token_range(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
    old_start: int,
    old_end: int,
    new_start: int,
    new_end: int,
) -> None:
    """Align a source range whose token order is preserved in the output."""

    removed_wrapper_tokens: set[int] = set()
    for old_open in range(old_start, old_end):
        if (
            before[old_open].text == "("
            and old_open > old_start
            and before[old_open - 1].text in LOCATIONS
        ):
            old_close = matching_index(before, old_open)
            if old_close is not None and old_close < old_end:
                removed_wrapper_tokens.update(
                    {old_open - 1, old_open, old_close}
                )

    new_cursor = new_start
    for old_index in range(old_start, old_end):
        if old_index in removed_wrapper_tokens:
            continue
        match = next(
            (
                new_index
                for new_index in range(new_cursor, new_end)
                if (
                    before[old_index].kind,
                    before[old_index].text,
                )
                == (
                    after[new_index].kind,
                    after[new_index].text,
                )
            ),
            None,
        )
        if match is not None:
            aligned[old_index] = match
            new_cursor = match + 1

    for old_open in range(old_start, old_end):
        if (
            before[old_open].text not in {"(", "<"}
            or old_open <= old_start
            or before[old_open - 1].kind != "word"
            or (
                before[old_open].text == "("
                and before[old_open - 1].text in LOCATIONS
            )
            or old_open - 1 not in aligned
        ):
            continue
        new_open = aligned[old_open - 1] + 1
        if (
            new_open < new_start
            or new_open >= new_end
            or after[new_open].text != "<"
        ):
            continue
        old_close = matching_index(before, old_open)
        new_close = matching_index(after, new_open)
        if (
            old_close is None
            or old_close >= old_end
            or new_close is None
            or new_close >= new_end
        ):
            continue
        aligned[old_open] = new_open
        aligned[old_close] = new_close


def _realign_callable_signature_tokens(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    declaration_index = next(
        (
            index
            for index, token in enumerate(before)
            if (
                token.text in {"lam", "constructor", "fallback"}
                or (
                    token.text == "function"
                    and index + 1 < len(before)
                    and before[index + 1].kind == "word"
                )
            )
        ),
        None,
    )
    if declaration_index is None or declaration_index not in aligned:
        return
    new_declaration = aligned[declaration_index]
    old_open = next(
        (
            index
            for index in range(declaration_index + 1, len(before))
            if before[index].text == "("
        ),
        None,
    )
    new_open = next(
        (
            index
            for index in range(new_declaration + 1, len(after))
            if after[index].text == "("
        ),
        None,
    )
    if old_open is None or new_open is None:
        return
    old_close = matching_index(before, old_open)
    new_close = matching_index(after, new_open)
    if old_close is None or new_close is None:
        return

    aligned[old_open] = new_open
    aligned[old_close] = new_close
    _realign_token_range(
        before,
        after,
        aligned,
        old_open + 1,
        old_close,
        new_open + 1,
        new_close,
    )

    old_tail_end = len(before)
    old_where = find_top(before[old_close + 1 :], "where")
    if old_where is not None:
        old_tail_end = old_close + 1 + old_where
    old_tail = before[old_close + 1 : old_tail_end]
    old_arrow = find_top(old_tail, "->")
    old_returns = find_top(old_tail, "returns")
    if old_arrow is not None:
        old_return_start = old_close + 1 + old_arrow + 1
        old_return_end = old_tail_end
    elif old_returns is not None:
        old_returns += old_close + 1
        if (
            old_returns + 1 >= len(before)
            or before[old_returns + 1].text != "("
        ):
            return
        old_returns_close = matching_index(before, old_returns + 1)
        if old_returns_close is None:
            return
        old_return_start = old_returns + 2
        old_return_end = old_returns_close
    else:
        return

    new_returns = find_top(after[new_close + 1 :], "returns")
    if new_returns is None:
        return
    new_returns += new_close + 1
    if (
        new_returns + 1 >= len(after)
        or after[new_returns + 1].text != "("
    ):
        return
    new_returns_close = matching_index(after, new_returns + 1)
    if new_returns_close is None:
        return
    if old_returns is not None:
        aligned[old_returns] = new_returns
        aligned[old_returns + 1] = new_returns + 1
        aligned[old_return_end] = new_returns_close
    _realign_token_range(
        before,
        after,
        aligned,
        old_return_start,
        old_return_end,
        new_returns + 2,
        new_returns_close,
    )


def _realign_location_wrapper_delimiters(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Anchor removed `memory(T)` delimiters within the rewritten type."""

    wrapper_ordinals: dict[str, int] = {}
    for old_open, token in enumerate(before):
        if (
            token.text != "("
            or old_open == 0
            or before[old_open - 1].text not in LOCATIONS
        ):
            continue
        old_close = matching_index(before, old_open)
        if old_close is None:
            continue
        old_location = old_open - 1
        location = before[old_location].text
        candidates = [
            index for index, token in enumerate(after) if token.text == location
        ]
        mapped_payload = [
            aligned[index]
            for index in range(old_open + 1, old_close)
            if index in aligned
        ]
        new_location = next(
            (
                index
                for index in candidates
                if mapped_payload and index == max(mapped_payload) + 1
            ),
            None,
        )
        if new_location is None and mapped_payload:
            following = [
                index for index in candidates if index > max(mapped_payload)
            ]
            if following:
                new_location = min(following)
        if new_location is None:
            ordinal = wrapper_ordinals.get(location, 0)
            wrapper_ordinals[location] = ordinal + 1
            if ordinal >= len(candidates):
                continue
            new_location = candidates[ordinal]
        else:
            wrapper_ordinals[location] = (
                wrapper_ordinals.get(location, 0) + 1
            )
        if new_location is None:
            continue
        aligned[old_location] = new_location
        aligned.pop(old_open, None)
        aligned.pop(old_close, None)
        # The wrapper close corresponds to the postfix location keyword. The
        # open intentionally remains unaligned so a comment immediately inside
        # it falls forward to the payload type instead of a later `returns (`.
        aligned[old_close] = new_location


def _realign_declaration_lhs_tokens(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Map a Classic class/instance lhs to the generated trait argument."""

    for declaration_index, token in enumerate(before):
        if (
            token.text not in {"class", "instance"}
            or declaration_index not in aligned
        ):
            continue
        head_start = declaration_index + 1
        arrow_relative = find_top(before[head_start:], "=>")
        if arrow_relative is not None:
            head_start += arrow_relative + 1
        colon_relative = find_top(before[head_start:], ":")
        if colon_relative is None:
            continue
        colon = head_start + colon_relative
        lhs = range(head_start, colon)

        old_trait_open = next(
            (
                index
                for index in range(colon + 1, len(before))
                if before[index].text in {"(", "<"}
            ),
            None,
        )
        old_trait_leaf = (
            old_trait_open - 1
            if old_trait_open is not None
            else len(before) - 1
        )
        if old_trait_leaf not in aligned:
            continue
        generic_open = aligned[old_trait_leaf] + 1
        if (
            generic_open >= len(after)
            or after[generic_open].text != "<"
        ):
            continue
        generic_close = matching_index(after, generic_open)
        if generic_close is None:
            continue
        first_arg_end = generic_close
        depth: list[str] = []
        for index in range(generic_open + 1, generic_close):
            if not depth and after[index].text == ",":
                first_arg_end = index
                break
            _depth_step(depth, after[index].text)
        first_arg = range(generic_open + 1, first_arg_end)

        for old_index in lhs:
            candidates = [
                new_index
                for new_index in first_arg
                if (
                    before[old_index].kind,
                    before[old_index].text,
                )
                == (
                    after[new_index].kind,
                    after[new_index].text,
                )
            ]
            if len(candidates) == 1:
                aligned[old_index] = candidates[0]


def _realign_forall_binder_tokens(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Map Classic `forall` binder names to generated declaration generics."""

    for forall_index, token in enumerate(before):
        if token.text != "forall":
            continue
        dot_relative = find_top(
            before[forall_index + 1 :],
            ".",
            angles=False,
        )
        if dot_relative is None:
            continue
        dot = forall_index + 1 + dot_relative
        declaration_index = next(
            (
                index
                for index in range(dot + 1, len(before))
                if before[index].text in {"function", "class", "instance"}
            ),
            None,
        )
        if declaration_index not in aligned:
            continue
        declaration = before[declaration_index].text
        new_declaration = aligned[declaration_index]
        generic_open = (
            new_declaration + 1
            if declaration == "instance"
            else new_declaration + 2
        )
        if generic_open >= len(after) or after[generic_open].text != "<":
            continue
        generic_close = matching_index(after, generic_open)
        if generic_close is None:
            continue

        for old_index in range(forall_index + 1, dot):
            if before[old_index].kind != "word":
                continue
            candidates = [
                new_index
                for new_index in range(generic_open + 1, generic_close)
                if (
                    before[old_index].kind,
                    before[old_index].text,
                )
                == (
                    after[new_index].kind,
                    after[new_index].text,
                )
            ]
            if len(candidates) == 1:
                aligned[old_index] = candidates[0]


def _realign_trait_head_arguments(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Account for the lhs inserted before explicit class/instance arguments."""

    for declaration_index, token in enumerate(before):
        if (
            token.text not in {"class", "instance"}
            or declaration_index not in aligned
        ):
            continue
        colon_relative = find_top(before[declaration_index + 1 :], ":")
        if colon_relative is None:
            continue
        colon = declaration_index + 1 + colon_relative
        old_open = next(
            (
                index
                for index in range(colon + 1, len(before))
                if before[index].text in {"(", "<"}
            ),
            None,
        )
        if old_open is None:
            continue
        old_close = matching_index(before, old_open)
        if old_close is None or old_open == colon + 1:
            continue
        old_name = old_open - 1
        if old_name not in aligned:
            continue
        new_open = aligned[old_name] + 1
        if new_open >= len(after) or after[new_open].text != "<":
            continue
        new_close = matching_index(after, new_open)
        if new_close is None:
            continue

        depth: list[str] = []
        inserted_lhs_comma = None
        for index in range(new_open + 1, new_close):
            if not depth and after[index].text == ",":
                inserted_lhs_comma = index
                break
            _depth_step(depth, after[index].text)
        if inserted_lhs_comma is None:
            continue

        new_cursor = inserted_lhs_comma + 1
        for old_index in range(old_open + 1, old_close):
            match = next(
                (
                    new_index
                    for new_index in range(new_cursor, new_close)
                    if (
                        before[old_index].kind,
                        before[old_index].text,
                    )
                    == (
                        after[new_index].kind,
                        after[new_index].text,
                    )
                ),
                None,
            )
            if match is not None:
                aligned[old_index] = match
                new_cursor = match + 1

        aligned.pop(old_open, None)
        aligned[old_close] = new_close


def _realign_predicate_context_tokens(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Keep moved constraint comments anchored in the generated `where` tail."""

    declaration_index = next(
        (
            index
            for index, token in enumerate(before)
            if token.text in {"function", "class", "instance"}
        ),
        None,
    )
    where_index = next(
        (
            index
            for index, token in enumerate(after)
            if token.text == "where"
        ),
        None,
    )
    if declaration_index is None or where_index is None:
        return

    context_ranges: list[range] = []
    prefix_start = 0
    forall_index = next(
        (
            index
            for index in range(declaration_index)
            if before[index].text == "forall"
        ),
        None,
    )
    if forall_index is not None:
        dot_relative = find_top(
            before[forall_index + 1 : declaration_index],
            ".",
            angles=False,
        )
        if dot_relative is not None:
            dot = forall_index + 1 + dot_relative
            binder = before[forall_index + 1 : dot]
            if find_top(binder, ":") is not None:
                context_ranges.append(range(forall_index + 1, dot))
            prefix_start = dot + 1

    prefix_arrow = find_top(
        before[prefix_start:declaration_index],
        "=>",
        angles=False,
    )
    if prefix_arrow is not None:
        prefix_arrow += prefix_start
        context_ranges.append(range(prefix_start, prefix_arrow))

    tail_arrow = find_top(
        before[declaration_index + 1 :],
        "=>",
        angles=False,
    )
    if tail_arrow is not None:
        tail_arrow += declaration_index + 1
        context_ranges.append(range(declaration_index + 1, tail_arrow))

    existing_where = find_top(before[declaration_index + 1 :], "where")
    if existing_where is not None:
        existing_where += declaration_index + 1
        context_ranges.append(range(existing_where + 1, len(before)))

    new_cursor = where_index + 1
    for context_range in context_ranges:
        _realign_token_range(
            before,
            after,
            aligned,
            context_range.start,
            context_range.stop,
            new_cursor,
            len(after),
        )
        mapped = [
            aligned[index]
            for index in context_range
            if index in aligned and aligned[index] >= new_cursor
        ]
        if mapped:
            new_cursor = max(mapped) + 1

    context_indices = [
        index for context_range in context_ranges for index in context_range
    ]
    context_set = set(context_indices)
    for old_open in context_indices:
        if (
            before[old_open].text != "("
            or old_open == 0
            or old_open - 1 not in context_set
            or before[old_open - 1].kind != "word"
            or old_open - 1 not in aligned
        ):
            continue
        old_close = matching_index(before, old_open)
        if old_close is None or old_close not in context_set:
            continue
        new_open = aligned[old_open - 1] + 1
        if (
            new_open <= where_index
            or new_open >= len(after)
            or after[new_open].text != "<"
        ):
            continue
        new_close = matching_index(after, new_open)
        if new_close is None:
            continue
        aligned[old_open] = new_open
        aligned[old_close] = new_close

    for context_range in context_ranges:
        if not context_range:
            continue
        old_open = context_range.start
        old_close = context_range.stop - 1
        if (
            before[old_open].text != "("
            or before[old_close].text != ")"
            or matching_index(before, old_open) != old_close
        ):
            continue
        mapped_context = [
            aligned[index]
            for index in context_range
            if index in aligned and aligned[index] > where_index
        ]
        if mapped_context:
            aligned[old_close] = max(mapped_context)


def _realign_import_tokens(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Align import path, alias, and selector roles across reordered syntax."""

    if (
        not before
        or not after
        or before[0].text != "import"
        or after[0].text != "import"
    ):
        return
    old_end = len(before) - 1 if before[-1].text == ";" else len(before)
    new_end = len(after) - 1 if after[-1].text == ";" else len(after)
    old_as = find_top(before[1:old_end], "as", angles=False)
    if old_as is not None:
        old_as += 1
    new_as = find_top(after[1:new_end], "as", angles=False)
    if new_as is not None:
        new_as += 1
    new_from = find_top(after[1:new_end], "from", angles=False)
    if new_from is not None:
        new_from += 1

    if old_as is not None and new_as is not None and new_from is not None:
        aligned[old_as] = new_as
        _realign_token_range(
            before,
            after,
            aligned,
            1,
            old_as,
            new_from + 1,
            new_end,
        )
        _realign_token_range(
            before,
            after,
            aligned,
            old_as + 1,
            old_end,
            new_as + 1,
            new_from,
        )
        return

    old_brace = find_top(before[1:old_end], "{", angles=False)
    if old_brace is not None:
        old_brace += 1
        old_close = matching_index(before, old_brace)
        if old_close is None:
            return
        old_path_end = old_brace - 1 if before[old_brace - 1].text == "." else old_brace
        if new_from is not None:
            new_path_start = new_from + 1
        else:
            new_path_start = 1
        _realign_token_range(
            before,
            after,
            aligned,
            1,
            old_path_end,
            new_path_start,
            new_end,
        )

        new_brace = find_top(after[1:new_end], "{", angles=False)
        if new_brace is not None:
            new_brace += 1
            new_close = matching_index(after, new_brace)
            if new_close is None:
                return
            aligned[old_brace] = new_brace
            aligned[old_close] = new_close
            _realign_token_range(
                before,
                after,
                aligned,
                old_brace + 1,
                old_close,
                new_brace + 1,
                new_close,
            )
            removed_anchor = new_close
        else:
            mapped_path = [
                aligned[index]
                for index in range(1, old_path_end)
                if index in aligned
            ]
            removed_anchor = max(mapped_path) if mapped_path else 0

        for old_index in range(old_path_end, old_end):
            if old_index not in aligned:
                aligned[old_index] = removed_anchor
        return

    if new_as is not None and new_from is not None:
        # Classic bare imports generate an alias from the final path segment.
        _realign_token_range(
            before,
            after,
            aligned,
            1,
            old_end,
            new_from + 1,
            new_end,
        )


def _aligned_token_indices(
    before: Sequence[Token], after: Sequence[Token]
) -> dict[int, int]:
    """Align unchanged header tokens, including uniquely moved modifiers."""

    rows = len(before) + 1
    columns = len(after) + 1
    lengths = [[0] * columns for _ in range(rows)]
    for left in range(len(before) - 1, -1, -1):
        for right in range(len(after) - 1, -1, -1):
            if (
                before[left].kind,
                before[left].text,
            ) == (
                after[right].kind,
                after[right].text,
            ):
                lengths[left][right] = lengths[left + 1][right + 1] + 1
            else:
                lengths[left][right] = max(
                    lengths[left + 1][right],
                    lengths[left][right + 1],
                )

    aligned: dict[int, int] = {}
    left = 0
    right = 0
    while left < len(before) and right < len(after):
        if (
            before[left].kind,
            before[left].text,
        ) == (
            after[right].kind,
            after[right].text,
        ):
            aligned[left] = right
            left += 1
            right += 1
        elif lengths[left + 1][right] >= lengths[left][right + 1]:
            left += 1
        else:
            right += 1

    _realign_type_application_delimiters(before, after, aligned)

    used_after = set(aligned.values())
    renamed_keywords = {
        "data": "enum",
        "type": "alias",
        "class": "trait",
        "instance": "impl",
    }
    for old_text, new_text in renamed_keywords.items():
        before_indices = [
            index
            for index, token in enumerate(before)
            if index not in aligned and token.text == old_text
        ]
        after_indices = [
            index
            for index, token in enumerate(after)
            if index not in used_after and token.text == new_text
        ]
        if len(before_indices) == 1 and len(after_indices) == 1:
            aligned[before_indices[0]] = after_indices[0]
            used_after.add(after_indices[0])

    # `forall T. declaration` moves the binder into the declaration head.
    # Anchor comments after the removed dot to the generated binder close
    # instead of turning them into declaration-leading documentation.
    for dot_index, token in enumerate(before):
        if token.text != "." or dot_index in aligned:
            continue
        forall_index = next(
            (
                index
                for index in range(dot_index - 1, -1, -1)
                if before[index].text == "forall"
            ),
            None,
        )
        declaration_index = next(
            (
                index
                for index in range(dot_index + 1, len(before))
                if before[index].text in {"function", "class", "instance"}
            ),
            None,
        )
        if forall_index is None or declaration_index not in aligned:
            continue
        declaration = before[declaration_index].text
        new_declaration = aligned[declaration_index]
        generic_open = (
            new_declaration + 1
            if declaration == "instance"
            else new_declaration + 2
        )
        if generic_open >= len(after) or after[generic_open].text != "<":
            continue
        generic_close = matching_index(after, generic_open)
        if generic_close is not None:
            aligned[dot_index] = generic_close
            used_after.add(generic_close)

    # A Classic prefix modifier moves from before `function` to the end of
    # the parameter list, so it falls outside the order-preserving alignment.
    # Recover any such token only when both sides have one unambiguous match.
    unmatched_before: dict[tuple[str, str], list[int]] = {}
    unmatched_after: dict[tuple[str, str], list[int]] = {}
    for index, token in enumerate(before):
        if index not in aligned:
            unmatched_before.setdefault((token.kind, token.text), []).append(index)
    for index, token in enumerate(after):
        if index not in used_after:
            unmatched_after.setdefault((token.kind, token.text), []).append(index)
    for key, before_indices in unmatched_before.items():
        after_indices = unmatched_after.get(key, [])
        if len(before_indices) == 1 and len(after_indices) == 1:
            aligned[before_indices[0]] = after_indices[0]
    _realign_type_application_delimiters(before, after, aligned)
    _realign_declaration_lhs_tokens(before, after, aligned)
    _realign_forall_binder_tokens(before, after, aligned)
    _realign_trait_head_arguments(before, after, aligned)
    _realign_predicate_context_tokens(before, after, aligned)
    _realign_callable_signature_tokens(before, after, aligned)
    _realign_location_wrapper_delimiters(before, after, aligned)
    _realign_import_tokens(before, after, aligned)
    return aligned


def _comment_insertion(
    original: str,
    comment: Token,
    next_token: Token | None,
    replacement: str,
    position: int,
) -> str:
    prefix = ""
    if (
        position > 0
        and not replacement[position - 1].isspace()
        and replacement[position - 1] not in "([{<"
    ):
        prefix = " "

    following = original[
        comment.end : next_token.start if next_token is not None else len(original)
    ]
    line_break = re.search(r"\r?\n[ \t]*", following)
    if comment.text.startswith("//"):
        suffix = line_break.group(0) if line_break is not None else "\n"
    elif line_break is not None:
        suffix = line_break.group(0)
    elif position < len(replacement) and replacement[position].isspace():
        suffix = ""
    else:
        suffix = " "
    return prefix + comment.text + suffix


def _aligned_token_position(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: Mapping[int, int],
    old_index: int,
    *,
    trailing: bool,
) -> int:
    new_index = aligned[old_index]
    token = after[new_index]
    if (
        len(token.text) > 1
        and set(token.text) == {">"}
        and before[old_index].text in {")", ">"}
    ):
        old_closers = sorted(
            index
            for index, mapped in aligned.items()
            if (
                mapped == new_index
                and before[index].text in {")", ">"}
            )
        )
        rank = old_closers.index(old_index)
        offset = rank + 1 if trailing else rank
        return token.start + min(offset, len(token.text))
    return token.end if trailing else token.start


def _append_generated_suffix(fragment: str, suffix: str) -> str:
    """Keep generated punctuation outside a trailing line comment."""

    tokens = [token for token in lex(fragment) if token.kind != "ws"]
    if (
        tokens
        and tokens[-1].kind == "comment"
        and tokens[-1].text.startswith("//")
        and "\n" not in fragment[tokens[-1].end :]
        and "\r" not in fragment[tokens[-1].end :]
    ):
        return fragment + "\n" + suffix.lstrip()
    return fragment + suffix


def _has_nested_location_wrapper(tokens: Sequence[Token]) -> bool:
    for index in range(len(tokens) - 1):
        if (
            tokens[index].text not in LOCATIONS
            or tokens[index + 1].text != "("
        ):
            continue
        close = matching_index(tokens, index + 1)
        if close is None:
            continue
        if any(
            tokens[nested].text in LOCATIONS
            and nested + 1 < close
            and tokens[nested + 1].text == "("
            for nested in range(index + 2, close)
        ):
            return True
    return False


def _with_preserved_comments(
    source: str,
    start: int,
    end: int,
    replacement: str,
) -> str:
    original = source[start:end]
    original_tokens = lex(original)
    before = [token for token in original_tokens if token.kind not in TRIVIA]
    after = significant(replacement)
    if [
        (token.kind, token.text)
        for token in _expand_type_angle_closers(before)
    ] == [
        (token.kind, token.text)
        for token in _expand_type_angle_closers(after)
    ]:
        return original

    comments = [token for token in original_tokens if token.kind == "comment"]
    if not comments:
        return replacement

    aligned = _aligned_token_indices(before, after)
    insertions: dict[int, list[tuple[int, str]]] = {}
    preserve_source_order = _has_nested_location_wrapper(before)
    placements: list[
        tuple[int, int, Token, Token | None]
    ] = []

    for order, comment in enumerate(comments):
        previous_index = next(
            (
                index
                for index in range(len(before) - 1, -1, -1)
                if before[index].end <= comment.start
            ),
            None,
        )
        next_index = next(
            (
                index
                for index, token in enumerate(before)
                if token.start >= comment.end
            ),
            None,
        )
        previous = before[previous_index] if previous_index is not None else None
        next_token = before[next_index] if next_index is not None else None
        trailing = previous is not None and not any(
            newline in original[previous.end : comment.start]
            for newline in ("\n", "\r")
        )

        if trailing and previous_index in aligned:
            position = _aligned_token_position(
                before,
                after,
                aligned,
                previous_index,
                trailing=True,
            )
        elif next_index in aligned:
            next_position = _aligned_token_position(
                before,
                after,
                aligned,
                next_index,
                trailing=False,
            )
            if next_position == 0 and previous_index in aligned:
                position = _aligned_token_position(
                    before,
                    after,
                    aligned,
                    previous_index,
                    trailing=True,
                )
            elif next_position == 0 and after:
                position = after[0].end
            else:
                position = next_position
        elif previous_index in aligned:
            position = _aligned_token_position(
                before,
                after,
                aligned,
                previous_index,
                trailing=True,
            )
        else:
            position = after[0].end if after else 0

        if preserve_source_order:
            placements.append((order, position, comment, next_token))
        else:
            text = _comment_insertion(
                original,
                comment,
                next_token,
                replacement,
                position,
            )
            insertions.setdefault(position, []).append((order, text))

    # Token rewrites can move a wrapper keyword past its payload.  Preserve
    # source comment order even when alignment would otherwise attach an
    # earlier comment to that moved token after a later payload comment.
    monotonic: list[tuple[int, int, Token, Token | None]] = []
    next_position = len(replacement)
    for order, position, comment, next_token in reversed(placements):
        position = min(position, next_position)
        monotonic.append((order, position, comment, next_token))
        next_position = position

    for order, position, comment, next_token in reversed(monotonic):
        text = _comment_insertion(
            original,
            comment,
            next_token,
            replacement,
            position,
        )
        insertions.setdefault(position, []).append((order, text))

    result = replacement
    for position in sorted(insertions, reverse=True):
        text = "".join(
            item
            for _, item in sorted(insertions[position], key=lambda entry: entry[0])
        )
        result = result[:position] + text + result[position:]
    return result


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


def _mark_expression_type_region(
    tokens: Sequence[Token],
    start: int,
    limit: int,
    boundaries: set[str],
    marked: set[int],
) -> None:
    """Mark a type span using the same generic-aware scan as migration."""

    tail = _split_type_angle_operator_tokens(tokens[start:limit])
    end = _type_expression_end(
        tail,
        0,
        boundaries,
        word_boundaries={"as", "else", "then"},
    )
    if end <= 0:
        return
    end_position = tail[end - 1].end
    marked.update(
        index
        for index in range(start, limit)
        if tokens[index].start < end_position
    )


def _body_type_tokens(
    tokens: Sequence[Token], body_start: int, body_end: int
) -> set[int]:
    """Conservatively mark type-only regions inside an executable body."""

    marked: set[int] = set()
    for index in range(body_start, body_end):
        text = tokens[index].text
        if text == ":":
            if _is_ternary_colon(tokens, index):
                continue
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
            _mark_expression_type_region(
                tokens,
                index + 1,
                body_end,
                FUNCTION_TYPE_CONVERSION_BOUNDARIES
                | {"{", "else", "then"},
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

    if has_comment_marker(source, KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER):
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


def _closes_type_arguments(
    tokens: Sequence[Token],
    close_index: int,
) -> bool:
    close = tokens[close_index].text
    if not close or set(close) != {">"}:
        return False
    depth = len(close)
    for index in range(close_index - 1, -1, -1):
        text = tokens[index].text
        if text in {";", "{", "}"}:
            return False
        if text and set(text) == {">"}:
            depth += len(text)
        elif text == "<":
            depth -= 1
            if depth == 0:
                return (
                    index > 0
                    and tokens[index - 1].kind == "word"
                )
    return False


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
    if (
        previous.text
        and set(previous.text) == {">"}
        and _closes_type_arguments(tokens, dot_index - 1)
    ):
        return False
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

    if has_comment_marker(source, KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER):
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
        if token.text not in {"alias", "type"}:
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
        params_open = "(" if token.text == "type" else "<"
        if cursor < equals and body[cursor].text == params_open:
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
        rhs_tokens = body[equals + 1 :]
        generic = "<" + ", ".join(params) + ">" if params else ""
        rhs = render_type(rhs_tokens)
        if not rhs:
            continue
        if (
            token.text == "alias"
            and [item.text for item in significant(rhs)]
            == [item.text for item in rhs_tokens]
        ):
            continue
        replacement = f"alias {name}{generic} = {rhs};"
        replacement = _with_preserved_comments(
            source, token.start, tokens[end].end, replacement
        )
        replacements.append((token.start, tokens[end].end, replacement))
    return replace_spans(source, replacements)


def migrate_value_type_underlying_types(source: str) -> str:
    """Canonicalize the underlying type in `type Name is Type;`."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if (
            token.text != "type"
            or index + 2 >= len(tokens)
            or tokens[index + 1].kind != "word"
        ):
            continue
        is_index = index + 2
        if tokens[is_index].text == "<":
            binder_close = matching_index(tokens, is_index)
            if binder_close is None:
                continue
            is_index = binder_close + 1
        if (
            is_index >= len(tokens)
            or tokens[is_index].text != "is"
        ):
            continue
        end = _statement_end(tokens, index)
        if end is None or is_index + 1 >= end:
            continue
        underlying = list(tokens[is_index + 1 : end])
        rendered = render_type(underlying)
        if (
            not rendered
            or [item.text for item in significant(rendered)]
            == [item.text for item in underlying]
        ):
            continue
        replacement = _with_preserved_comments(
            source,
            underlying[0].start,
            underlying[-1].end,
            rendered,
        )
        replacements.append(
            (underlying[0].start, underlying[-1].end, replacement)
        )
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


def _reject_unsupported_header_tokens(
    source: str,
    subject: str,
    tokens: Sequence[Token],
) -> None:
    if not tokens:
        return
    line, column = _source_line_column(source, tokens[0].start)
    spelling = join_tokens(tokens)
    raise ValueError(
        f"cannot migrate {subject} header at line {line}, column {column}: "
        f"unsupported modifier or base-constructor syntax `{spelling}`"
    )


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
        candidate_start = _previous_boundary(tokens, index)
        variables, constraints, modifiers, start_index = _function_prefix(
            tokens, candidate_start, index
        )

        cursor = index + 2
        existing_variables: list[str] = []
        if tokens[cursor].text == "<":
            generic_close = matching_index(tokens, cursor)
            if generic_close is None:
                continue
            existing_variables = [
                join_tokens(part)
                for part in split_top(tokens[cursor + 1 : generic_close], ",")
                if part
            ]
            cursor = generic_close + 1
        if cursor >= len(tokens) or tokens[cursor].text != "(":
            continue
        close = matching_index(tokens, cursor)
        if close is None:
            continue
        end = _header_boundary(tokens, close + 1)
        if end is None:
            continue
        tail = list(tokens[close + 1 : end])

        where = find_top(tail, "where")
        signature_tail = tail[:where] if where is not None else tail
        existing_constraints = tail[where + 1 :] if where is not None else []
        arrow = find_top(signature_tail, "->")
        returns = (
            None
            if arrow is not None
            else find_top(signature_tail, "returns")
        )

        return_tokens: list[Token] = []
        has_returns = returns is not None
        if arrow is not None:
            unsupported = [
                item
                for item in signature_tail[:arrow]
                if item.text not in MODIFIERS
            ]
            _reject_unsupported_header_tokens(
                source, "function", unsupported
            )
            return_tokens = signature_tail[arrow + 1 :]
            # Modifiers after the parameter list are already canonical; retain
            # them if a partially migrated file still has an old return arrow.
            modifiers.extend(
                item.text
                for item in signature_tail[:arrow]
                if item.text in MODIFIERS
            )
        elif returns is not None:
            if (
                returns + 1 >= len(signature_tail)
                or signature_tail[returns + 1].text != "("
            ):
                continue
            returns_close = matching_index(signature_tail, returns + 1)
            if returns_close != len(signature_tail) - 1:
                continue
            return_tokens = signature_tail[returns + 2 : returns_close]
            unsupported = [
                item
                for item in signature_tail[:returns]
                if item.text not in MODIFIERS
            ]
            _reject_unsupported_header_tokens(
                source, "function", unsupported
            )
            modifiers.extend(
                item.text
                for item in signature_tail[:returns]
                if item.text in MODIFIERS
            )
        elif signature_tail:
            unsupported = [
                item
                for item in signature_tail
                if item.text not in MODIFIERS
            ]
            _reject_unsupported_header_tokens(
                source, "function", unsupported
            )
            modifiers.extend(
                item.text for item in signature_tail if item.text in MODIFIERS
            )

        predicates = render_predicates(constraints) if constraints else []
        if constraints and not predicates:
            continue
        existing_predicates = (
            render_predicates(existing_constraints)
            if existing_constraints
            else []
        )
        if existing_constraints and not existing_predicates:
            continue
        predicates.extend(existing_predicates)

        params = render_params(tokens[cursor + 1 : close])
        replacement = f"function {name_token.text}"
        variables = list(dict.fromkeys([*existing_variables, *variables]))
        if variables:
            replacement += "<" + ", ".join(variables) + ">"
        replacement += f"({params})"
        if modifiers:
            replacement += " " + " ".join(dict.fromkeys(modifiers))
        if arrow is not None or has_returns:
            rendered_return = (
                render_return_clause_items(return_tokens)
                if has_returns
                else render_return_type(return_tokens)
            )
            replacement += " returns (" + rendered_return + ")"
        if predicates:
            replacement += " where " + ", ".join(predicates)
        if tokens[end].text == "{":
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
        arrow = find_top(tail, "->")
        returns = None if arrow is not None else find_top(tail, "returns")

        return_tokens: list[Token] = []
        has_returns = returns is not None
        if arrow is not None:
            _reject_unsupported_header_tokens(
                source, "lambda", tail[:arrow]
            )
            return_tokens = tail[arrow + 1 :]
        elif returns is not None:
            if (
                returns != 0
                or returns + 1 >= len(tail)
                or tail[returns + 1].text != "("
            ):
                continue
            _reject_unsupported_header_tokens(
                source, "lambda", tail[:returns]
            )
            returns_close = matching_index(tail, returns + 1)
            if returns_close != len(tail) - 1:
                continue
            return_tokens = tail[returns + 2 : returns_close]
        else:
            _reject_unsupported_header_tokens(source, "lambda", tail)

        params = render_params(tokens[open_index + 1 : close])
        replacement = f"lam ({params})"
        if arrow is not None or has_returns:
            rendered_return = (
                render_return_clause_items(return_tokens)
                if has_returns
                else render_return_type(return_tokens)
            )
            replacement += " returns (" + rendered_return + ")"
        replacement += " "
        replacement = _with_preserved_comments(
            source, token.start, tokens[end].start, replacement
        )
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
        arrow = find_top(tail, "->")
        returns = None if arrow is not None else find_top(tail, "returns")
        start_index = _previous_boundary(tokens, index)
        prefix = list(tokens[start_index:index])
        modifiers = [item.text for item in prefix if item.text in MODIFIERS]
        if prefix and len(modifiers) != len(prefix):
            start_index = index
            modifiers = []

        return_tokens: list[Token] = []
        has_returns = returns is not None
        if arrow is not None:
            unsupported = [
                item for item in tail[:arrow] if item.text not in MODIFIERS
            ]
            _reject_unsupported_header_tokens(
                source, token.text, unsupported
            )
            modifiers.extend(
                item.text for item in tail[:arrow] if item.text in MODIFIERS
            )
            return_tokens = tail[arrow + 1 :]
        elif returns is not None:
            if (
                returns + 1 >= len(tail)
                or tail[returns + 1].text != "("
            ):
                continue
            unsupported = [
                item
                for item in tail[:returns]
                if item.text not in MODIFIERS
            ]
            _reject_unsupported_header_tokens(
                source, token.text, unsupported
            )
            returns_close = matching_index(tail, returns + 1)
            if returns_close != len(tail) - 1:
                continue
            modifiers.extend(
                item.text
                for item in tail[:returns]
                if item.text in MODIFIERS
            )
            return_tokens = tail[returns + 2 : returns_close]
        else:
            unsupported = [
                item for item in tail if item.text not in MODIFIERS
            ]
            _reject_unsupported_header_tokens(
                source, token.text, unsupported
            )
            modifiers.extend(item.text for item in tail if item.text in MODIFIERS)
        params = render_params(tokens[open_index + 1 : close])
        replacement = f"{token.text}({params})"
        if modifiers:
            replacement += " " + " ".join(dict.fromkeys(modifiers))
        if arrow is not None or has_returns:
            rendered_return = (
                render_return_clause_items(return_tokens)
                if has_returns
                else render_return_type(return_tokens)
            )
            replacement += " returns (" + rendered_return + ")"
        replacement += " "
        start = tokens[start_index].start
        replacement = _with_preserved_comments(
            source, start, tokens[end].start, replacement
        )
        replacements.append(
            (start, tokens[end].start, replacement)
        )
    return replace_spans(source, replacements)


def migrate_incomplete_arrows(source: str) -> str:
    """Rewrite return arrows in deliberately incomplete function headers.

    Complete declarations are handled by ``migrate_functions`` and
    ``migrate_lambdas``.  This fallback is for negative parser fixtures whose
    missing body or semicolon prevents those structural passes from finding a
    whole header.  A closed parameter list is required: without ``)`` an arrow
    could belong to a parameter type, so rewriting it would be ambiguous.  The
    original structural error is retained.
    """

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "->" or index + 1 >= len(tokens):
            continue
        start = _previous_boundary(tokens, index)
        callable_index = next(
            (
                cursor
                for cursor in range(index - 1, start - 1, -1)
                if (
                    tokens[cursor].text
                    in {"constructor", "fallback", "function", "lam"}
                    and (
                        tokens[cursor].text != "function"
                        or (
                            cursor + 1 < len(tokens)
                            and tokens[cursor + 1].kind == "word"
                        )
                    )
                )
            ),
            None,
        )
        if callable_index is None:
            continue
        params_open = next(
            (
                cursor
                for cursor in range(callable_index + 1, index)
                if tokens[cursor].text == "("
            ),
            None,
        )
        if params_open is None:
            continue
        params_close = matching_index(tokens, params_open)
        if params_close is None or params_close >= index:
            continue
        stack: list[str] = []
        end = index + 1
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if (
                cursor > index + 1
                and not stack
                and text in {"where", "{", "}", ";"}
            ):
                break
            _depth_step(stack, text)
            end = cursor + 1
        return_tokens = list(tokens[index + 1 : end])
        if not return_tokens:
            continue
        rendered = render_return_type(return_tokens)
        replacement = _with_preserved_comments(
            source,
            token.start,
            tokens[end - 1].end,
            f"returns ({rendered})",
        )
        replacements.append((token.start, tokens[end - 1].end, replacement))
    return replace_spans(source, replacements)


def reject_remaining_classic_arrows(source: str) -> None:
    """Reject Classic type arrows left outside a safely rendered type span."""

    for token in significant(source):
        if token.text != "->":
            continue
        line, column = _source_line_column(source, token.start)
        raise ValueError(
            "cannot migrate Classic type arrow at "
            f"line {line}, column {column}: rewrite the complete type "
            "`A -> B` as `function(A) returns (B)` or isolate it in a "
            "declaration the migrator can render safely"
        )


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
        replacement = _with_preserved_comments(
            source, token.start, tokens[end].start, replacement
        )
        replacements.append((token.start, tokens[end].start, replacement))
    return replace_spans(source, replacements)


def migrate_field_types(source: str) -> str:
    tokens = significant(source)
    executable_bodies = _executable_regions(tokens)[0]
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != ":" or index == 0 or tokens[index - 1].kind != "word":
            continue
        if _is_expression_annotation_colon(
            tokens, index, executable_bodies
        ):
            continue
        name_index = index - 1
        before = tokens[name_index - 1].text if name_index else None
        if before not in {None, "{", "}", ";"}:
            continue
        stack: list[str] = []
        end = None
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if not stack and text in {"=", ";"}:
                end = cursor
                break
            if not stack and text in {"{", "}"}:
                break
            _depth_step(stack, text)
        if end is None:
            continue
        ty = render_type(tokens[index + 1 : end])
        if not ty:
            continue
        replacement = _with_preserved_comments(
            source, token.start, tokens[end].start, ": " + ty
        )
        replacements.append((token.start, tokens[end].start, replacement))
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
        head = _with_preserved_comments(
            source,
            tokens[arm_start].end,
            tokens[arrow].start,
            head,
        )
        # Keep arm boundaries on their own lines.  Appending the next arm or
        # closing brace to a ``//`` comment would silently comment it out.
        rendered_arms.append(
            _append_generated_suffix(head, " {\n") + body + "\n}"
        )
    scrutinee = source[tokens[index].end : tokens[brace].start].strip()
    if (
        scrutinee.startswith("(")
        and scrutinee.endswith(")")
        and tokens[index + 1].text == "("
        and matching_index(tokens, index + 1) == brace - 1
    ):
        scrutinee = source[tokens[index + 1].end : tokens[brace - 1].start].strip()
    replacement = (
        "match ("
        + _append_generated_suffix(scrutinee, ") {\n")
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
        replacement = (
            "("
            + _append_generated_suffix(condition, " ? ")
            + _append_generated_suffix(then_expr, " : ")
            + _append_generated_suffix(else_expr, ")")
        )
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
        replacement = " (" + _append_generated_suffix(condition, ") ")
        replacements.append((token.end, tokens[boundary].start, replacement))
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
    unmatched_colons = 0
    for index in range(colon_index - 1, -1, -1):
        text = tokens[index].text
        if text in {")", "]", "}"}:
            stack.append(text)
        elif text in {"(", "[", "{"}:
            if stack:
                stack.pop()
            else:
                break
        elif not stack:
            if text == ":":
                unmatched_colons += 1
            elif text == "?":
                if unmatched_colons:
                    unmatched_colons -= 1
                else:
                    return True
            elif text in {
                ";",
                ",",
                "=",
                ":=",
                "=>",
                "return",
                "case",
            }:
                break
    return False


def _is_expression_annotation_colon(
    tokens: Sequence[Token],
    index: int,
    executable_bodies: Sequence[tuple[int, int]] | None = None,
) -> bool:
    if (
        tokens[index].text != ":"
        or index == 0
        or index + 1 >= len(tokens)
    ):
        return False
    if tokens[index + 1].text in {"#", "?"}:
        return False
    if tokens[index + 1].kind != "word" and tokens[index + 1].text not in {
        "(",
        "@",
    }:
        return False
    if (
        _inside_function_parameter_list(tokens, index)
        or _is_ternary_colon(tokens, index)
    ):
        return False

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
    if (
        statement_prefix & {"export", "import", "pragma"}
        or "where" in statement_prefix
    ):
        return False
    if "let" in statement_prefix:
        let_index = next(
            cursor
            for cursor in range(statement_start + 1, index)
            if tokens[cursor].text == "let"
        )
        if not any(
            tokens[cursor].text == "="
            for cursor in range(let_index + 1, index)
        ):
            return False

    previous = tokens[index - 1]
    if executable_bodies is None:
        executable_bodies = _executable_regions(tokens)[0]
    inside_executable_body = any(
        body_start <= index < body_end
        for body_start, body_end in executable_bodies
    )
    if (
        not inside_executable_body
        and previous.kind == "word"
        and statement_start + 1 == index - 1
    ):
        field_end = _header_boundary(tokens, index + 1)
        if field_end is not None and tokens[field_end].text in {"=", ";"}:
            return False
    return not (
        previous.kind == "word"
        and index >= 2
        and tokens[index - 2].text in {"trait", "impl"}
    )


def _annotation_expression_start(
    tokens: Sequence[Token],
    colon_index: int,
) -> int:
    """Find the expression wrapped by a Classic trailing type annotation."""

    stack: list[str] = []
    for index in range(colon_index - 1, -1, -1):
        text = tokens[index].text
        if text in {")", "]", "}"}:
            stack.append(text)
            continue
        if text in {"(", "[", "{"}:
            if stack:
                stack.pop()
                continue
            return index + 1
        if stack:
            continue
        if text in {
            ";",
            ",",
            "=",
            ":=",
            "=>",
            "+=",
            "-=",
            "^=",
            "&=",
            "|=",
            "%=",
        } or text in {
            "return",
            "case",
            "then",
            "else",
            "if",
            "while",
            "match",
        }:
            return index + 1
    return 0


def migrate_expression_annotations(source: str) -> str:
    tokens = significant(source)
    executable_bodies = _executable_regions(tokens)[0]
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if not _is_expression_annotation_colon(
            tokens, index, executable_bodies
        ):
            continue

        type_tail = _split_type_angle_operator_tokens(
            tokens[index + 1 :]
        )
        end = _type_expression_end(
            type_tail,
            0,
            FUNCTION_TYPE_CONVERSION_BOUNDARIES | {"else", "then", "{"},
            word_boundaries={"as", "else", "then"},
        )
        type_tokens = list(type_tail[:end])
        if not type_tokens:
            continue
        _reject_dangling_type_comparison(type_tail, end)
        rendered = render_type(type_tokens)
        if not rendered:
            continue
        expression_start = _annotation_expression_start(tokens, index)
        if expression_start >= index:
            continue
        replacement_start = token.start
        preceding_end = tokens[index - 1].end
        if not source[preceding_end:token.start].strip():
            replacement_start = preceding_end
        replacement = _with_preserved_comments(
            source,
            replacement_start,
            type_tokens[-1].end,
            ") as " + rendered,
        )
        replacement = _separate_following_type_token(
            source, type_tokens[-1].end, replacement
        )
        replacements.append(
            (
                tokens[expression_start].start,
                tokens[expression_start].start,
                "(",
            )
        )
        replacements.append(
            (replacement_start, type_tokens[-1].end, replacement)
        )
    return replace_spans(source, replacements)


def _migrate_expression_types(
    source: str,
    introducer: str,
    boundaries: set[str],
    *,
    require_arrow: bool = False,
) -> str:
    """Canonicalize one complete type introduced inside an expression."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    covered_until = 0
    for index, token in enumerate(tokens):
        if (
            token.start < covered_until
            or token.text != introducer
            or index + 1 >= len(tokens)
        ):
            continue
        if introducer == "as":
            statement_start = index - 1
            while (
                statement_start >= 0
                and tokens[statement_start].text != ";"
            ):
                statement_start -= 1
            if any(
                item.text in {"import", "export", "pragma"}
                for item in tokens[statement_start + 1 : index]
            ):
                continue
        type_tail = _split_type_angle_operator_tokens(
            tokens[index + 1 :]
        )
        end = _type_expression_end(
            type_tail,
            0,
            boundaries,
            word_boundaries={"as", "else", "then"},
        )
        type_tokens = list(type_tail[:end])
        if not type_tokens:
            continue
        _reject_dangling_type_comparison(type_tail, end)
        if require_arrow and not any(
            item.text == "->" for item in type_tokens
        ):
            continue
        # The outer type renderer recursively canonicalizes nested proxy or
        # conversion types.  Never schedule a second overlapping inner span,
        # even when the outer token sequence is already canonical.
        covered_until = type_tokens[-1].end
        rendered = render_type(type_tokens)
        if any(item.text == "->" for item in significant(rendered)):
            # Leave an incomplete or ambiguous span untouched.  The fixed
            # point validator will report the remaining Classic arrow.
            continue
        needs_separator = _type_span_needs_operator_separation(
            source, type_tokens, type_tail, end
        )
        if (
            [item.text for item in significant(rendered)]
            == [item.text for item in type_tokens]
            and not needs_separator
        ):
            continue
        replacement = _with_preserved_comments(
            source,
            type_tokens[0].start,
            type_tokens[-1].end,
            rendered,
        )
        replacement = _separate_following_type_token(
            source, type_tokens[-1].end, replacement
        )
        replacements.append(
            (type_tokens[0].start, type_tokens[-1].end, replacement)
        )
    return replace_spans(source, replacements)


def migrate_conversion_types(source: str) -> str:
    return _migrate_expression_types(
        source,
        "as",
        FUNCTION_TYPE_CONVERSION_BOUNDARIES | {"else", "then", "{"},
    )


def _proxy_bracket_kind(
    tokens: Sequence[Token],
    open_index: int,
) -> tuple[str, int | None]:
    close = matching_index(tokens, open_index)
    if close is None:
        return "index", None
    contents = tokens[open_index + 1 : close]
    if not contents:
        return "dynamic", close
    if len(contents) != 1 or contents[0].kind != "number":
        return "index", close
    spelling = contents[0].text
    if spelling.startswith("0X"):
        return "invalid-fixed", close
    base = 16 if spelling.lower().startswith("0x") else 10
    digits = spelling[2:] if base == 16 else spelling
    length = int(digits, base)
    if length == 0 or length > (1 << 64) - 1:
        return "invalid-fixed", close
    return "fixed", close


def _plausible_type_application_argument(
    tokens: Sequence[Token],
) -> bool:
    if not tokens:
        return False
    if (
        tokens[0].kind != "word"
        and tokens[0].text not in {"(", "@"}
    ):
        return False
    if any(
        tokens[index].text == "@"
        and tokens[index + 1].text == "comptime"
        for index in range(len(tokens) - 1)
    ):
        return False
    if tokens[0].text == "(":
        close = matching_index(tokens, 0)
        if (
            close is not None
            and close < len(tokens) - 1
            and tokens[close + 1].text != "->"
        ):
            try:
                _validated_type_suffix_end(
                    tokens, close + 1, label="type"
                )
            except ValueError:
                return False
    stack: list[str] = []
    for index, token in enumerate(tokens):
        if token.kind in {"string", "assembly"}:
            return False
        if token.kind == "number":
            if not stack or stack[-1] != "[":
                return False
        if token.text in {"true", "false"}:
            return False
        if token.text in {
            "+",
            "-",
            "*",
            "/",
            "%",
            "**",
            "==",
            "!=",
            "<=",
            ">=",
            "&&",
            "||",
            "&",
            "|",
            "^",
            "!",
            "~",
            "?",
            ":",
            "=",
            "+=",
            "-=",
        }:
            return False
        _depth_step(stack, token.text)
    try:
        rendered = render_type(tokens)
    except ValueError:
        return False
    return bool(rendered)


def _type_application_argument_is_type_only(
    tokens: Sequence[Token],
) -> bool:
    if not tokens or tokens[0].text == "@":
        return False
    if tokens[0].text in {"function", "comptime"}:
        return True
    if any(token.text in {"->", "=>"} for token in tokens):
        return True
    stack: list[str] = []
    for index, token in enumerate(tokens):
        if (
            not stack
            and index > 0
            and token.text in LOCATIONS
            and tokens[index - 1].text != "."
        ):
            return True
        _depth_step(stack, token.text)
    return any(
        tokens[index].text == "["
        and index + 1 < len(tokens)
        and tokens[index + 1].text == "]"
        for index in range(len(tokens) - 1)
    )


def _proxy_call_boundary(
    source: str,
    tokens: Sequence[Token],
) -> tuple[int | None, int | None]:
    """Classify ``@Qualified(...)`` as a call or an ambiguous old type app."""

    base_start = 0
    while (
        base_start < len(tokens)
        and tokens[base_start].text in {"@", "comptime"}
    ):
        base_start += 1
    base_tokens = tokens[base_start:]
    relative_name_end = _qualified_name_end(base_tokens)
    name_end = base_start + relative_name_end
    if (
        not relative_name_end
        or name_end >= len(tokens)
        or tokens[name_end].text != "("
        or tokens[base_start].text == "function"
    ):
        return None, None
    close = matching_index(tokens, name_end)
    if (
        close is not None
        and close + 1 < len(tokens)
        and tokens[close + 1].text == "->"
    ):
        # Runtime calls cannot be followed by a Classic type arrow.  The
        # parenthesized segment is therefore unambiguously an old type
        # application, even when its arguments are otherwise name-like.
        return None, name_end
    if (
        close is not None
        and relative_name_end == 1
        and tokens[base_start].text == "mapping"
        and find_top(tokens[name_end + 1 : close], "=>") is not None
    ):
        # A top-level fat arrow is unique to the mapping type constructor.
        # Include even malformed key/value types in the span so nested-type
        # validators cannot mistake them for runtime call arguments.
        return None, name_end
    if close is None:
        return name_end, None
    argument_tokens = tokens[name_end + 1 : close]
    arguments = split_top(argument_tokens, ",")
    plausible_types = (
        not argument_tokens
        or (
            bool(arguments)
            and all(
                _plausible_type_application_argument(argument)
                for argument in (
                    arguments[:-1]
                    if not arguments[-1]
                    else arguments
                )
            )
            and (
                bool(arguments[-1])
                or len(arguments) == 1
                or bool(arguments[-2])
            )
        )
    )
    if not plausible_types:
        return name_end, None
    nonempty_arguments = [
        argument for argument in arguments if argument
    ]
    if argument_tokens and argument_tokens[-1].text == ",":
        return None, name_end
    if any(
        _type_application_argument_is_type_only(argument)
        for argument in nonempty_arguments
    ):
        return None, name_end
    line, column = _source_line_column(
        source, tokens[name_end].start
    )
    raise ValueError(
        "cannot safely migrate ambiguous proxy call/type-application "
        f"syntax at line {line}, column {column}: write `@T<...>` for "
        "a proxy of a generic type or `(@T)(...)` for a proxy-expression "
        "call"
    )


def _reject_ambiguous_proxy_array(
    source: str,
    token: Token,
) -> None:
    line, column = _source_line_column(source, token.start)
    raise ValueError(
        "cannot safely migrate ambiguous proxy array/index syntax at "
        f"line {line}, column {column}: write `(@T)[n]` for a proxy "
        "expression index; for a proxy of a fixed-array type, introduce "
        "an alias such as `alias Fixed = T[n];` and write `@Fixed`"
    )


def migrate_proxy_types(source: str) -> str:
    tokens = significant(source)
    body_contexts = [
        (
            body_start,
            body_end,
            _body_type_tokens(tokens, body_start, body_end),
        )
        for body_start, body_end in _executable_regions(tokens)[0]
    ]
    replacements: list[tuple[int, int, str]] = []
    covered_until = 0
    for index, token in enumerate(tokens):
        if (
            token.start < covered_until
            or token.text != "@"
            or index + 1 >= len(tokens)
            or not _proxy_prefix_is_expression(
                tokens, index, body_contexts
            )
        ):
            continue
        type_tail = _split_type_angle_operator_tokens(
            tokens[index + 1 :]
        )
        (
            call_boundary,
            forced_type_application_open,
        ) = _proxy_call_boundary(source, type_tail)
        scan_tail = (
            type_tail[:call_boundary]
            if call_boundary is not None
            else type_tail
        )
        end = _type_expression_end(
            scan_tail,
            0,
            (FUNCTION_TYPE_PROXY_BOUNDARIES - {"->"})
            | {"else", "then", "{"},
            word_boundaries={"as", "else", "then"},
            # Classic proxy expressions parsed every following bracket as an
            # index postfix.  Parenthesize the proxy so the new fixed-array
            # type suffix cannot greedily change that meaning.
            allow_array_suffix=False,
            forced_type_application_opens=(
                {forced_type_application_open}
                if forced_type_application_open is not None
                else None
            ),
        )
        type_tokens = list(scan_tail[:end])
        if not type_tokens:
            continue
        _reject_dangling_type_comparison(scan_tail, end)
        rendered = render_type(type_tokens)
        rendered_tokens = significant(rendered)
        if any(item.text == "->" for item in rendered_tokens):
            continue
        changed = (
            [item.text for item in rendered_tokens]
            != [item.text for item in type_tokens]
            or _type_span_needs_operator_separation(
                source, type_tokens, scan_tail, end
            )
        )
        has_postfix_index = False
        if end < len(scan_tail) and scan_tail[end].text == "[":
            bracket_kind, close = _proxy_bracket_kind(scan_tail, end)
            if bracket_kind == "dynamic":
                assert close is not None
                end = close + 1
                while (
                    end < len(scan_tail)
                    and scan_tail[end].text == "["
                ):
                    nested_kind, nested_close = _proxy_bracket_kind(
                        scan_tail, end
                    )
                    if nested_kind not in {"dynamic", "fixed"}:
                        break
                    assert nested_close is not None
                    end = nested_close + 1
                if (
                    end < len(scan_tail)
                    and scan_tail[end].text in LOCATIONS
                ):
                    end += 1
                type_tokens = list(scan_tail[:end])
                rendered = render_type(type_tokens)
                rendered_tokens = significant(rendered)
                changed = (
                    [item.text for item in rendered_tokens]
                    != [item.text for item in type_tokens]
                    or _type_span_needs_operator_separation(
                        source, type_tokens, scan_tail, end
                    )
                )
                has_postfix_index = (
                    end < len(scan_tail)
                    and scan_tail[end].text == "["
                )
            elif (
                bracket_kind == "fixed"
                and not changed
                and type_tokens[-1].text not in LOCATIONS
            ):
                _reject_ambiguous_proxy_array(
                    source, scan_tail[end]
                )
            else:
                has_postfix_index = True
        covered_until = type_tokens[-1].end
        if not changed and not has_postfix_index:
            continue
        replacement = _with_preserved_comments(
            source,
            token.start,
            type_tokens[-1].end,
            "@" + rendered,
        )
        if has_postfix_index:
            replacement = "(" + replacement + ")"
        replacement = _separate_following_type_token(
            source, type_tokens[-1].end, replacement
        )
        replacements.append(
            (token.start, type_tokens[-1].end, replacement)
        )
    return replace_spans(source, replacements)


def migrate_enum_payload_types(source: str) -> str:
    """Canonicalize payload types in otherwise canonical enums."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if (
            token.text != "enum"
            or index + 1 >= len(tokens)
            or tokens[index + 1].kind != "word"
        ):
            continue
        body_open = index + 2
        if body_open < len(tokens) and tokens[body_open].text == "<":
            generic_close = matching_index(tokens, body_open)
            if generic_close is None:
                continue
            body_open = generic_close + 1
        if body_open >= len(tokens) or tokens[body_open].text != "{":
            continue
        body_close = matching_index(tokens, body_open)
        if body_close is None:
            continue
        for variant in split_top(
            tokens[body_open + 1 : body_close], ","
        ):
            if (
                len(variant) < 3
                or variant[0].kind != "word"
                or variant[1].text != "("
            ):
                continue
            payload_close = matching_index(variant, 1)
            if payload_close != len(variant) - 1:
                continue
            for payload in split_top(
                variant[2:payload_close], ","
            ):
                if not payload:
                    continue
                rendered = render_type(payload)
                if any(
                    item.text == "->" for item in significant(rendered)
                ):
                    continue
                if (
                    [item.text for item in significant(rendered)]
                    == [item.text for item in payload]
                ):
                    continue
                replacement = _with_preserved_comments(
                    source,
                    payload[0].start,
                    payload[-1].end,
                    rendered,
                )
                replacements.append(
                    (payload[0].start, payload[-1].end, replacement)
                )
    return replace_spans(source, replacements)


def migrate_canonical_trait_impl_headers(source: str) -> str:
    """Canonicalize nested type syntax in already-new trait/impl headers."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text not in {"trait", "impl"}:
            continue
        end = _header_boundary(tokens, index + 1)
        if end is None or tokens[end].text != "{":
            continue

        head_start = index + 1
        if (
            token.text == "impl"
            and head_start < end
            and tokens[head_start].text == "<"
        ):
            binder_close = matching_index(tokens, head_start)
            if binder_close is None or binder_close >= end:
                continue
            head_start = binder_close + 1
        if head_start >= end:
            continue

        header = list(tokens[head_start:end])
        where_relative = find_top(header, "where")
        head_end = (
            head_start + where_relative
            if where_relative is not None
            else end
        )
        head_tokens = list(tokens[head_start:head_end])
        rendered_head = render_trait_ref_text(head_tokens)
        if (
            rendered_head is not None
            and [item.text for item in significant(rendered_head)]
            != [item.text for item in head_tokens]
        ):
            replacement = _with_preserved_comments(
                source,
                head_tokens[0].start,
                head_tokens[-1].end,
                rendered_head,
            )
            replacements.append(
                (head_tokens[0].start, head_tokens[-1].end, replacement)
            )

        if where_relative is None:
            continue
        predicates = list(tokens[head_end + 1 : end])
        if not predicates:
            continue
        rendered_predicates = render_predicates(predicates)
        if not rendered_predicates:
            continue
        rendered_context = ", ".join(rendered_predicates)
        if (
            [item.text for item in significant(rendered_context)]
            == [item.text for item in predicates]
        ):
            continue
        replacement = _with_preserved_comments(
            source,
            predicates[0].start,
            predicates[-1].end,
            rendered_context,
        )
        replacements.append(
            (predicates[0].start, predicates[-1].end, replacement)
        )
    return replace_spans(source, replacements)


def migrate_source(
    source: str,
    global_constructor_owners: Mapping[str, str] | None = None,
    global_dot_constructor_candidates: Mapping[str, set[str]] | None = None,
) -> str:
    if has_comment_marker(source, KEEP_LEGACY_NEGATIVE_MARKER):
        return source
    reject_string_imports(source)
    reject_solidity_call_options(source)
    reject_contract_inheritance(source)
    reject_noncanonical_proxy_comptime(source)
    reject_malformed_mapping_types(source)
    reject_noncanonical_function_type_qualifiers(source)
    passes = (
        migrate_pragmas,
        migrate_imports,
        migrate_data_declarations,
        migrate_incomplete_data_heads,
        migrate_aliases,
        migrate_value_type_underlying_types,
        migrate_classes,
        migrate_instances,
        migrate_canonical_trait_impl_headers,
        migrate_functions,
        migrate_lambdas,
        migrate_special_functions,
        migrate_let_types,
        migrate_field_types,
        migrate_expression_annotations,
        migrate_conversion_types,
        migrate_proxy_types,
        migrate_enum_payload_types,
        migrate_incomplete_arrows,
        migrate_matches,
        remove_match_trailing_semicolons,
        migrate_if_expressions,
        migrate_condition_parentheses,
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
            reject_remaining_classic_arrows(source)
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
    """Decode a Rust ordinary string body to its semantic text."""

    simple_escapes = {
        "\\": "\\",
        '"': '"',
        "'": "'",
        "n": "\n",
        "r": "\r",
        "t": "\t",
        "0": "\0",
    }
    decoded: list[str] = []
    cursor = 0
    while cursor < len(body):
        if body[cursor] != "\\":
            decoded.append(body[cursor])
            cursor += 1
            continue
        if cursor + 1 >= len(body):
            return None

        escaped = body[cursor + 1]
        if escaped in simple_escapes:
            decoded.append(simple_escapes[escaped])
            cursor += 2
            continue
        if escaped == "x" and cursor + 3 < len(body):
            digits = body[cursor + 2 : cursor + 4]
            if all(digit in "0123456789abcdefABCDEF" for digit in digits):
                value = int(digits, 16)
                if value > 0x7F:
                    return None
                decoded.append(chr(value))
                cursor += 4
                continue
        if escaped == "u" and cursor + 2 < len(body) and body[cursor + 2] == "{":
            close = body.find("}", cursor + 3)
            digits = body[cursor + 3 : close] if close >= 0 else ""
            normalized = digits.replace("_", "")
            if (
                normalized
                and len(normalized) <= 6
                and all(
                    digit in "0123456789abcdefABCDEF"
                    for digit in normalized
                )
            ):
                value = int(normalized, 16)
                if value <= 0x10FFFF and not 0xD800 <= value <= 0xDFFF:
                    decoded.append(chr(value))
                    cursor = close + 1
                    continue
        if escaped == "\n" or (
            escaped == "\r"
            and cursor + 2 < len(body)
            and body[cursor + 2] == "\n"
        ):
            cursor += 2
            if escaped == "\r" and cursor < len(body) and body[cursor] == "\n":
                cursor += 1
            while cursor < len(body) and body[cursor].isspace():
                cursor += 1
            continue

        return None
    return "".join(decoded)


def _encode_rust_ordinary_body(body: str) -> str:
    """Encode semantic text as a valid Rust ordinary string body."""

    escapes = {
        "\\": "\\\\",
        '"': '\\"',
        "\n": "\\n",
        "\r": "\\r",
        "\t": "\\t",
        "\0": "\\0",
    }
    encoded: list[str] = []
    for char in body:
        if char in escapes:
            encoded.append(escapes[char])
        elif ord(char) < 0x20 or ord(char) == 0x7F:
            encoded.append(f"\\u{{{ord(char):x}}}")
        else:
            encoded.append(char)
    return "".join(encoded)


def _rust_literal_semantic_body(body: str, is_raw: bool) -> str | None:
    """Return one literal body's semantic text for every migration phase."""

    return body if is_raw else _decode_rust_ordinary_body(body)


_SOLCORE_PREFIX_MODIFIERS = {
    "public",
    "external",
    "internal",
    "private",
    "payable",
    "pure",
    "view",
    "comptime",
}
_SOLCORE_FUNCTION_TRAILER_MODIFIERS = _SOLCORE_PREFIX_MODIFIERS | {
    "memory",
    "storage",
    "calldata",
}
_SOLCORE_CONTAINER_KEYWORDS = {
    "contract",
    "interface",
    "library",
    "enum",
    "struct",
    "trait",
    "class",
    "impl",
    "instance",
}
_RUST_NAMED_TEMPLATE_HOLE_RE = re.compile(
    r"\{[a-z_$][A-Za-z0-9_$]*\}"
)


def _rust_source_text_is_trivia(source: str) -> bool:
    cursor = 0
    while cursor < len(source):
        if source[cursor].isspace():
            cursor += 1
            continue
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            if newline < 0:
                return True
            cursor = newline + 1
            continue
        if source.startswith("/*", cursor):
            depth = 1
            cursor += 2
            while cursor < len(source) and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            if depth:
                return False
            continue
        return False
    return True


def _rust_source_boundary(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    """Return whether a declaration begins at a source-like boundary."""

    start = index
    while (
        start > 0
        and tokens[start - 1].kind == "word"
        and tokens[start - 1].text in _SOLCORE_PREFIX_MODIFIERS
    ):
        start -= 1
    if start == 0:
        return True
    line_start = source.rfind("\n", 0, tokens[start].start) + 1
    if _rust_source_text_is_trivia(source[line_start : tokens[start].start]):
        return True
    return start > 0 and tokens[start - 1].text in {"{", "}", ";"}


def _rust_classic_prefix_start(
    tokens: Sequence[Token],
    index: int,
    *,
    allow_default: bool = False,
) -> int | None:
    """Return the start of a structured Classic declaration prefix."""

    start = _previous_boundary(tokens, index)
    prefix = list(tokens[start:index])
    if not prefix:
        return None
    if allow_default and prefix[0].text == "default":
        prefix = prefix[1:]
    prefix = [
        token
        for token in prefix
        if not (
            token.kind == "word"
            and token.text in _SOLCORE_PREFIX_MODIFIERS
        )
    ]
    if not prefix:
        return start
    if prefix[0].text == "forall":
        dot = find_top(prefix[1:], ".", angles=False)
        if dot is None:
            return None
        binder = prefix[1 : dot + 1]
        return start if any(token.kind == "word" for token in binder) else None
    if prefix[-1].text == "=>":
        has_name = any(token.kind == "word" for token in prefix[:-1])
        has_predicate_shape = any(
            token.text in {"(", ":", "<"} for token in prefix[:-1]
        )
        return start if has_name and has_predicate_shape else None
    return None


def _rust_source_line_starts_at(source: str, token: Token) -> bool:
    line_start = source.rfind("\n", 0, token.start) + 1
    return _rust_source_text_is_trivia(source[line_start:token.start])


def _rust_source_line_ends_after(source: str, token: Token) -> bool:
    line_end = source.find("\n", token.end)
    if line_end < 0:
        line_end = len(source)
    suffix = source[token.end:line_end].strip()
    return not suffix or suffix.startswith(("//", "/*"))


def _rust_format_escaped_block(
    source: str,
    tokens: Sequence[Token],
    open_index: int,
    close_index: int,
) -> bool:
    """Recognize Rust format-string `{{ ... }}` escapes."""

    if open_index + 1 >= close_index or tokens[open_index + 1].text != "{":
        return False
    inner_close = matching_index(tokens, open_index + 1)
    if inner_close != close_index - 1:
        return False
    return (
        source[tokens[open_index].start : tokens[open_index + 1].end] == "{{"
        and source[tokens[inner_close].start : tokens[close_index].end] == "}}"
    )


def _rust_executable_block_is_source_like(
    source: str,
    tokens: Sequence[Token],
    open_index: int,
    close_index: int,
) -> bool:
    if close_index == open_index + 1:
        return True
    if _rust_format_escaped_block(source, tokens, open_index, close_index):
        return False
    inner = tokens[open_index + 1 : close_index]
    statement_words = {
        "return",
        "let",
        "match",
        "if",
        "else",
        "for",
        "while",
        "unchecked",
        "revert",
        "break",
        "continue",
    }
    return (
        any(token.text == ";" for token in inner)
        or any(token.kind == "assembly" for token in inner)
        or any(token.text in statement_words for token in inner)
    )


def _rust_declaration_block_is_source_like(
    source: str,
    tokens: Sequence[Token],
    open_index: int,
    close_index: int,
) -> bool:
    if close_index == open_index + 1:
        return True
    if _rust_format_escaped_block(source, tokens, open_index, close_index):
        return False
    inner = tokens[open_index + 1 : close_index]
    declaration_words = {
        "function",
        "constructor",
        "fallback",
        "alias",
        "type",
        "enum",
        "struct",
        "let",
        "return",
    }
    return (
        any(token.text in {";", ","} for token in inner)
        or any(token.text == "{" for token in inner)
        or any(token.text in declaration_words for token in inner)
    )


def _rust_function_trailer_is_source_like(tokens: Sequence[Token]) -> bool:
    if not tokens:
        return True
    texts = [token.text for token in tokens]
    if "->" in texts:
        arrow = texts.index("->")
        return arrow + 1 < len(tokens)
    if "returns" in texts:
        returns = texts.index("returns")
        if returns + 1 >= len(tokens) or tokens[returns + 1].text != "(":
            return False
        close = matching_index(tokens, returns + 1)
        return close is not None
    if "where" in texts:
        return texts.index("where") + 1 < len(tokens)
    return all(
        token.kind == "word"
        and token.text in _SOLCORE_FUNCTION_TRAILER_MODIFIERS
        for token in tokens
    )


def _rust_function_fragment_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    prefixed_start = _rust_classic_prefix_start(tokens, index)
    source_boundary = _rust_source_boundary(source, tokens, index)
    if prefixed_start is not None:
        source_boundary = source_boundary or _rust_source_boundary(
            source, tokens, prefixed_start
        )
    if (
        not source_boundary
        or index + 2 >= len(tokens)
        or tokens[index + 1].kind != "word"
    ):
        return False
    cursor = index + 2
    if tokens[cursor].text == "<":
        generic_close = matching_index(tokens, cursor)
        if generic_close is None:
            return False
        cursor = generic_close + 1
    if cursor >= len(tokens) or tokens[cursor].text != "(":
        return False
    params_close = matching_index(tokens, cursor)
    if params_close is None:
        return False
    boundary = _header_boundary(tokens, params_close + 1)
    if boundary is None or not _rust_function_trailer_is_source_like(
        tokens[params_close + 1 : boundary]
    ):
        return False
    if tokens[boundary].text == ";":
        return _rust_source_line_ends_after(source, tokens[boundary])
    if tokens[boundary].text != "{":
        return False
    body_close = matching_index(tokens, boundary)
    return body_close is not None and _rust_executable_block_is_source_like(
        source, tokens, boundary, body_close
    )


def _rust_assignment_declaration_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    if (
        not _rust_source_boundary(source, tokens, index)
        or index + 1 >= len(tokens)
        or tokens[index + 1].kind != "word"
    ):
        return False
    end = _statement_end(tokens, index)
    if end is None or not _rust_source_line_ends_after(source, tokens[end]):
        return False
    equals = find_top(tokens[index + 2 : end], "=", angles=False)
    return equals is not None and index + 2 + equals + 1 < end


def _rust_comptime_let_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    if not _rust_source_boundary(source, tokens, index):
        return False
    end = _statement_end(tokens, index)
    if end is None or not _rust_source_line_ends_after(source, tokens[end]):
        return False
    declaration = tokens[index + 1 : end]
    colon = find_top(declaration, ":", angles=False)
    if colon is None or colon == 0 or colon + 1 >= len(declaration):
        return False
    canonical = declaration[0].text == "comptime"
    classic = declaration[colon + 1].text == "comptime"
    if not (canonical or classic):
        return False
    binding = declaration[1:colon] if canonical else declaration[:colon]
    return bool(binding) and (
        binding[0].kind == "word" or binding[0].text == "("
    )


def _rust_match_fragment_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    previous = tokens[index - 1].text if index else ""
    if not (
        index == 0
        or _rust_source_line_starts_at(source, tokens[index])
        or previous in {"return", "=", ":=", "(", "[", ",", "=>"}
    ):
        return False
    body_open = _header_boundary(tokens, index + 1)
    if body_open is None or tokens[body_open].text != "{":
        return False
    body_close = matching_index(tokens, body_open)
    if body_close is None or _rust_format_escaped_block(
        source, tokens, body_open, body_close
    ):
        return False
    body = tokens[body_open + 1 : body_close]
    if any(token.text == "=>" for token in body):
        return True
    has_case = any(token.text in {"case", "default"} for token in body)
    return has_case and any(token.text == "{" for token in body)


def _rust_container_fragment_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    previous = tokens[index - 1].text if index else ""
    source_boundary = (
        index == 0
        or _rust_source_line_starts_at(source, tokens[index])
        or previous in {"{", "}", ";"}
    )
    if tokens[index].text in {"class", "instance"}:
        prefixed_start = _rust_classic_prefix_start(
            tokens,
            index,
            allow_default=tokens[index].text == "instance",
        )
        if prefixed_start is not None:
            source_boundary = source_boundary or _rust_source_boundary(
                source, tokens, prefixed_start
            )
    if not source_boundary:
        return False
    body_open = _header_boundary(tokens, index + 1)
    if body_open is None or tokens[body_open].text != "{":
        return False
    header = tokens[index + 1 : body_open]
    if not header or not any(token.kind == "word" for token in header):
        return False
    if (
        tokens[index].text
        in {"contract", "interface", "library", "enum", "struct", "trait", "class"}
        and header[0].kind != "word"
    ):
        return False
    body_close = matching_index(tokens, body_open)
    if body_close is None:
        return False
    if tokens[index].text == "enum":
        return not _rust_format_escaped_block(
            source, tokens, body_open, body_close
        )
    return _rust_declaration_block_is_source_like(
        source, tokens, body_open, body_close
    )


def _rust_module_path_is_source_like(tokens: Sequence[Token]) -> bool:
    if not tokens:
        return False
    cursor = 0
    if tokens[cursor].text == "@":
        cursor += 1
    if cursor >= len(tokens) or tokens[cursor].kind != "word":
        return False
    cursor += 1
    while cursor < len(tokens):
        if (
            tokens[cursor].text != "."
            or cursor + 1 >= len(tokens)
            or tokens[cursor + 1].kind != "word"
        ):
            return False
        cursor += 2
    return True


def _rust_import_path_is_source_like(tokens: Sequence[Token]) -> bool:
    return (
        len(tokens) == 1 and tokens[0].kind == "string"
    ) or _rust_module_path_is_source_like(tokens)


def _rust_import_selectors_are_source_like(
    tokens: Sequence[Token],
) -> bool:
    parts = split_top(tokens, ",", angles=False)
    if not parts or any(not part for part in parts):
        return False
    for part in parts:
        if len(part) == 1 and (
            part[0].kind == "word" or part[0].text == "*"
        ):
            continue
        if (
            len(part) == 3
            and part[0].kind == "word"
            and part[1].text == "as"
            and part[2].kind == "word"
        ):
            continue
        return False
    return True


def _rust_import_body_is_source_like(tokens: Sequence[Token]) -> bool:
    if not tokens:
        return False

    if len(tokens) == 1 and tokens[0].kind == "string":
        return True

    if tokens[0].text == "*":
        return (
            len(tokens) >= 5
            and tokens[1].text == "as"
            and tokens[2].kind == "word"
            and tokens[3].text == "from"
            and _rust_import_path_is_source_like(tokens[4:])
        )

    if tokens[0].text == "{":
        close = matching_index(tokens, 0)
        return (
            close is not None
            and close + 2 < len(tokens)
            and tokens[close + 1].text == "from"
            and _rust_import_selectors_are_source_like(tokens[1:close])
            and _rust_import_path_is_source_like(tokens[close + 2 :])
        )

    brace = find_top(tokens, "{", angles=False)
    if brace is not None:
        if brace == 0 or tokens[brace - 1].text != ".":
            return False
        close = matching_index(tokens, brace)
        if (
            close is None
            or not _rust_module_path_is_source_like(tokens[: brace - 1])
            or not _rust_import_selectors_are_source_like(
                tokens[brace + 1 : close]
            )
        ):
            return False
        tail = tokens[close + 1 :]
        if not tail:
            return True
        if len(tail) < 3 or tail[0].text != "hiding":
            return False
        if tail[1].text != "{" or matching_index(tail, 1) != len(tail) - 1:
            return False
        return _rust_import_selectors_are_source_like(tail[2:-1])

    as_index = find_top(tokens, "as", angles=False)
    if as_index is not None:
        return (
            as_index + 2 == len(tokens)
            and tokens[as_index + 1].kind == "word"
            and _rust_import_path_is_source_like(tokens[:as_index])
        )
    return _rust_module_path_is_source_like(tokens)


def _rust_import_fragment_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    if not _rust_source_boundary(source, tokens, index):
        return False
    end = _statement_end(tokens, index)
    return (
        end is not None
        and _rust_source_line_ends_after(source, tokens[end])
        and _rust_import_body_is_source_like(tokens[index + 1 : end])
    )


def _rust_pragma_fragment_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    if not _rust_source_boundary(source, tokens, index):
        return False
    end = _statement_end(tokens, index)
    if (
        end is None
        or not _rust_source_line_ends_after(source, tokens[end])
        or index + 1 >= end
    ):
        return False
    body = tokens[index + 1 : end]
    if body[0].text in {"solidity", "abicoder", "solcore"}:
        return len(body) > 1
    body_texts = [token.text for token in body]
    for legacy_name in PRAGMA_NAMES:
        expected: list[str] = []
        for piece_index, piece in enumerate(legacy_name.split("-")):
            if piece_index:
                expected.append("-")
            expected.append(piece)
        if body_texts[: len(expected)] == expected:
            return True
    return False


def _looks_like_solcore_literal(source: str) -> bool:
    """Classify an embedded literal from combined Solcore syntax signals."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if token.text == "import" and _rust_import_fragment_is_source_like(
            source, tokens, index
        ):
            return True

    # Dynamically completed Rust test templates are not standalone Solcore
    # sources yet. Rewriting around a `{name}` hole can mistake a canonical
    # `name: Type` declaration for the removed `expression : Type` spelling.
    # Recognize structurally complete imports first because their selector
    # braces are ordinary source syntax, not Rust format placeholders.
    if _RUST_NAMED_TEMPLATE_HOLE_RE.search(source) is not None:
        return False

    for index, token in enumerate(tokens):
        if token.text == "pragma" and _rust_pragma_fragment_is_source_like(
            source, tokens, index
        ):
            return True
        if token.text == "function" and _rust_function_fragment_is_source_like(
            source, tokens, index
        ):
            return True
        if token.text in {"data", "type", "alias"} and (
            _rust_assignment_declaration_is_source_like(source, tokens, index)
        ):
            return True
        if token.text == "let" and _rust_comptime_let_is_source_like(
            source, tokens, index
        ):
            return True
        if token.text == "match" and _rust_match_fragment_is_source_like(
            source, tokens, index
        ):
            return True
        if token.text in _SOLCORE_CONTAINER_KEYWORDS and (
            _rust_container_fragment_is_source_like(source, tokens, index)
        ):
            return True
    return False


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
        semantic_body = _rust_literal_semantic_body(body, is_raw)
        if semantic_body is not None and _looks_like_solcore_literal(
            semantic_body
        ):
            literals.append((body_start, body_end, is_raw))
        cursor = literal_end
    return literals


def _rust_solcore_literal_spans(source: str) -> list[tuple[int, int]]:
    return [
        (body_start, body_end)
        for body_start, body_end, _ in _rust_solcore_literals(source)
    ]


def has_rust_comment_marker(source: str, marker: str) -> bool:
    """Find a marker in Rust comments without inspecting literal bodies."""

    cursor = 0
    while cursor < len(source):
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            end = len(source) if newline < 0 else newline
            if marker in source[cursor:end]:
                return True
            cursor = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", cursor):
            end = _rust_block_comment_end(source, cursor)
            if marker in source[cursor:end]:
                return True
            cursor = end
            continue
        if source[cursor] == "'":
            char_end = _rust_char_end(source, cursor)
            if char_end is not None:
                cursor = char_end
                continue

        literal = _rust_raw_literal(source, cursor)
        if literal is None:
            literal = _rust_ordinary_literal(source, cursor)
        if literal is not None:
            cursor = literal[2]
            continue
        cursor += 1
    return False


def migrate_rust_strings(
    source: str,
    global_constructor_owners: Mapping[str, str] | None = None,
    global_dot_constructor_candidates: Mapping[str, set[str]] | None = None,
    *,
    classic_bare_imports: bool = False,
) -> str:
    """Migrate Solcore programs embedded in Rust string literals.

    Literals are rewritten only when combined token, boundary, and declaration
    signals make them source-like, leaving unrelated regex, format templates,
    and prose byte-for-byte unchanged.  Ordinary strings are transformed in
    their escaped spelling so the surrounding Rust source and existing escapes
    stay intact.
    """

    if has_rust_comment_marker(source, KEEP_RUST_FILE_MARKER):
        return source

    replacements: list[tuple[int, int, str]] = []
    for body_start, body_end, is_raw in _rust_solcore_literals(source):
        encoded_body = source[body_start:body_end]
        body = _rust_literal_semantic_body(encoded_body, is_raw)
        if body is None:
            continue
        if classic_bare_imports:
            body = migrate_classic_bare_imports(body)
        migrated = migrate_source(
            body,
            global_constructor_owners,
            global_dot_constructor_candidates,
        )
        if migrated == body:
            continue
        if not is_raw:
            migrated = _encode_rust_ordinary_body(migrated)
        if migrated != encoded_body:
            replacements.append((body_start, body_end, migrated))

    return replace_spans(source, replacements)


def _stage_atomic_bytes(
    path: Path,
    source: bytes,
    mode: int,
    *,
    label: str,
) -> Path:
    file_descriptor, staged_name = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.migrate-{label}-",
    )
    staged = Path(staged_name)
    try:
        with os.fdopen(file_descriptor, "wb") as stream:
            written = stream.write(source)
            if written != len(source):
                raise OSError(
                    f"short recovery write: wrote {written} of "
                    f"{len(source)} bytes"
                )
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(staged, mode)
    except BaseException:
        try:
            staged.unlink()
        except OSError as cleanup_error:
            raise OSError(
                f"failed to clean staged migration file {staged}: "
                f"{cleanup_error}"
            ) from cleanup_error
        raise
    return staged


def _write_binary_stream(stream: BinaryIO, source: bytes) -> None:
    stream.seek(0)
    written = stream.write(source)
    if written != len(source):
        raise OSError(
            f"short migration write: wrote {written} of {len(source)} bytes"
        )
    stream.truncate()
    stream.flush()
    os.fsync(stream.fileno())


def _default_text_encoding() -> str:
    """Match `Path.read_text()` including Python's UTF-8 mode."""

    return locale.getpreferredencoding(False)


def write_migrations_atomically(
    expected_originals: Mapping[Path, bytes],
    migrations: Mapping[Path, str],
) -> None:
    """Open a complete batch before writing and roll back every failed write.

    Existing inodes are updated in place so ownership, ACLs, extended
    attributes, hard links, and symbolic-link targets retain their previous
    behavior.  Exact original bytes stay in memory for rollback.  If restoring
    an open file fails, a recovery file is preserved and named in the error.
    Callers must exclude concurrent, non-cooperating in-place writers; byte
    and identity checks detect ordinary races but cannot make compare/write a
    single portable filesystem operation.
    """

    encoding = _default_text_encoding()
    encoded: dict[Path, bytes] = {}
    try:
        encoded = {
            path: migrated.encode(encoding)
            for path, migrated in migrations.items()
        }
    except UnicodeEncodeError as error:
        raise OSError(
            f"cannot encode migrated source as {encoding}: {error}"
        ) from error

    originals: dict[Path, bytes] = {}
    modes: dict[Path, int] = {}
    identities: dict[tuple[int, int], Path] = {}
    identity_by_path: dict[Path, tuple[int, int]] = {}
    desired_by_identity: dict[tuple[int, int], bytes] = {}
    write_paths: list[Path] = []
    try:
        missing = set(migrations) - set(expected_originals)
        if missing:
            rendered = ", ".join(str(path) for path in sorted(missing))
            raise OSError(
                f"missing expected original bytes for {rendered}"
            )
        for path, expected in expected_originals.items():
            access = "r+b" if path in migrations else "rb"
            with path.open(access) as stream:
                metadata = os.fstat(stream.fileno())
                identity = (metadata.st_dev, metadata.st_ino)
                current = stream.read()
            if current != expected_originals[path]:
                raise OSError(
                    f"source changed after migration planning: {path}"
                )
            desired = encoded.get(path, current)
            identity_by_path[path] = identity
            if identity in identities:
                primary = identities[identity]
                if desired != desired_by_identity[identity]:
                    raise OSError(
                        f"selected paths {primary} and {path} refer to the "
                        "same file but require different migrated contents"
                    )
                continue
            identities[identity] = path
            desired_by_identity[identity] = desired
            originals[path] = current
            modes[path] = metadata.st_mode & 0o777
            if desired != current:
                write_paths.append(path)
    except Exception as error:
        raise OSError(
            f"atomic migration preparation failed: {error}"
        ) from error

    applied: list[Path] = []
    completed: set[Path] = set()
    try:
        for path in write_paths:
            with path.open("r+b") as stream:
                metadata = os.fstat(stream.fileno())
                identity = (metadata.st_dev, metadata.st_ino)
                current = stream.read()
                if (
                    identity != identity_by_path[path]
                    or current != originals[path]
                ):
                    raise OSError(
                        f"source changed during migration: {path}"
                    )
                # Record the current path before writing so even a partial or
                # short write is restored.
                applied.append(path)
                _write_binary_stream(
                    stream,
                    desired_by_identity[identity],
                )
                completed.add(path)
        for path in expected_originals:
            with path.open("rb") as stream:
                metadata = os.fstat(stream.fileno())
                identity = (metadata.st_dev, metadata.st_ino)
                current = stream.read()
            if (
                identity != identity_by_path[path]
                or current != desired_by_identity[
                    identity_by_path[path]
                ]
            ):
                raise OSError(
                    f"source changed while committing migration: {path}"
                )
    except BaseException as error:
        rollback_errors: list[str] = []
        for path in reversed(applied):
            try:
                with path.open("r+b") as stream:
                    metadata = os.fstat(stream.fileno())
                    identity = (metadata.st_dev, metadata.st_ino)
                    if identity != identity_by_path[path]:
                        raise OSError(
                            "path identity changed before rollback"
                        )
                    current = stream.read()
                    if (
                        path in completed
                        and current
                        != desired_by_identity[identity_by_path[path]]
                    ):
                        raise OSError(
                            "file content changed before rollback"
                        )
                    _write_binary_stream(stream, originals[path])
            except BaseException as rollback_error:
                recovery_path = None
                recovery_error = None
                try:
                    recovery_path = _stage_atomic_bytes(
                        path,
                        originals[path],
                        modes[path],
                        label="recovery",
                    )
                except BaseException as staging_error:
                    recovery_error = staging_error
                recovery = (
                    f"; original preserved at {recovery_path}"
                    if recovery_path is not None
                    else (
                        f"; could not stage recovery: {recovery_error}"
                    )
                )
                rollback_errors.append(
                    f"{path}: {rollback_error}{recovery}"
                )
        detail = (
            "; rollback failed for " + "; ".join(rollback_errors)
            if rollback_errors
            else ""
        )
        if (
            not rollback_errors
            and isinstance(error, (KeyboardInterrupt, SystemExit))
        ):
            raise
        raise OSError(
            f"atomic migration write failed: {error}{detail}"
        ) from error


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
    original_bytes: dict[Path, bytes] = {}
    for path in paths:
        try:
            source_bytes = path.read_bytes()
            original_bytes[path] = source_bytes
            originals[path] = (
                source_bytes.decode(_default_text_encoding())
                .replace("\r\n", "\n")
                .replace("\r", "\n")
            )
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
                source = _rust_literal_semantic_body(
                    encoded_source,
                    is_raw,
                )
                if source is None:
                    continue
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
    migrations: dict[Path, str] = {}
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
        migrations[path] = migrated

    if not args.check and not failures and migrations:
        try:
            write_migrations_atomically(
                original_bytes,
                migrations,
            )
        except Exception as error:
            failures.append((Path("<migration batch>"), error))

    action = "need migration" if args.check else "migrated"
    reported_changed = (
        changed if args.check or not failures else []
    )
    print(
        f"{len(reported_changed)} file(s) {action}; "
        f"{len(paths)} file(s) examined"
    )
    for path in reported_changed:
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
