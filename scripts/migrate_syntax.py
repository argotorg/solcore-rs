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

This tool does not translate Classic Solidity.  In historical Solcore,
``T(expression)`` was always a call or constructor expression, never a type
conversion, so the migrator never reinterprets that spelling as a conversion:
ordinary calls remain calls, while proven constructors are qualified where the
new syntax requires it.  A Solidity importer must resolve conversion targets
semantically before rewriting them to Core's ``expression as T`` syntax.

Pass ``--classic-bare-imports`` when the input still uses Classic Solcore's
``import M;`` namespace semantics.  That spelling becomes
``import * as M from M;`` before the remaining syntax migration; without the
flag, canonical Core ``import M;`` keeps its open-import meaning.

Classic prefix-dot enum constructors are qualified from a source-local
declaration or a constructor explicitly exposed through the consumer's
resolved import surface (``.Some(...)`` becomes ``Option.Some(...)``).
Providers are matched only through unambiguous ordinary relative paths among
the selected source files; standard, external, multi-root, unresolved, and
ambiguous imports fail closed instead of borrowing an unrelated declaration.

Batch writes are failure-atomic for migration errors, I/O failures, and
interrupts.  Selected files must not be modified concurrently by another
process: the command revalidates bytes and file identities before and after
the commit, but no portable filesystem primitive can combine an in-place
metadata-preserving write with exclusion of uncooperative writers.

Bare same-name constructors are also qualified when their source-local or
imported origin is unambiguous (``enum Point { Point(...) }`` makes term and
pattern uses become ``Point.Point(...)``). Imported functions retain term
precedence in expressions, while constructor patterns remain structural.
Embedded Rust literals are isolated sources and never seed one another's
constructor surface. A source that deliberately tests a rejected unqualified
spelling can opt out of these passes with this comment:

    // migrate-syntax: keep-unqualified-constructor

Likewise, a negative fixture that deliberately exercises any rejected Classic
surface can opt out of the complete rewrite with:

    // migrate-syntax: keep-legacy-negative

Direct Unicode string operands of a bare Rust ``concat!`` are treated as one
isolated source. A ``concat!`` that contains non-Solcore output which happens
to resemble Classic syntax can opt out with a comment inside its token tree:

    // migrate-syntax: keep-rust-concat
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import locale
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import unicodedata
from typing import BinaryIO, Iterable, Mapping, Sequence


TRIVIA = {"ws", "comment"}
CORE_LEXER_WORD_TOKENS = frozenset(
    {
        "contract",
        "interface",
        "library",
        "import",
        "from",
        "export",
        "as",
        "let",
        "comptime",
        "enum",
        "struct",
        "trait",
        "impl",
        "alias",
        "is",
        "where",
        "returns",
        "if",
        "else",
        "for",
        "while",
        "unchecked",
        "switch",
        "type",
        "case",
        "default",
        "match",
        "public",
        "external",
        "internal",
        "private",
        "pure",
        "view",
        "payable",
        "function",
        "constructor",
        "fallback",
        "return",
        "revert",
        "leave",
        "continue",
        "break",
        "lam",
        "assembly",
        "pragma",
        "true",
        "false",
    }
)
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
KEEP_RUST_CONCAT_MARKER = "migrate-syntax: keep-rust-concat"
RUST_PATTERN_WHITESPACE = frozenset(
    " \t\n\v\f\r\u0085\u200e\u200f\u2028\u2029"
)
BUILTIN_CONSTRUCTORS = {"true", "false", "pair", "inl", "inr"}


@dataclass(frozen=True)
class Token:
    kind: str
    text: str
    start: int
    end: int


def _is_identifier_letter(character: str) -> bool:
    return unicodedata.category(character).startswith("L")


def _is_identifier_number(character: str) -> bool:
    return unicodedata.category(character).startswith("N")


def _is_core_identifier_text(text: str) -> bool:
    """Match the current parser's non-hyphenated identifier grammar."""

    return (
        bool(text)
        and _is_identifier_letter(text[0])
        and all(
            character == "_"
            or _is_identifier_letter(character)
            or _is_identifier_number(character)
            for character in text[1:]
        )
    )


def _is_legacy_identifier_text(text: str) -> bool:
    """Match Unicode identifiers plus the legacy Solidity ``_``/``$`` forms."""

    return (
        bool(text)
        and (
            text[0] in {"_", "$"}
            or _is_identifier_letter(text[0])
        )
        and all(
            character in {"_", "$"}
            or _is_identifier_letter(character)
            or _is_identifier_number(character)
            for character in text[1:]
        )
    )


def _legacy_identifier_end(source: str, start: int) -> int | None:
    if (
        start >= len(source)
        or not _is_legacy_identifier_text(source[start])
    ):
        return None
    cursor = start + 1
    while cursor < len(source):
        character = source[cursor]
        if not (
            character in {"_", "$"}
            or _is_identifier_letter(character)
            or _is_identifier_number(character)
        ):
            break
        cursor += 1
    return cursor


def _is_core_import_identifier(token: Token) -> bool:
    """Mirror ``ident_parser`` without accepting its diagnosed recoveries."""

    return (
        token.kind == "word"
        and _is_core_identifier_text(token.text)
        and (
            token.text == "from"
            or token.text not in CORE_LEXER_WORD_TOKENS
        )
    )


@dataclass(frozen=True)
class FunctionTypeSuffix:
    visibility_index: int | None
    mutability_index: int | None
    returns_open: int | None
    returns_close: int | None
    end: int


@dataclass(frozen=True, order=True)
class ConstructorOrigin:
    """Stable identity for one declaration occupying a type namespace."""

    provider: str
    type_name: str
    declaration_start: int


@dataclass(frozen=True, order=True)
class ConstructorBinding:
    """One constructor origin and the type qualifier visible to a consumer."""

    origin: ConstructorOrigin
    owner: str


@dataclass(frozen=True, order=True)
class ConstructorOwnerClaim:
    """One source surface claiming a visible constructor type namespace."""

    visible_through: str
    origin: ConstructorOrigin
    local: bool = False


def _constructor_binding_preference(
    binding: ConstructorBinding,
) -> tuple[int, bool, bool, int, str]:
    """Choose a stable visible owner for aliases of one declaration."""

    return (
        binding.owner.count("."),
        binding.owner != binding.origin.type_name,
        binding.owner.rsplit(".", 1)[-1]
        != binding.origin.type_name,
        len(binding.owner),
        binding.owner,
    )


def _single_origin_constructor_binding(
    bindings: Iterable[ConstructorBinding],
) -> ConstructorBinding | None:
    candidates = tuple(bindings)
    if (
        not candidates
        or len({binding.origin for binding in candidates}) != 1
    ):
        return None
    return min(candidates, key=_constructor_binding_preference)


@dataclass(frozen=True)
class ConstructorImportSurface:
    """Constructor and term facts proven from one source file's imports."""

    bare_candidates: Mapping[str, frozenset[ConstructorBinding]]
    dot_candidates: Mapping[str, frozenset[ConstructorBinding]]
    owner_claims: Mapping[str, frozenset[ConstructorOwnerClaim]]
    namespace_qualifier_targets: Mapping[str, frozenset[str]]
    qualified_namespace_term_targets: Mapping[str, frozenset[str]]
    qualified_import_term_winners: Mapping[str, str]
    imported_terms: frozenset[str]
    unknown_imported_terms: frozenset[str]
    has_unknown_unqualified_terms: bool
    has_unknown_unqualified_constructors: bool
    has_unknown_constructors: bool


EMPTY_CONSTRUCTOR_IMPORT_SURFACE = ConstructorImportSurface(
    bare_candidates={},
    dot_candidates={},
    owner_claims={},
    namespace_qualifier_targets={},
    qualified_namespace_term_targets={},
    qualified_import_term_winners={},
    imported_terms=frozenset(),
    unknown_imported_terms=frozenset(),
    has_unknown_unqualified_terms=False,
    has_unknown_unqualified_constructors=False,
    has_unknown_constructors=False,
)


def _constructor_owner_conflict_targets(
    surface: ConstructorImportSurface,
    owner: str,
) -> tuple[str, ...]:
    """Return import targets which make ``owner`` an ambiguous namespace."""

    root = owner.split(".", 1)[0]
    claims = surface.owner_claims.get(owner, frozenset())
    qualifier_targets = surface.namespace_qualifier_targets.get(
        root,
        frozenset(),
    )
    imported_claims = {
        claim for claim in claims if not claim.local
    }
    local_claims = {
        claim for claim in claims if claim.local
    }
    imported_claim_families = {
        (
            claim.visible_through,
            claim.origin.provider,
            claim.origin.type_name,
        )
        for claim in imported_claims
    }
    conflicted = (
        len(imported_claim_families) > 1
        or bool(imported_claims and local_claims)
        or ("." in owner and len(qualifier_targets) > 1)
    )
    if not conflicted:
        return ()
    return tuple(
        sorted(
            {
                claim.visible_through
                for claim in claims
            }
            | (
                set(qualifier_targets)
                if "." in owner
                else set()
            )
        )
    )


def _constructor_qualification_conflict_targets(
    surface: ConstructorImportSurface,
    owner: str,
    leaf: str,
) -> tuple[str, ...]:
    """Return sources which make the emitted ``owner.leaf`` ambiguous."""

    return tuple(
        sorted(
            set(_constructor_owner_conflict_targets(surface, owner))
            | (
                set(
                    surface.qualified_namespace_term_targets.get(
                        f"{owner}.{leaf}",
                        frozenset(),
                    )
                )
                if surface.qualified_import_term_winners.get(
                    f"{owner}.{leaf}"
                )
                != "constructor"
                else set()
            )
        )
    )


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
    opaque_pragma_active = False
    pending_pragma = False

    def append(token: Token) -> None:
        nonlocal opaque_pragma_active
        nonlocal pending_pragma
        tokens.append(token)
        if token.kind in TRIVIA:
            return
        if token.text == ";":
            opaque_pragma_active = False
            pending_pragma = False
            return
        if opaque_pragma_active:
            return
        if pending_pragma:
            opaque_pragma_active = token.text in {
                "solidity",
                "abicoder",
            }
            pending_pragma = False
            if opaque_pragma_active:
                return
        pending_pragma = token.text == "pragma"

    i = 0
    while i < len(source):
        start = i
        if source[i].isspace():
            i += 1
            while i < len(source) and source[i].isspace():
                i += 1
            append(Token("ws", source[start:i], start, i))
            continue
        if source.startswith("//", i):
            end = source.find("\n", i + 2)
            i = len(source) if end < 0 else end
            append(Token("comment", source[start:i], start, i))
            continue
        if source.startswith("/*", i):
            i = _scan_block_comment(source, i)
            append(Token("comment", source[start:i], start, i))
            continue
        if source[i] in {'"', "'"}:
            i = _scan_quoted(source, i, source[i])
            append(Token("string", source[start:i], start, i))
            continue
        word_end = _legacy_identifier_end(source, i)
        if word_end is not None:
            i = word_end
            text = source[start:i]
            if text == "assembly":
                assembly_end = (
                    None
                    if opaque_pragma_active
                    else _scan_assembly(source, start, i)
                )
                if assembly_end is not None:
                    i = assembly_end
                    append(Token("assembly", source[start:i], start, i))
                    continue
            append(Token("word", text, start, i))
            continue
        number = NUMBER_RE.match(source, i)
        if number:
            i = number.end()
            append(Token("number", number.group(0), start, i))
            continue
        symbol = next(
            (candidate for candidate in MULTI_SYMBOLS if source.startswith(candidate, i)),
            None,
        )
        if symbol is not None:
            i += len(symbol)
            append(Token("symbol", symbol, start, i))
            continue
        i += 1
        append(Token("symbol", source[start:i], start, i))
    return tokens


def significant(source: str) -> list[Token]:
    tokens = [
        token for token in lex(source) if token.kind not in TRIVIA
    ]
    result: list[Token] = []
    cursor = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if (
            token.text != "pragma"
            or cursor + 1 >= len(tokens)
            or tokens[cursor + 1].text
            not in {"solidity", "abicoder"}
        ):
            result.append(token)
            cursor += 1
            continue

        family = tokens[cursor + 1]
        result.extend((token, family))
        end = next(
            (
                index
                for index in range(cursor + 2, len(tokens))
                if tokens[index].text == ";"
            ),
            None,
        )
        payload_end = (
            tokens[end].start if end is not None else len(source)
        )
        if family.end < payload_end:
            result.append(
                Token(
                    "pragma_payload",
                    source[family.end:payload_end],
                    family.end,
                    payload_end,
                )
            )
        if end is None:
            break
        result.append(tokens[end])
        cursor = end + 1
    return result


def _has_core_lex_errors(source: str) -> bool:
    """Recognize lexical errors that Core reports before parsing."""

    symbols = frozenset("+-*/%!~<>=|&^@?.:;,(){}[]_")
    whitespace = frozenset(" \t\n\r\f")
    cursor = 0
    while cursor < len(source):
        character = source[cursor]
        if character in whitespace:
            cursor += 1
            continue
        if source.startswith("//", cursor):
            end = source.find("\n", cursor + 2)
            cursor = len(source) if end < 0 else end + 1
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
                return True
            continue
        if character == '"':
            cursor += 1
            while cursor < len(source):
                if source[cursor] == '"':
                    cursor += 1
                    break
                if source[cursor] != "\\":
                    cursor += 1
                    continue
                if (
                    cursor + 1 >= len(source)
                    or source[cursor + 1] not in {'n', 't', '"', "\\"}
                ):
                    return True
                cursor += 2
            else:
                return True
            continue
        if _is_identifier_letter(character):
            cursor += 1
            while (
                cursor < len(source)
                and (
                    source[cursor] == "_"
                    or _is_identifier_letter(source[cursor])
                    or _is_identifier_number(source[cursor])
                )
            ):
                cursor += 1
            continue
        if "0" <= character <= "9":
            cursor += 1
            while (
                cursor < len(source)
                and "0" <= source[cursor] <= "9"
            ):
                cursor += 1
            continue
        if character in symbols:
            cursor += 1
            continue
        return True
    return False


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


def _provider_statement_end(
    tokens: Sequence[Token],
    start: int,
) -> int | None:
    """Honor the parser's delimiter-opaque Solidity pragma payload."""

    if (
        tokens[start].text == "pragma"
        and start + 1 < len(tokens)
        and tokens[start + 1].text in {"solidity", "abicoder"}
    ):
        return next(
            (
                index
                for index in range(start + 2, len(tokens))
                if tokens[index].text == ";"
            ),
            None,
        )
    return _statement_end(tokens, start)


def reject_string_imports(source: str) -> None:
    """Reject Solidity path strings, which have no canonical Core spelling."""

    tokens = significant(source)
    for index, _ in _provider_top_level_item_regions(tokens):
        token = tokens[index]
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


def reject_operator_import_selectors(source: str) -> None:
    """Reject Classic parenthesized operator selectors without guessing a name."""

    tokens = significant(source)
    for index, _ in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if token.text != "import":
            continue
        end = _statement_end(tokens, index)
        if end is None:
            continue
        operator = None
        for cursor in range(index + 1, end):
            if (
                tokens[cursor].text != "("
                or tokens[cursor - 1].text not in {"{", ","}
            ):
                continue
            close = matching_index(tokens, cursor)
            if (
                close is not None
                and close < end
                and close > cursor + 1
                and all(
                    item.kind == "symbol"
                    for item in tokens[cursor + 1 : close]
                )
            ):
                operator = tokens[cursor]
                break
        if operator is None:
            continue
        line, column = _source_line_column(source, operator.start)
        raise ValueError(
            "cannot migrate operator import selector at "
            f"line {line}, column {column}: Core selective imports require "
            "identifier names; rename the operator and import that identifier"
        )


def migrate_pragmas(source: str) -> str:
    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, _ in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if token.text != "pragma":
            continue
        end = _provider_statement_end(tokens, index)
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
    for index, _ in _provider_top_level_item_regions(tokens):
        token = tokens[index]
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
            selected: list[tuple[str, str]] = []
            has_wildcard = False
            for part in selected_parts:
                if not part:
                    continue
                if len(part) == 1 and part[0].text == "*":
                    has_wildcard = True
                    continue
                if len(part) == 1 and part[0].kind == "word":
                    selected.append((part[0].text, part[0].text))
                    continue
                if (
                    len(part) == 3
                    and part[0].kind == "word"
                    and part[1].text == "as"
                    and part[2].kind == "word"
                ):
                    selected.append(
                        (
                            f"{part[0].text} as {part[2].text}",
                            part[2].text,
                        )
                    )
                    continue
                spelling = join_tokens(part)
                line, column = _source_line_column(source, part[0].start)
                raise ValueError(
                    "cannot migrate non-identifier import selector "
                    f"`{spelling}` at line {line}, column {column}: "
                    "Classic and Core selective imports require an "
                    "identifier with an optional identifier alias"
                )
            hidden: set[str] = set()
            if hiding is not None and hiding + 1 < len(body) and body[hiding + 1].text == "{":
                hidden_close = matching_index(body, hiding + 1)
                if hidden_close is not None:
                    for part in split_top(
                        body[hiding + 2 : hidden_close],
                        ",",
                        angles=False,
                    ):
                        if not part:
                            continue
                        if len(part) != 1 or part[0].kind != "word":
                            spelling = join_tokens(part)
                            line, column = _source_line_column(
                                source, part[0].start
                            )
                            raise ValueError(
                                "cannot migrate non-identifier hidden import "
                                f"name `{spelling}` at line {line}, "
                                f"column {column}: Classic import hiding "
                                "requires identifier names"
                            )
                        hidden.add(part[0].text)
            names = [
                spelling
                for spelling, local_name in selected
                if local_name not in hidden
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
            elif selected:
                line, column = _source_line_column(
                    source, body[brace].start
                )
                raise ValueError(
                    "cannot migrate empty selective import at "
                    f"line {line}, column {column}: `hiding` removes every "
                    "selected local name and Core has no empty import surface"
                )
            else:
                continue
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
    tokens: Sequence[Token],
    namespace_roots: set[str],
    constructor_names: set[str] | None = None,
) -> set[str]:
    """Collect binders without mistaking qualified constructor paths for them."""

    result: set[str] = set()
    constructor_names = constructor_names or set()
    comptime_expression_tokens = _comptime_pattern_expression_tokens(tokens)
    for index, token in enumerate(tokens):
        if (
            index in comptime_expression_tokens
            or token.kind != "word"
            or token.text not in namespace_roots
        ):
            continue
        previous = tokens[index - 1].text if index else ""
        following = tokens[index + 1].text if index + 1 < len(tokens) else ""
        if previous == "." or following in {".", "("}:
            continue
        if (
            token.text in constructor_names
            and token.text[:1].isupper()
        ):
            continue
        result.add(token.text)
    return result


def _comptime_pattern_expression_tokens(
    tokens: Sequence[Token],
) -> set[int]:
    """Mark `comptime` labels, whose payload is an expression, not a binder."""

    marked: set[int] = set()
    stack: list[str] = []
    expression_depth: int | None = None
    for index, token in enumerate(tokens):
        text = token.text
        if expression_depth is not None:
            if len(stack) == expression_depth and (
                text == "," or text in CLOSE_TO_OPEN
            ):
                expression_depth = None
            else:
                marked.add(index)
        if expression_depth is None and text == "comptime":
            expression_depth = len(stack)
            marked.add(index)
        _depth_step(stack, text, angles=False)
    return marked


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
    tokens: Sequence[Token],
    namespace_roots: set[str],
    *,
    include_callable_declarations: bool = False,
    include_top_level_fields: bool = False,
    constructor_pattern_names: set[str] | None = None,
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

    # Parameters and named results shadow a namespace throughout their
    # function or lambda body.
    for body_open, body_close, param_open, param_close in callables:
        if param_open is not None and param_close is not None:
            for parameter in split_top(
                tokens[param_open + 1 : param_close], ","
            ):
                colon = find_top(parameter, ":", angles=False)
                binding = parameter if colon is None else parameter[:colon]
                add(
                    _classic_binding_names(binding, namespace_roots),
                    body_open + 1,
                    body_close,
                )

        header_start = (
            param_close + 1 if param_close is not None else body_open
        )
        for cursor in range(header_start, body_open - 1):
            if (
                tokens[cursor].text != "returns"
                or tokens[cursor + 1].text != "("
            ):
                continue
            returns_close = matching_index(tokens, cursor + 1)
            if returns_close is None or returns_close >= body_open:
                break
            for result in split_top(
                tokens[cursor + 2 : returns_close], ","
            ):
                colon = find_top(result, ":", angles=False)
                if colon is None:
                    continue
                add(
                    _classic_binding_names(
                        result[:colon], namespace_roots
                    ),
                    body_open + 1,
                    body_close,
                )
            break

    if include_callable_declarations:
        class_scope_opens = {
            boundary
            for index, token in enumerate(tokens)
            if token.text in {"trait", "class"}
            and (boundary := _header_boundary(tokens, index + 1)) is not None
            and tokens[boundary].text == "{"
        }
        implementation_scope_opens = {
            boundary
            for index, token in enumerate(tokens)
            if token.text in {"impl", "instance"}
            and (boundary := _header_boundary(tokens, index + 1)) is not None
            and tokens[boundary].text == "{"
        }
        contract_scope_opens = {
            boundary
            for index, token in enumerate(tokens)
            if token.text in {"contract", "interface", "library"}
            and (boundary := _header_boundary(tokens, index + 1)) is not None
            and tokens[boundary].text == "{"
        }
        class_method_counts: dict[str, int] = {}
        for index, token in enumerate(tokens[:-1]):
            if (
                token.text == "function"
                and enclosing[index] in class_scope_opens
                and tokens[index + 1].kind == "word"
            ):
                name = tokens[index + 1].text
                class_method_counts[name] = class_method_counts.get(name, 0) + 1
        # A named top-level function shadows the term globally. Class methods
        # are looked up by their unqualified leaf only when exactly one class
        # declares that leaf. Implementations do not add another module term.
        # Contract methods remain confined to their enclosing contract scope.
        for index, token in enumerate(tokens[:-1]):
            if (
                token.text != "function"
                or tokens[index + 1].kind != "word"
                or tokens[index + 1].text not in namespace_roots
            ):
                continue
            name = tokens[index + 1].text
            scope_open = enclosing[index]
            boundary = _header_boundary(tokens, index + 2)
            cursor = index + 2
            if cursor < len(tokens) and tokens[cursor].text == "<":
                generic_close = matching_index(tokens, cursor)
                cursor = (
                    generic_close + 1
                    if generic_close is not None
                    else len(tokens)
                )
            parameter_close = (
                matching_index(tokens, cursor)
                if cursor < len(tokens) and tokens[cursor].text == "("
                else None
            )
            modifier_start = (
                parameter_close + 1
                if parameter_close is not None
                else boundary
            )
            visibility: str | None = None
            if modifier_start is not None and boundary is not None:
                for item in tokens[modifier_start:boundary]:
                    if item.text not in MODIFIERS:
                        break
                    if item.text in {
                        "public",
                        "external",
                        "internal",
                        "private",
                    }:
                        visibility = item.text
            is_self_external = (
                scope_open in contract_scope_opens
                and visibility == "external"
            )
            if scope_open is None:
                add({name}, 0, len(tokens))
            elif scope_open in class_scope_opens:
                if class_method_counts.get(name) == 1:
                    add({name}, 0, len(tokens))
            elif scope_open not in implementation_scope_opens:
                if not is_self_external:
                    add(
                        {name},
                        scope_open + 1,
                        brace_pairs.get(scope_open, len(tokens)),
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

    if include_top_level_fields:
        # Preserve the previous conservative constructor behavior for any
        # top-level value fields: if accepted, its term is module-wide.
        for index in range(1, len(tokens) - 1):
            token = tokens[index]
            if (
                enclosing[index] is None
                and token.kind == "word"
                and token.text in namespace_roots
                and tokens[index + 1].text == ":"
                and tokens[index - 1].text in {";", "}"}
            ):
                add({token.text}, 0, len(tokens))

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
                tokens[index + 1 : body_open],
                namespace_roots,
                constructor_pattern_names,
            ),
            body_open + 1,
            brace_pairs[body_open],
        )

    # Classic `| pattern => expression` binders end at the next match arm.
    for index, token in enumerate(tokens):
        if token.text != "match":
            continue
        match_open = _expression_block_boundary(tokens, index + 1)
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
                    tokens[arm_start + 1 : arrow],
                    namespace_roots,
                    constructor_pattern_names,
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
    bare_imports: list[tuple[int, int, list[str], list[str], str]] = []
    namespace_paths: list[tuple[list[str], str]] = []
    replacements: list[tuple[int, int, str]] = []
    imported_local_names: set[str] = set()

    for index, _ in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if token.text != "import":
            continue
        end = _statement_end(tokens, index)
        if end is None:
            continue
        body = list(tokens[index + 1 : end])
        if not body:
            continue
        if (
            len(body) >= 3
            and body[0].text == "*"
            and body[1].text == "as"
            and body[2].kind == "word"
        ):
            imported_local_names.add(body[2].text)
            continue
        selector_open = find_top(body, "{", angles=False)
        if selector_open is not None:
            close = matching_index(body, selector_open)
            if close is not None:
                for part in split_top(
                    body[selector_open + 1 : close], ",", angles=False
                ):
                    if not part:
                        continue
                    as_index = find_top(part, "as", angles=False)
                    local = (
                        part[as_index + 1]
                        if as_index is not None and as_index + 1 < len(part)
                        else part[0]
                    )
                    if local.kind == "word":
                        imported_local_names.add(local.text)
            continue
        explicit_alias = find_top(body, "as", angles=False)
        if (
            explicit_alias is not None
            and explicit_alias + 1 < len(body)
            and body[explicit_alias + 1].kind == "word"
        ):
            imported_local_names.add(body[explicit_alias + 1].text)
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
        visible = segments[1:] if external else segments
        bare_imports.append(
            (token.start, tokens[end].end, segments, visible, path)
        )
        import_spans.append((token.start, tokens[end].end))

    suffix_names = {
        "_".join(segments[-width:])
        for _, _, segments, _, _ in bare_imports
        for width in range(1, len(segments) + 1)
    }
    alias_shadow_ranges = _classic_shadow_ranges(
        tokens,
        suffix_names,
        include_callable_declarations=True,
        include_top_level_fields=True,
    )
    unavailable_aliases = imported_local_names | {
        name for name, ranges in alias_shadow_ranges.items() if ranges
    }
    declared_item_names = {
        tokens[index + 1].text
        for index, token in enumerate(tokens[:-1])
        if token.text in {
            "alias",
            "class",
            "contract",
            "data",
            "enum",
            "interface",
            "library",
            "struct",
            "trait",
            "type",
        }
        and tokens[index + 1].kind == "word"
    }
    unavailable_aliases.update(declared_item_names)
    leaf_paths: dict[str, set[str]] = {}
    for _, _, segments, _, path in bare_imports:
        leaf_paths.setdefault(segments[-1], set()).add(path)

    assigned_aliases: set[str] = set()
    aliases_by_path: dict[str, str] = {}
    source_words = {token.text for token in tokens if token.kind == "word"}
    for start, end, segments, visible, path in bare_imports:
        alias = aliases_by_path.get(path)
        if alias is None:
            first_width = 2 if len(leaf_paths[segments[-1]]) > 1 else 1
            alias = ""
            for width in range(first_width, len(segments) + 1):
                candidate = "_".join(segments[-width:])
                if (
                    candidate not in unavailable_aliases
                    and candidate not in assigned_aliases
                ):
                    alias = candidate
                    break
            if not alias:
                base = "_".join(segments)
                suffix = 2
                alias = base
                while (
                    alias in unavailable_aliases
                    or alias in assigned_aliases
                    or alias in source_words
                ):
                    alias = f"{base}{suffix}"
                    suffix += 1
            aliases_by_path[path] = alias
            assigned_aliases.add(alias)
        replacement = _with_preserved_comments(
            source,
            start,
            end,
            f"import * as {alias} from {path};",
        )
        replacements.append((start, end, replacement))
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

    seen_uses: list[tuple[int, int]] = []
    for segments, alias in sorted(
        namespace_paths, key=lambda item: len(item[0]), reverse=True
    ):
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
            if inside_import(start, end) or any(
                start < seen_end and seen_start < end
                for seen_start, seen_end in seen_uses
            ):
                continue
            if index not in type_only_tokens and any(
                scope_start <= index < scope_end
                for scope_start, scope_end in shadow_ranges[segments[0]]
            ):
                continue
            seen_uses.append((start, end))
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


def _expression_block_boundary(
    tokens: Sequence[Token], start: int
) -> int | None:
    """Find an expression's body without treating comparison ``<`` as generic."""

    stack: list[str] = []
    for index in range(start, len(tokens)):
        text = tokens[index].text
        if not stack and text in {"{", ";"}:
            return index
        _depth_step(stack, text, angles=False)
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


def reject_named_call_arguments(source: str) -> None:
    """Reject named call/struct arguments before colons look like annotations."""

    tokens = significant(source)
    delimiter_stack: list[int] = []
    closing = {")": "(", "]": "[", "}": "{"}
    for open_index, token in enumerate(tokens):
        if token.text in closing:
            if (
                delimiter_stack
                and tokens[delimiter_stack[-1]].text == closing[token.text]
            ):
                delimiter_stack.pop()
            continue
        if token.text not in {"(", "[", "{"}:
            continue
        if token.text == "{" and any(
            tokens[index].text == "(" for index in delimiter_stack
        ):
            close_index = matching_index(tokens, open_index)
            if close_index is not None:
                parts = split_top(
                    tokens[open_index + 1 : close_index],
                    ",",
                    angles=False,
                )
                if len(parts) > 1 and not parts[-1]:
                    parts = parts[:-1]
                named = bool(parts) and all(
                    len(part) >= 3
                    and part[0].kind == "word"
                    and find_top(part, ":", angles=False) == 1
                    and find_top(part, ";", angles=False) is None
                    for part in parts
                )
                if named:
                    line, column = _source_line_column(
                        source, tokens[open_index].start
                    )
                    raise ValueError(
                        "cannot migrate named call or struct arguments at "
                        f"line {line}, column {column}: Core has no "
                        "`{name: value}` argument surface; use positional "
                        "arguments in declaration order"
                    )
        delimiter_stack.append(open_index)


def _begins_statement(tokens: Sequence[Token], index: int) -> bool:
    """Return whether ``index`` can begin a standalone source statement."""

    if index == 0 or tokens[index - 1].text in {"{", "}", ";", "else"}:
        return True
    start = _previous_boundary(tokens, index)
    prefix = list(tokens[start:index])
    if (
        len(prefix) >= 3
        and prefix[0].text in {"if", "while", "for"}
        and prefix[1].text == "("
    ):
        return matching_index(prefix, 1) == len(prefix) - 1
    return False


def _unsupported_solidity_construct(
    source: str,
) -> tuple[Token, str, str] | None:
    """Find omitted Solidity sugar without rejecting ordinary identifiers."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if token.text == "receive":
            start = _previous_boundary(tokens, index)
            prefix = tokens[start:index]
            if prefix and not all(
                item.kind == "word" and item.text in MODIFIERS
                for item in prefix
            ):
                continue
            if index + 1 >= len(tokens) or tokens[index + 1].text != "(":
                continue
            close = matching_index(tokens, index + 1)
            if close is None:
                continue
            boundary = _header_boundary(tokens, close + 1)
            if boundary is None:
                continue
            trailer = tokens[close + 1 : boundary]
            if (
                all(
                    item.kind == "word" and item.text in MODIFIERS
                    for item in trailer
                )
                and (
                    tokens[boundary].text == "{"
                    or (bool(trailer) and tokens[boundary].text == ";")
                )
            ):
                return (
                    token,
                    "Solidity `receive` declaration",
                    "Core uses the general `fallback` entry point",
                )

        if (
            token.text in {"event", "error"}
            and _previous_boundary(tokens, index) == index
            and index + 2 < len(tokens)
            and tokens[index + 1].kind == "word"
            and tokens[index + 2].text == "("
        ):
            close = matching_index(tokens, index + 2)
            if close is None:
                continue
            cursor = close + 1
            if (
                token.text == "event"
                and cursor < len(tokens)
                and tokens[cursor].text == "anonymous"
            ):
                cursor += 1
            if cursor < len(tokens) and tokens[cursor].text == ";":
                if token.text == "event":
                    return (
                        token,
                        "Solidity event declaration",
                        "use an explicit log or standard-library event helper",
                    )
                return (
                    token,
                    "Solidity custom-error declaration",
                    "use an explicit revert-data helper",
                )

        if (
            token.text == "modifier"
            and _previous_boundary(tokens, index) == index
            and index + 1 < len(tokens)
            and tokens[index + 1].kind == "word"
        ):
            cursor = index + 2
            if cursor < len(tokens) and tokens[cursor].text == "(":
                close = matching_index(tokens, cursor)
                if close is None:
                    continue
                cursor = close + 1
            boundary = _header_boundary(tokens, cursor)
            if boundary is not None and tokens[boundary].text == "{":
                return (
                    token,
                    "Solidity modifier declaration",
                    "replace the modifier with an ordinary helper function",
                )

        if (
            token.text == "emit"
            and _begins_statement(tokens, index)
            and index + 2 < len(tokens)
            and tokens[index + 1].kind == "word"
        ):
            cursor = index + 2
            while (
                cursor + 1 < len(tokens)
                and tokens[cursor].text == "."
                and tokens[cursor + 1].kind == "word"
            ):
                cursor += 2
            if cursor < len(tokens) and tokens[cursor].text == "(":
                close = matching_index(tokens, cursor)
                if (
                    close is not None
                    and close + 1 < len(tokens)
                    and tokens[close + 1].text == ";"
                ):
                    return (
                        token,
                        "Solidity `emit` statement",
                        "use an explicit log or standard-library event helper",
                    )

        if token.text == "new" and index + 1 < len(tokens):
            cursor = index + 1
            if tokens[cursor].kind != "word":
                continue
            cursor += 1
            while (
                cursor + 1 < len(tokens)
                and tokens[cursor].text == "."
                and tokens[cursor + 1].kind == "word"
            ):
                cursor += 2
            if cursor < len(tokens) and tokens[cursor].text == "<":
                close = matching_index(tokens, cursor)
                if close is None:
                    continue
                cursor = close + 1
            while cursor < len(tokens) and tokens[cursor].text == "[":
                close = matching_index(tokens, cursor)
                if close is None:
                    break
                cursor = close + 1
            if cursor < len(tokens) and tokens[cursor].text in {"(", "{"}:
                return (
                    token,
                    "Solidity `new` creation expression",
                    "use the explicit standard-library creation operation",
                )

        if token.text == "revert":
            if (
                index + 1 < len(tokens)
                and tokens[index + 1].text == ";"
                and _begins_statement(tokens, index)
            ):
                continue
            return (
                token,
                "Classic `revert` identifier or custom-error revert",
                "Core only supports bare `revert;`; use an explicit "
                "revert-data helper for payloads",
            )
    return None


def reject_unsupported_solidity_sugar(source: str) -> None:
    """Reject Solidity constructs deliberately omitted from the Core surface."""

    unsupported = _unsupported_solidity_construct(source)
    if unsupported is None:
        return
    token, construct, guidance = unsupported
    line, column = _source_line_column(source, token.start)
    raise ValueError(
        f"cannot migrate {construct} at line {line}, column {column}: "
        f"{guidance}"
    )


def reject_generic_fallback(source: str) -> None:
    """Reject Classic generic or constrained fallback declarations."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if (
            token.text != "fallback"
            or index + 1 >= len(tokens)
            or tokens[index + 1].text != "("
        ):
            continue
        start = _previous_boundary(tokens, index)
        prefix = list(tokens[start:index])
        non_modifiers = [
            item for item in prefix if item.text not in MODIFIERS
        ]
        if not non_modifiers:
            continue
        _, _, had_forall = _parse_forall_prefix(non_modifiers)
        if not had_forall and not any(
            item.text == "=>" for item in non_modifiers
        ):
            continue
        line, column = _source_line_column(source, non_modifiers[0].start)
        raise ValueError(
            "cannot migrate generic or constrained fallback at "
            f"line {line}, column {column}: Core fallback declarations "
            "cannot bind type parameters or carry trait predicates"
        )


def reject_comptime_tuple_bindings(source: str) -> None:
    """Reject tuple destructuring that would acquire a binding modifier."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if token.text != "let" or index + 1 >= len(tokens):
            continue
        first = index + 1
        if (
            tokens[first].text == "comptime"
            and first + 1 < len(tokens)
            and tokens[first + 1].text == "("
        ):
            offending = tokens[first]
        else:
            stack: list[str] = []
            colon: int | None = None
            end: int | None = None
            for cursor in range(first, len(tokens)):
                text = tokens[cursor].text
                if not stack and text == ":":
                    colon = cursor
                if not stack and text in {"=", ":=", ";"}:
                    end = cursor
                    break
                _depth_step(stack, text, angles=False)
            if (
                colon is None
                or end is None
                or colon <= first
                or colon + 1 >= end
                or tokens[first].text != "("
                or tokens[colon + 1].text != "comptime"
            ):
                continue
            offending = tokens[colon + 1]
        line, column = _source_line_column(source, offending.start)
        raise ValueError(
            "cannot migrate comptime tuple destructuring at "
            f"line {line}, column {column}: Core does not support the "
            "`comptime` binding modifier on tuple patterns; split the "
            "destructuring into explicitly typed scalar bindings"
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
            and tokens[inheritance].text in {"(", "<"}
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


def migrate_contract_type_parameters(source: str) -> str:
    """Rewrite Classic ``contract C(T)`` binders as ``contract C<T>``."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if (
            token.text not in {"contract", "interface", "library"}
            or index + 2 >= len(tokens)
            or tokens[index + 1].kind != "word"
            or tokens[index + 2].text != "("
        ):
            continue
        open_index = index + 2
        close_index = matching_index(tokens, open_index)
        if (
            close_index is None
            or close_index + 1 >= len(tokens)
            or tokens[close_index + 1].text != "{"
        ):
            continue
        body_tokens = list(tokens[open_index + 1 : close_index])
        parameters = split_top(body_tokens, ",", angles=False)
        trailing_comma = bool(body_tokens) and body_tokens[-1].text == ","
        if trailing_comma:
            parameters = parameters[:-1]
        if any(
            len(parameter) != 1 or parameter[0].kind != "word"
            for parameter in parameters
            if parameter
        ) or (
            len(parameters) > 1 and any(not parameter for parameter in parameters)
        ):
            continue
        names = [parameter[0].text for parameter in parameters if parameter]
        if names:
            replacements.extend(
                (
                    (
                        tokens[open_index].start,
                        tokens[open_index].end,
                        "<",
                    ),
                    (
                        tokens[close_index].start,
                        tokens[close_index].end,
                        ">",
                    ),
                )
            )
            if trailing_comma:
                replacements.append(
                    (
                        body_tokens[-1].start,
                        body_tokens[-1].end,
                        "",
                    )
                )
        else:
            if any(item.text == "," for item in body_tokens):
                continue
            replacements.extend(
                (
                    (
                        tokens[open_index].start,
                        tokens[open_index].end,
                        "",
                    ),
                    (
                        tokens[close_index].start,
                        tokens[close_index].end,
                        "",
                    ),
                )
            )
    return replace_spans(source, replacements)


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


def reject_malformed_let_type_delimiters(source: str) -> None:
    """Reject unbalanced typed-let spans that ordinary scans cannot migrate."""

    tokens = significant(source)
    for index, token in enumerate(tokens):
        if token.text != "let" or index + 2 >= len(tokens):
            continue
        cursor = index + 1
        if tokens[cursor].text == "comptime":
            cursor += 1
        if cursor >= len(tokens):
            continue
        if tokens[cursor].kind == "word":
            colon = cursor + 1
        elif tokens[cursor].text == "(":
            binding_close = matching_index(tokens, cursor)
            if binding_close is None:
                continue
            colon = binding_close + 1
        else:
            continue
        if colon >= len(tokens) or tokens[colon].text != ":":
            continue
        type_tail = _split_type_angle_operator_tokens(
            tokens[colon + 1 :]
        )
        terminator = next(
            (
                position
                for position, item in enumerate(type_tail)
                if item.text in {"=", ";", "{", "}"}
            ),
            None,
        )
        if terminator is None:
            terminator = len(type_tail)
        if terminator == 0:
            continue
        type_tokens = type_tail[:terminator]
        try:
            _validate_type_delimiters(type_tokens)
        except ValueError as error:
            line, column = _source_line_column(
                source, type_tokens[0].start
            )
            raise ValueError(
                f"{error} in typed let at line {line}, column {column}"
            ) from error


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


def _realign_let_comptime_tokens(
    before: Sequence[Token],
    after: Sequence[Token],
    aligned: dict[int, int],
) -> None:
    """Keep nested ``comptime`` comments attached when one layer moves."""

    if (
        len(before) < 4
        or len(after) < 4
        or before[0].text != "let"
        or after[0].text != "let"
        or before[1].text == "comptime"
        or after[1].text != "comptime"
    ):
        return
    old_colon = find_top(before, ":", angles=False)
    new_colon = find_top(after, ":", angles=False)
    if (
        old_colon is None
        or new_colon is None
        or old_colon + 1 >= len(before)
        or before[old_colon + 1].text != "comptime"
    ):
        return

    old_comptime: list[int] = []
    cursor = old_colon + 1
    while cursor < len(before) and before[cursor].text == "comptime":
        old_comptime.append(cursor)
        cursor += 1
    new_comptime = [1]
    cursor = new_colon + 1
    while cursor < len(after) and after[cursor].text == "comptime":
        new_comptime.append(cursor)
        cursor += 1
    if len(old_comptime) != len(new_comptime):
        return

    old_set = set(old_comptime)
    new_set = set(new_comptime)
    for old_index, new_index in list(aligned.items()):
        if old_index in old_set or new_index in new_set:
            del aligned[old_index]
    aligned.update(zip(old_comptime, new_comptime, strict=True))


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
    _realign_let_comptime_tokens(before, after, aligned)
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
) -> tuple[
    dict[str, set[str]],
    dict[str, list[tuple[int, int, str]]],
    set[int],
    dict[str, list[tuple[int, int, str, str]]],
]:
    """Collect module/scoped constructor owners and declaration tokens.

    Constructor use sites cannot be inferred safely from capitalization:
    declarations such as ``enum memory<a> { memory(word) }`` are valid source
    in the historical corpus.  Build the owner table structurally for every
    variant, exclude primitive constructors that intentionally remain
    unqualified, and keep complete enum/struct declarations out of the term
    rewrite.  Expression shadowing is resolved later, while constructor
    patterns retain structural priority over terms.
    """

    module_candidates: dict[str, set[str]] = {}
    scoped_candidates: dict[str, list[tuple[int, int, str]]] = {}
    promoted_aliases: dict[
        str,
        list[tuple[int, int, str, str]],
    ] = {}
    declarations: set[int] = set()
    brace_pairs, enclosing = _classic_brace_context(tokens)
    contract_scope_opens = {
        boundary
        for item_index, item in enumerate(tokens)
        if item.text in {"contract", "interface", "library"}
        and (boundary := _header_boundary(tokens, item_index + 1)) is not None
        and tokens[boundary].text == "{"
    }
    library_scope_names = {
        boundary: tokens[item_index + 1].text
        for item_index, item in enumerate(tokens[:-1])
        if item.text == "library"
        and enclosing[item_index] is None
        and tokens[item_index + 1].kind == "word"
        and (boundary := _header_boundary(tokens, item_index + 1)) is not None
        and tokens[boundary].text == "{"
    }

    def add_owner(leaf: str, owner: str, declaration_index: int) -> None:
        scope_open = enclosing[declaration_index]
        if scope_open is None:
            module_candidates.setdefault(leaf, set()).add(owner)
            return
        if scope_open not in contract_scope_opens:
            return
        scoped_candidates.setdefault(leaf, []).append(
            (
                scope_open + 1,
                brace_pairs.get(scope_open, len(tokens)),
                owner,
            )
        )
        library_name = library_scope_names.get(scope_open)
        if library_name is not None:
            promoted_owner = f"{library_name}.{owner}"
            module_candidates.setdefault(leaf, set()).add(
                promoted_owner
            )
            promoted_aliases.setdefault(leaf, []).append(
                (
                    scope_open + 1,
                    brace_pairs.get(scope_open, len(tokens)),
                    owner,
                    promoted_owner,
                )
            )

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
                add_owner(name, name, index)
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
            if (
                leaf in BUILTIN_CONSTRUCTORS
                or (
                    leaf in BUILTIN_TYPE_NAMES
                    and leaf != name
                )
            ):
                continue
            add_owner(leaf, name, index)
    return (
        module_candidates,
        scoped_candidates,
        declarations,
        promoted_aliases,
    )


def _dot_constructor_owner_candidates(
    tokens: Sequence[Token],
) -> tuple[
    dict[str, set[str]],
    dict[str, list[tuple[int, int, str]]],
    dict[str, list[tuple[int, int, str, str]]],
]:
    """Collect every enum constructor owner for Classic ``.Leaf`` uses.

    The older bare-constructor fallback intentionally considers only
    same-name constructors because an ordinary bare call can otherwise be
    indistinguishable from a function call.  A prefix dot is explicit Classic
    constructor syntax, so all enum variants are safe candidates.  Structs
    contribute their implicit same-name constructor as well.
    """

    module_candidates: dict[str, set[str]] = {}
    scoped_candidates: dict[str, list[tuple[int, int, str]]] = {}
    promoted_aliases: dict[
        str,
        list[tuple[int, int, str, str]],
    ] = {}
    brace_pairs, enclosing = _classic_brace_context(tokens)
    contract_scope_opens = {
        boundary
        for item_index, item in enumerate(tokens)
        if item.text in {"contract", "interface", "library"}
        and (boundary := _header_boundary(tokens, item_index + 1)) is not None
        and tokens[boundary].text == "{"
    }
    library_scope_names = {
        boundary: tokens[item_index + 1].text
        for item_index, item in enumerate(tokens[:-1])
        if item.text == "library"
        and enclosing[item_index] is None
        and tokens[item_index + 1].kind == "word"
        and (boundary := _header_boundary(tokens, item_index + 1)) is not None
        and tokens[boundary].text == "{"
    }

    def add_owner(leaf: str, owner: str, declaration_index: int) -> None:
        scope_open = enclosing[declaration_index]
        if scope_open is None:
            module_candidates.setdefault(leaf, set()).add(owner)
            return
        if scope_open not in contract_scope_opens:
            return
        scoped_candidates.setdefault(leaf, []).append(
            (
                scope_open + 1,
                brace_pairs.get(scope_open, len(tokens)),
                owner,
            )
        )
        library_name = library_scope_names.get(scope_open)
        if library_name is not None:
            promoted_owner = f"{library_name}.{owner}"
            module_candidates.setdefault(leaf, set()).add(
                promoted_owner
            )
            promoted_aliases.setdefault(leaf, []).append(
                (
                    scope_open + 1,
                    brace_pairs.get(scope_open, len(tokens)),
                    owner,
                    promoted_owner,
                )
            )

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
            add_owner(owner, owner, index)
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
            add_owner(constructor[0].text, owner, index)
    return module_candidates, scoped_candidates, promoted_aliases


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
    """Return executable body/initializer ranges and declaration headers."""

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

    brace_pairs, enclosing = _classic_brace_context(tokens)
    contract_scope_opens = {
        boundary
        for index, token in enumerate(tokens)
        if token.text in {"contract", "interface", "library"}
        and (boundary := _header_boundary(tokens, index + 1)) is not None
        and tokens[boundary].text == "{"
        and boundary in brace_pairs
    }
    for index in range(1, len(tokens) - 1):
        scope_open = enclosing[index]
        if (
            scope_open not in contract_scope_opens
            or tokens[index].kind != "word"
            or tokens[index + 1].text != ":"
            or tokens[index - 1].text not in {"{", "}", ";"}
        ):
            continue
        statement_end = _statement_end(tokens, index)
        if (
            statement_end is None
            or statement_end >= brace_pairs[scope_open]
        ):
            continue
        equals = find_top(
            tokens[index + 2 : statement_end],
            "=",
            angles=False,
        )
        if equals is not None:
            bodies.append((index + 2 + equals + 1, statement_end))
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
    for index, region_end in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if token.text in statement_items:
            marked.update(range(index, region_end + 1))
        elif token.text in header_items:
            boundary = _header_boundary(tokens, index + 1)
            if boundary is not None:
                marked.update(range(index, boundary + 1))
    return marked


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
    regions = _provider_top_level_item_regions(tokens)
    names = {
        tokens[index + 1].text
        for index, _ in regions
        if (
            tokens[index].text in type_items
            and index + 1 < len(tokens)
            and tokens[index + 1].kind == "word"
        )
    }
    for index, _ in regions:
        token = tokens[index]
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
            following = (
                tokens[close + 1]
                if close + 1 < body_end
                else None
            )
            if (
                following is not None
                and following.kind in {"word", "number", "string"}
                and following.text not in LOCATIONS
            ):
                # In an executable region, ``a < T(x) > b`` is a pair of
                # comparisons rather than a generic argument list.  A type
                # application can only be followed by another bare word when
                # that word is a storage-location suffix.
                continue
            marked.update(range(index + 1, close))
    return marked


def _body_binding_tokens(
    tokens: Sequence[Token], body_start: int, body_end: int
) -> tuple[set[int], set[int]]:
    """Mark bindings and bare match names that may resolve as constructors."""

    marked: set[int] = set()
    bare_match_bindings: set[int] = set()
    for index in range(body_start, body_end):
        if tokens[index].text != "let":
            continue
        stack: list[str] = []
        for cursor in range(index + 1, body_end):
            text = tokens[cursor].text
            if not stack and text in {":", "=", ":=", ";"}:
                marked.update(range(index + 1, cursor))
                break
            _depth_step(stack, text, angles=False)

    # A bare lowercase identifier in a match pattern is a binder. Lowercase
    # constructors remain distinguishable through arguments or qualification.
    for index in range(body_start, body_end):
        if tokens[index].text != "case":
            continue
        arm_open = _header_boundary(tokens, index + 1)
        if (
            arm_open is None
            or arm_open >= body_end
            or tokens[arm_open].text != "{"
        ):
            continue
        pattern = tokens[index + 1 : arm_open]
        comptime_expression_tokens = _comptime_pattern_expression_tokens(
            pattern
        )
        for cursor in range(index + 1, arm_open):
            token = tokens[cursor]
            if (
                cursor - index - 1 in comptime_expression_tokens
                or token.kind != "word"
                or not token.text[:1].islower()
                or (cursor > index + 1 and tokens[cursor - 1].text == ".")
                or (
                    cursor + 1 < arm_open
                    and tokens[cursor + 1].text in {"(", "."}
                )
            ):
                continue
            marked.add(cursor)
            bare_match_bindings.add(cursor)
    return marked, bare_match_bindings


def _body_constructor_pattern_tokens(
    tokens: Sequence[Token], body_start: int, body_end: int
) -> set[int]:
    """Mark unqualified constructor heads, which ignore value shadowing."""

    marked: set[int] = set()
    for index in range(body_start, body_end):
        if tokens[index].text != "case":
            continue
        arm_open = _header_boundary(tokens, index + 1)
        if (
            arm_open is None
            or arm_open >= body_end
            or tokens[arm_open].text != "{"
        ):
            continue
        pattern = tokens[index + 1 : arm_open]
        comptime_expression_tokens = _comptime_pattern_expression_tokens(
            pattern
        )
        for cursor in range(index + 1, arm_open):
            token = tokens[cursor]
            offset = cursor - index - 1
            previous = tokens[cursor - 1].text
            following = tokens[cursor + 1].text
            if (
                offset in comptime_expression_tokens
                or token.kind != "word"
                or following == "."
                or (
                    previous != "."
                    and token.text[:1].islower()
                    and following != "("
                )
            ):
                continue
            marked.add(cursor)
    return marked


def migrate_qualified_constructors(
    source: str,
    global_owners: Mapping[str, str] | None = None,
    import_surface: ConstructorImportSurface | None = None,
) -> str:
    """Qualify term and pattern uses with local or proven imported owners."""

    if has_comment_marker(source, KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER):
        return source

    surface = import_surface or EMPTY_CONSTRUCTOR_IMPORT_SURFACE
    tokens = significant(source)
    local_qualified_term_winners = _local_qualified_term_winners(tokens)
    (
        module_candidates,
        scoped_candidates,
        declaration_tokens,
        promoted_aliases,
    ) = _constructor_owner_candidates(tokens)
    module_owners = _unique_constructor_owners(module_candidates)
    # ``global_owners`` is retained for direct API compatibility.  The CLI no
    # longer builds this repository-wide spelling table: imported owners carry
    # provider identity in ``surface`` and are considered only by consumers
    # whose import declarations actually expose them.
    constructor_owners = dict(global_owners or {})
    for namespace_name in _declared_type_and_module_names(tokens):
        constructor_owners.pop(namespace_name, None)
    trusted_import_bindings: dict[
        str,
        frozenset[ConstructorBinding],
    ] = {}
    ambiguous_import_leaves: set[str] = set()
    imported_constructor_leaves = set(surface.bare_candidates)
    for leaf, bindings in surface.bare_candidates.items():
        origins = {candidate.origin for candidate in bindings}
        safe_bindings = frozenset(
            candidate
            for candidate in bindings
            if not _constructor_qualification_conflict_targets(
                surface,
                candidate.owner,
                leaf,
            )
        )
        binding = (
            _single_origin_constructor_binding(safe_bindings)
            if len(origins) == 1
            else None
        )
        if (
            binding is not None
            and not surface.has_unknown_unqualified_constructors
        ):
            owner = binding.owner
            constructor_owners[leaf] = owner
            trusted_import_bindings[leaf] = safe_bindings
        else:
            constructor_owners.pop(leaf, None)
            ambiguous_import_leaves.add(leaf)
    # A declaration in this source is more precise than a repository-wide
    # compatibility spelling.  An actual imported declaration is not a guess,
    # however: a local and imported constructor with the same leaf are
    # different origins and must remain ambiguous.
    constructor_owners.update(module_owners)
    for leaf, candidates in module_candidates.items():
        module_owner = module_owners.get(leaf)
        imported_bindings = surface.bare_candidates.get(leaf, ())
        local_precedes_same_import_owner = (
            module_owner is not None
            and bool(imported_bindings)
            and {
                binding.owner for binding in imported_bindings
            }
            == {module_owner}
            and not _constructor_owner_conflict_targets(
                surface,
                module_owner,
            )
        )
        if leaf in imported_constructor_leaves:
            trusted_import_bindings.pop(leaf, None)
        if (
            len(candidates) != 1
            or (
                leaf in imported_constructor_leaves
                and not local_precedes_same_import_owner
            )
            or (
                leaf in module_owners
                and (
                    _constructor_owner_conflict_targets(
                        surface,
                        module_owners[leaf],
                    )
                    or local_qualified_term_winners.get(
                        f"{module_owners[leaf]}.{leaf}"
                    )
                    == "term"
                )
            )
        ):
            constructor_owners.pop(leaf, None)
    constructor_leaves = (
        set(constructor_owners)
        | set(scoped_candidates)
        | ambiguous_import_leaves
    )
    if not constructor_leaves:
        return source

    owner_roots = {
        owner.split(".", 1)[0]
        for owner in constructor_owners.values()
    }
    owner_roots.update(
        binding.owner.split(".", 1)[0]
        for bindings in trusted_import_bindings.values()
        for binding in bindings
    )
    owner_roots.update(
        owner.split(".", 1)[0]
        for candidates in scoped_candidates.values()
        for _, _, owner in candidates
    )
    owner_shadow_ranges = _classic_shadow_ranges(
        tokens,
        owner_roots,
        include_top_level_fields=True,
        constructor_pattern_names=constructor_leaves,
    )
    def owner_is_shadowed(
        owner: str,
        index: int,
        *,
        respect_term_shadowing: bool,
    ) -> bool:
        if not respect_term_shadowing:
            return False
        root = owner.split(".", 1)[0]
        return any(
            start <= index < end
            for start, end in owner_shadow_ranges[root]
        )

    def owner_at(
        index: int,
        leaf: str,
        *,
        respect_term_shadowing: bool = True,
    ) -> tuple[str | None, bool, bool]:
        if surface.has_unknown_unqualified_constructors:
            # Classic constructor leaves are resolved across every visible
            # imported type namespace.  An unresolved open, selective, or
            # namespace import can therefore collide even with a declaration
            # in this source; do not guess that the local owner wins.
            return None, leaf in module_owners, False
        scoped = {
            owner
            for start, end, owner in scoped_candidates.get(leaf, [])
            if start <= index < end
            and not owner_is_shadowed(
                owner,
                index,
                respect_term_shadowing=respect_term_shadowing,
            )
        }
        if scoped:
            outer_owners = set(module_candidates.get(leaf, set()))
            outer_owners.difference_update(
                promoted_owner
                for start, end, scoped_owner, promoted_owner in (
                    promoted_aliases.get(leaf, ())
                )
                if (
                    start <= index < end
                    and scoped_owner in scoped
                )
            )
            outer_owners.update(
                binding.owner
                for binding in surface.bare_candidates.get(leaf, ())
            )
            if (
                len(scoped) != 1
                or (
                    outer_owners
                    and not outer_owners.issubset(scoped)
                )
            ):
                return None, True, False
            return (
                next(iter(scoped)),
                True,
                False,
            )
        imported = trusted_import_bindings.get(leaf)
        if imported is not None:
            binding = _single_origin_constructor_binding(
                binding
                for binding in imported
                if not owner_is_shadowed(
                    binding.owner,
                    index,
                    respect_term_shadowing=respect_term_shadowing,
                )
            )
            return (
                binding.owner if binding is not None else None,
                False,
                binding is not None,
            )
        owner = constructor_owners.get(leaf)
        is_local = leaf in module_owners
        if (
            owner is not None
            and owner_is_shadowed(
                owner,
                index,
                respect_term_shadowing=respect_term_shadowing,
            )
        ):
            owner = None
        return owner, is_local, False

    shadow_ranges = _classic_shadow_ranges(
        tokens,
        constructor_leaves,
        include_callable_declarations=True,
        include_top_level_fields=True,
    )
    bodies, header_tokens = _executable_regions(tokens)
    nonterm_tokens = declaration_tokens | header_tokens | _declaration_surface_tokens(tokens)
    replacements: dict[int, tuple[int, int, str]] = {}
    body_tokens: set[int] = set()
    for body_start, body_end in bodies:
        body_tokens.update(range(body_start, body_end))
        type_tokens = _body_type_tokens(tokens, body_start, body_end)
        binding_tokens, bare_match_bindings = _body_binding_tokens(
            tokens, body_start, body_end
        )
        constructor_pattern_tokens = _body_constructor_pattern_tokens(
            tokens, body_start, body_end
        )
        for index in range(body_start, body_end):
            token = tokens[index]
            leaf = token.text
            if token.kind != "word" or leaf not in constructor_leaves:
                continue
            is_constructor_pattern = (
                index in constructor_pattern_tokens
                or index in bare_match_bindings
            )
            owner, is_local_owner, is_trusted_import = owner_at(
                index,
                leaf,
                respect_term_shadowing=not is_constructor_pattern,
            )
            if owner is None:
                continue
            if (
                (
                    not is_constructor_pattern
                    and (
                        leaf in surface.imported_terms
                        or leaf in surface.unknown_imported_terms
                        or surface.has_unknown_unqualified_terms
                        or any(
                            start <= index < end
                            for start, end in shadow_ranges[leaf]
                        )
                    )
                )
                or index in nonterm_tokens
                or index in type_tokens
                or (
                    index in binding_tokens
                    and not is_constructor_pattern
                )
            ):
                continue
            previous = tokens[index - 1].text if index else ""
            following = tokens[index + 1].text if index + 1 < len(tokens) else ""
            if previous == "." or following == ".":
                continue
            if (
                not is_local_owner
                and not is_trusted_import
                and following == "("
                and matching_index(tokens, index + 1) == index + 2
            ):
                # A cross-file `leaf()` is more likely a zero-argument
                # callable (notably `std.opcodes.address()`) than a payload
                # constructor.  Only a local declaration can disambiguate it.
                continue
            if (
                not is_local_owner
                and not is_trusted_import
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
                not is_local_owner
                and not is_trusted_import
                and following not in {"(", "as"}
                and previous not in {"return", "=", ":=", "case"}
            ):
                # With only a cross-file owner table, a bare identifier in an
                # argument or nested pattern may be an ordinary binding (for
                # example `case Nat.Succ(m)`).  Limit nullary rewrites to
                # positions that explicitly introduce a value.
                continue
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
            or token.text not in constructor_leaves
            or index in body_tokens
            or index in nonterm_tokens
            or any(
                start <= index < end
                for start, end in shadow_ranges[token.text]
            )
            or index + 1 >= len(tokens)
            or tokens[index + 1].text != "("
        ):
            continue
        previous = tokens[index - 1].text if index else ""
        if previous not in {"return", "=", ":=", "case"}:
            continue
        leaf = token.text
        owner, is_local_owner, is_trusted_import = owner_at(
            index,
            leaf,
            respect_term_shadowing=previous != "case",
        )
        if owner is None:
            continue
        is_pattern = previous == "case"
        if (
            not is_pattern
            and (
                leaf in surface.imported_terms
                or leaf in surface.unknown_imported_terms
                or surface.has_unknown_unqualified_terms
            )
        ):
            continue
        if (
            not is_local_owner
            and not is_trusted_import
            and matching_index(tokens, index + 1) == index + 2
        ):
            continue
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
                if index == 0 or tokens[index - 1].kind != "word":
                    return False
                root = index - 1
                while (
                    root >= 2
                    and tokens[root - 1].text == "."
                    and tokens[root - 2].kind == "word"
                ):
                    root -= 2
                context = tokens[root - 1].text if root else ""
                tightly_bound = (
                    tokens[index - 1].end == tokens[index].start
                    and close_index + 1 < len(tokens)
                    and tokens[close_index].end
                    == tokens[close_index + 1].start
                )
                return tightly_bound or context in {"@", ":", "as"}
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
    import_surface: ConstructorImportSurface | None = None,
) -> str:
    """Rewrite Classic ``.Leaf`` constructors from proven visible owners."""

    if has_comment_marker(source, KEEP_UNQUALIFIED_CONSTRUCTOR_MARKER):
        return source

    surface = import_surface or EMPTY_CONSTRUCTOR_IMPORT_SURFACE
    tokens = significant(source)
    local_qualified_term_winners = _local_qualified_term_winners(tokens)
    module_candidates, scoped_candidates, promoted_aliases = (
        _dot_constructor_owner_candidates(tokens)
    )
    candidates: dict[str, set[ConstructorBinding]] = {}

    # Retain the old direct-call API as an explicitly untrusted compatibility
    # input.  The public CLI never constructs this repository-global table.
    for leaf, owners in (global_candidates or {}).items():
        candidates[leaf] = {
            ConstructorBinding(
                ConstructorOrigin("<legacy-global>", owner, 0),
                owner,
            )
            for owner in owners
        }

    # A source-local declaration is more precise than a legacy compatibility
    # table.  Actual imported bindings are then unioned by origin so two
    # providers which both spell the owner ``T`` remain ambiguous.
    for leaf, owners in module_candidates.items():
        candidates[leaf] = {
            ConstructorBinding(
                ConstructorOrigin("<local>", owner, 0),
                owner,
            )
            for owner in owners
        }
    for leaf, bindings in surface.dot_candidates.items():
        candidates.setdefault(leaf, set()).update(bindings)

    owner_roots = {
        binding.owner.split(".", 1)[0]
        for bindings in candidates.values()
        for binding in bindings
    }
    owner_roots.update(
        owner.split(".", 1)[0]
        for scoped in scoped_candidates.values()
        for _, _, owner in scoped
    )
    owner_shadow_ranges = _classic_shadow_ranges(
        tokens,
        owner_roots,
        include_top_level_fields=True,
        constructor_pattern_names=(
            set(candidates) | set(scoped_candidates)
        ),
    )
    def binding_is_shadowed(
        binding: ConstructorBinding,
        index: int,
    ) -> bool:
        root = binding.owner.split(".", 1)[0]
        return any(
            start <= index < end
            for start, end in owner_shadow_ranges[root]
        )

    def binding_conflict_targets(
        binding: ConstructorBinding,
        leaf: str,
    ) -> tuple[str, ...]:
        if binding.origin.provider == "<local-scope>":
            return ()
        if binding.origin.provider == "<local>":
            targets = set(
                _constructor_owner_conflict_targets(
                    surface,
                    binding.owner,
                )
            )
            if local_qualified_term_winners.get(
                f"{binding.owner}.{leaf}"
            ) == "term":
                targets.add("source-local library term")
            return tuple(sorted(targets))
        return _constructor_qualification_conflict_targets(
            surface,
            binding.owner,
            leaf,
        )

    replacements: list[tuple[int, int, str]] = []
    errors: list[str] = []
    constructor_pattern_tokens: set[int] = set()
    for body_start, body_end in _executable_regions(tokens)[0]:
        constructor_pattern_tokens.update(
            _body_constructor_pattern_tokens(
                tokens,
                body_start,
                body_end,
            )
        )
    for index, token in enumerate(tokens):
        if not _is_legacy_dot_constructor(tokens, index):
            continue
        leaf = tokens[index + 1].text
        scoped_bindings = {
            ConstructorBinding(
                ConstructorOrigin("<local-scope>", owner, start),
                owner,
            )
            for start, end, owner in scoped_candidates.get(leaf, [])
            if start <= index < end
        }
        outer_bindings = set(candidates.get(leaf, set()))
        active_promoted_owners = {
            promoted_owner
            for start, end, scoped_owner, promoted_owner in (
                promoted_aliases.get(leaf, ())
            )
            if (
                start <= index < end
                and any(
                    binding.owner == scoped_owner
                    for binding in scoped_bindings
                )
            )
        }
        outer_bindings = {
            binding
            for binding in outer_bindings
            if not (
                binding.origin.provider == "<local>"
                and binding.owner in active_promoted_owners
            )
        }
        scoped_owners = {
            binding.owner for binding in scoped_bindings
        }
        if (
            len(scoped_owners) == 1
            and outer_bindings
            and {
                binding.owner for binding in outer_bindings
            }.issubset(scoped_owners)
        ):
            bindings = scoped_bindings
        else:
            bindings = outer_bindings | scoped_bindings
        if not scoped_bindings:
            local_bindings = {
                binding
                for binding in bindings
                if binding.origin.provider == "<local>"
            }
            if (
                len({binding.origin for binding in local_bindings}) == 1
                and {
                    binding.owner for binding in bindings
                }
                == {
                    binding.owner for binding in local_bindings
                }
                and all(
                    not binding_conflict_targets(binding, leaf)
                    for binding in local_bindings
                )
            ):
                bindings = local_bindings
        if (
            len({binding.origin for binding in bindings}) == 1
            and index + 1 not in constructor_pattern_tokens
        ):
            bindings = {
                binding
                for binding in bindings
                if not binding_is_shadowed(binding, index)
            }
        binding_origins = {candidate.origin for candidate in bindings}
        resolvable_bindings = {
            candidate
            for candidate in bindings
            if not binding_conflict_targets(candidate, leaf)
        }
        line, column = _source_line_column(source, token.start)
        location = f"line {line}, column {column}"
        binding = (
            _single_origin_constructor_binding(resolvable_bindings)
            if len(binding_origins) == 1
            else None
        )
        if (
            binding is not None
            and not surface.has_unknown_constructors
        ):
            # Insert the owner before the original dot instead of replacing
            # the whole span so comments between `.` and the leaf survive.
            replacements.append(
                (token.start, token.start, binding.owner)
            )
        elif bindings:
            owner_counts: dict[str, int] = {}
            for binding in bindings:
                owner_counts[binding.owner] = (
                    owner_counts.get(binding.owner, 0) + 1
                )
            if all(count == 1 for count in owner_counts.values()):
                rendered = ", ".join(sorted(owner_counts))
            else:
                rendered_bindings = []
                for binding in sorted(bindings):
                    if owner_counts[binding.owner] == 1:
                        rendered_bindings.append(binding.owner)
                    else:
                        rendered_bindings.append(
                            f"{binding.owner} "
                            f"(from {binding.origin.provider})"
                        )
                rendered = ", ".join(rendered_bindings)
            if surface.has_unknown_constructors:
                rendered += ", unresolved imported constructors"
            conflict_targets: dict[str, set[str]] = {}
            for candidate in bindings:
                targets = binding_conflict_targets(candidate, leaf)
                if targets:
                    conflict_targets.setdefault(
                        candidate.owner,
                        set(),
                    ).update(targets)
            owner_conflicts = [
                (
                    f"qualification {owner}.{leaf} conflicts with "
                    + ", ".join(sorted(targets))
                )
                for owner, targets in sorted(conflict_targets.items())
            ]
            if owner_conflicts:
                rendered += "; " + "; ".join(owner_conflicts)
            errors.append(
                f"ambiguous legacy dot-constructor .{leaf} at {location}; "
                f"possible owners: {rendered}; qualify it explicitly"
            )
        else:
            reason = (
                " because at least one imported constructor surface is "
                "unresolved"
                if surface.has_unknown_constructors
                else ""
            )
            errors.append(
                f"cannot resolve legacy dot-constructor .{leaf} at "
                f"{location}{reason}; include its enum declaration and "
                "export/import path in this migration "
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
            if all(
                _is_legacy_identifier_text(param)
                for param in candidate_params
            ):
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
        if variables:
            duplicate_classic = next(
                (
                    variable
                    for position, variable in enumerate(variables)
                    if variable in variables[:position]
                ),
                None,
            )
            overlapping = next(
                (
                    variable
                    for variable in variables
                    if variable in existing_variables
                ),
                None,
            )
            duplicate = duplicate_classic or overlapping
            if duplicate is not None:
                line, column = _source_line_column(
                    source, name_token.start
                )
                raise ValueError(
                    "cannot migrate duplicate Classic generic binder "
                    f"`{duplicate}` on function `{name_token.text}` at "
                    f"line {line}, column {column}"
                )
        merged_variables = [*existing_variables, *variables]
        if merged_variables:
            replacement += "<" + ", ".join(merged_variables) + ">"
        replacement += f"({params})"
        if modifiers:
            replacement += " " + " ".join(modifiers)
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
            replacement += " " + " ".join(modifiers)
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


def migrate_let_initializers(source: str) -> str:
    """Replace Classic ``let ... := ...`` outside opaque assembly blocks."""

    tokens = significant(source)
    replacements: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        if token.text != "let":
            continue
        stack: list[str] = []
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if not stack and text in {"=", ":=", ";"}:
                if text == ":=":
                    replacements.append(
                        (
                            tokens[cursor].start,
                            tokens[cursor].end,
                            "=",
                        )
                    )
                break
            _depth_step(stack, text, angles=False)
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
        combined_generic_assignment = False
        for cursor in range(index + 1, len(tokens)):
            text = tokens[cursor].text
            if not stack and text == ":":
                colon = cursor
            if not stack and text in {"=", ";"}:
                end = cursor
                break
            if text == ">=" and colon is not None:
                split_tail = _split_type_angle_operator_tokens(
                    tokens[colon + 1 : cursor + 1]
                )
                if split_tail and split_tail[-1].text == "=":
                    end = cursor
                    combined_generic_assignment = True
                    break
            _depth_step(stack, text, angles=False)
        if colon is None or end is None or colon >= end:
            continue
        binding_tokens = list(tokens[index + 1 : colon])
        type_tokens = list(tokens[colon + 1 : end])
        if combined_generic_assignment:
            type_tokens.append(
                Token(
                    "symbol",
                    ">",
                    tokens[end].start,
                    tokens[end].start + 1,
                )
            )
        if not type_tokens:
            continue
        already_comptime = (
            binding_tokens and binding_tokens[0].text == "comptime"
        )
        move_comptime = (
            type_tokens[0].text == "comptime" and not already_comptime
        )
        if move_comptime:
            type_tokens = type_tokens[1:]
        binding = join_tokens(binding_tokens)
        ty = render_type(type_tokens)
        if not binding or not ty:
            continue
        replacement = "let "
        if move_comptime:
            replacement += "comptime "
        replacement += f"{binding}: {ty}"
        if combined_generic_assignment:
            replacement += " = "
        elif tokens[end].text == "=":
            replacement += " "
        replacement_end = (
            tokens[end].end
            if combined_generic_assignment
            else tokens[end].start
        )
        replacement = _with_preserved_comments(
            source, token.start, replacement_end, replacement
        )
        replacements.append((token.start, replacement_end, replacement))
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
        brace = _expression_block_boundary(tokens, index + 1)
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
        boundary = _expression_block_boundary(tokens, index + 1)
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


def migrate_let_initializer_annotations(source: str) -> str:
    """Move a whole-initializer annotation onto its untyped ``let`` binding."""

    tokens = significant(source)
    executable_bodies = _executable_regions(tokens)[0]
    replacements: list[tuple[int, int, str]] = []
    for let_index, token in enumerate(tokens):
        if token.text != "let":
            continue

        stack: list[str] = []
        equals: int | None = None
        terminator: int | None = None
        for cursor in range(let_index + 1, len(tokens)):
            text = tokens[cursor].text
            if not stack and text == "=" and equals is None:
                equals = cursor
            elif not stack and text == ";":
                terminator = cursor
                break
            _depth_step(stack, text, angles=False)
        if (
            equals is None
            or terminator is None
            or equals + 1 >= terminator
        ):
            continue

        binding_tokens = list(tokens[let_index + 1 : equals])
        if (
            not binding_tokens
            or find_top(binding_tokens, ":", angles=False) is not None
        ):
            continue

        root_colons: list[int] = []
        stack = []
        for cursor in range(equals + 1, terminator):
            text = tokens[cursor].text
            if (
                not stack
                and text == ":"
                and _is_expression_annotation_colon(
                    tokens, cursor, executable_bodies
                )
            ):
                root_colons.append(cursor)
            _depth_step(stack, text, angles=False)
        if len(root_colons) != 1:
            continue

        colon = root_colons[0]
        if (
            colon <= equals + 1
            or _annotation_expression_start(tokens, colon) != equals + 1
        ):
            continue
        type_tokens = list(tokens[colon + 1 : terminator])
        if not type_tokens:
            continue

        already_comptime = binding_tokens[0].text == "comptime"
        move_comptime = (
            type_tokens[0].text == "comptime" and not already_comptime
        )
        if already_comptime and type_tokens[0].text == "comptime":
            continue
        if move_comptime:
            if binding_tokens[0].text == "(":
                continue
            type_tokens = type_tokens[1:]
        if not type_tokens:
            continue
        try:
            rendered_type = render_type(type_tokens)
        except ValueError:
            continue
        if not rendered_type:
            continue

        binding = join_tokens(binding_tokens)
        expression = join_tokens(tokens[equals + 1 : colon])
        if not binding or not expression:
            continue
        replacement = "let "
        if move_comptime:
            replacement += "comptime "
        replacement += f"{binding}: {rendered_type} = {expression}"
        replacement = _with_preserved_comments(
            source,
            token.start,
            tokens[terminator].start,
            replacement,
        )
        replacements.append(
            (token.start, tokens[terminator].start, replacement)
        )
    return replace_spans(source, replacements)


def reject_remaining_expression_annotations(source: str) -> None:
    """Fail closed where Classic inference guidance needs a manual binding."""

    tokens = significant(source)
    executable_bodies = _executable_regions(tokens)[0]
    for index, token in enumerate(tokens):
        if not _is_expression_annotation_colon(
            tokens, index, executable_bodies
        ):
            continue
        line, column = _source_line_column(source, token.start)
        raise ValueError(
            "cannot safely migrate Classic expression annotation at "
            f"line {line}, column {column}: `expression : Type` guides "
            "inference but `as` performs a checked conversion; introduce a "
            "typed binding such as `let value: Type = expression`"
        )


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
    *,
    constructor_import_surface: ConstructorImportSurface | None = None,
) -> str:
    if has_comment_marker(source, KEEP_LEGACY_NEGATIVE_MARKER):
        return source
    reject_operator_import_selectors(source)
    reject_string_imports(source)
    reject_contract_inheritance(source)
    reject_solidity_call_options(source)
    reject_named_call_arguments(source)
    reject_unsupported_solidity_sugar(source)
    reject_generic_fallback(source)
    reject_comptime_tuple_bindings(source)
    reject_noncanonical_proxy_comptime(source)
    reject_malformed_let_type_delimiters(source)
    reject_malformed_mapping_types(source)
    reject_noncanonical_function_type_qualifiers(source)
    passes = (
        migrate_pragmas,
        migrate_imports,
        migrate_contract_type_parameters,
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
        migrate_let_initializers,
        migrate_let_types,
        migrate_field_types,
        migrate_let_initializer_annotations,
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
            source,
            global_dot_constructor_candidates,
            constructor_import_surface,
        )
        source = migrate_qualified_constructors(
            source,
            global_constructor_owners,
            constructor_import_surface,
        )
        if source == before:
            reject_remaining_expression_annotations(source)
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
        candidates, _, _, _ = _constructor_owner_candidates(
            significant(canonical)
        )
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
        candidates, _, _ = _dot_constructor_owner_candidates(
            significant(canonical)
        )
        for leaf, owners in candidates.items():
            merged.setdefault(leaf, set()).update(owners)
    return merged


@dataclass(frozen=True)
class _ExportedDataType:
    origin: ConstructorOrigin
    source_name: str
    constructors: frozenset[str]


@dataclass(frozen=True)
class _ProviderInterface:
    data_types: Mapping[str, tuple[_ExportedDataType, ...]]
    type_origins: Mapping[str, tuple[ConstructorOrigin, ...]]
    term_origins: Mapping[str, tuple[ConstructorOrigin, ...]]
    terms: frozenset[str]
    public_names: frozenset[str]
    unknown: bool


@dataclass(frozen=True)
class _ImportSpec:
    kind: str
    external: bool
    path: tuple[str, ...]
    selections: tuple[tuple[str, str], ...] = ()
    qualifier: str | None = None


@dataclass(frozen=True)
class _ProviderConstructorSelector:
    kind: str
    names: tuple[str, ...] = ()


@dataclass(frozen=True)
class _ProviderExportName:
    kind: str
    name: str = ""
    path: tuple[str, ...] = ()
    constructors: _ProviderConstructorSelector | None = None


@dataclass(frozen=True)
class _ProviderExportSpec:
    kind: str
    path: tuple[str, ...] = ()
    names: tuple[_ProviderExportName, ...] = ()
    alias: str | None = None


@dataclass(frozen=True)
class _ProviderPlan:
    local_data: Mapping[str, tuple[_ExportedDataType, ...]]
    local_type_origins: Mapping[str, tuple[ConstructorOrigin, ...]]
    local_term_origins: Mapping[str, tuple[ConstructorOrigin, ...]]
    local_public_names: frozenset[str]
    imports: tuple[_ImportSpec, ...]
    exports: tuple[_ProviderExportSpec, ...]
    direct_unknown: bool


def _absolute_lexical_path(path: Path) -> Path:
    """Make a selected path absolute without resolving symlink identity."""

    return Path(os.path.abspath(os.fspath(path)))


def _parse_module_path(
    tokens: Sequence[Token],
) -> tuple[bool, tuple[str, ...]] | None:
    cursor = 0
    external = bool(tokens and tokens[0].text == "@")
    if external:
        cursor += 1
    segments: list[str] = []
    expect_name = True
    while cursor < len(tokens):
        token = tokens[cursor]
        if expect_name:
            if not _is_core_import_identifier(token):
                return None
            segments.append(token.text)
        elif token.text != ".":
            return None
        expect_name = not expect_name
        cursor += 1
    if expect_name or not segments:
        return None
    return external, tuple(segments)


def _provider_top_level_item_regions(
    tokens: Sequence[Token],
) -> list[tuple[int, int]]:
    """Return complete top-level item regions without scanning payloads."""

    statement_items = {"import", "export", "pragma", "alias", "type"}
    regions: list[tuple[int, int]] = []
    cursor = 0
    while cursor < len(tokens):
        start = cursor
        if tokens[start].text in statement_items:
            end = _provider_statement_end(tokens, start)
            if end is None:
                regions.append((start, len(tokens) - 1))
                break
            regions.append((start, end))
            cursor = end + 1
            continue

        boundary = _header_boundary(tokens, start + 1)
        if boundary is None:
            regions.append((start, len(tokens) - 1))
            break
        if tokens[boundary].text == ";":
            regions.append((start, boundary))
            cursor = boundary + 1
            continue
        close = matching_index(tokens, boundary)
        if close is None:
            regions.append((start, len(tokens) - 1))
            break
        end = close
        if end + 1 < len(tokens) and tokens[end + 1].text == ";":
            end += 1
        regions.append((start, end))
        cursor = end + 1
    return regions


def _provider_structural_tokens(
    tokens: Sequence[Token],
) -> list[Token]:
    """Exclude delimiter-opaque pragma payloads from balance checks."""

    structural: list[Token] = []
    for start, end in _provider_top_level_item_regions(tokens):
        if (
            tokens[start].text == "pragma"
            and start + 1 <= end
            and tokens[start + 1].text in {"solidity", "abicoder"}
        ):
            structural.extend(tokens[start : start + 2])
            structural.append(tokens[end])
        else:
            structural.extend(tokens[start : end + 1])
    return structural


def _parse_import_specs(
    source: str,
) -> tuple[list[_ImportSpec], bool]:
    """Parse the canonical import forms needed by constructor discovery."""

    tokens = significant(source)
    specs: list[_ImportSpec] = []
    malformed = False
    for index, region_end in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if token.text != "import":
            if (
                token.text != "pragma"
                and any(
                    item.text == "import"
                    for item in tokens[index + 1 : region_end + 1]
                )
            ):
                malformed = True
            continue
        end = _statement_end(tokens, index)
        if end is None:
            malformed = True
            continue
        body = list(tokens[index + 1 : end])
        if not body:
            malformed = True
            continue

        if body[0].text == "*":
            if (
                len(body) < 5
                or body[1].text != "as"
                or not _is_core_import_identifier(body[2])
                or body[3].text != "from"
            ):
                malformed = True
                continue
            parsed_path = _parse_module_path(body[4:])
            if parsed_path is None:
                malformed = True
                continue
            external, path = parsed_path
            specs.append(
                _ImportSpec(
                    "namespace",
                    external,
                    path,
                    qualifier=body[2].text,
                )
            )
            continue

        if body[0].text == "{":
            close = matching_index(body, 0)
            if (
                close is None
                or close + 1 >= len(body)
                or body[close + 1].text != "from"
            ):
                malformed = True
                continue
            parsed_path = _parse_module_path(body[close + 2 :])
            if parsed_path is None:
                malformed = True
                continue
            selections: list[tuple[str, str]] = []
            valid = True
            parts = split_top(
                body[1:close],
                ",",
                angles=False,
            )
            if parts and not parts[-1]:
                parts = parts[:-1]
            if any(not part for part in parts):
                malformed = True
                continue
            for part in parts:
                if (
                    len(part) == 1
                    and _is_core_import_identifier(part[0])
                ):
                    selections.append((part[0].text, part[0].text))
                elif (
                    len(part) == 3
                    and _is_core_import_identifier(part[0])
                    and part[1].text == "as"
                    and _is_core_import_identifier(part[2])
                ):
                    selections.append((part[0].text, part[2].text))
                else:
                    valid = False
                    break
            if not valid or not selections:
                malformed = True
                continue
            if (
                len({source for source, _ in selections})
                != len(selections)
                or len({local for _, local in selections})
                != len(selections)
            ):
                malformed = True
                continue
            external, path = parsed_path
            specs.append(
                _ImportSpec(
                    "selective",
                    external,
                    path,
                    tuple(selections),
                )
            )
            continue

        parsed_path = _parse_module_path(body)
        if parsed_path is None:
            malformed = True
            continue
        external, path = parsed_path
        specs.append(_ImportSpec("open", external, path))
    return specs, malformed


_EXPORT_OPERATOR_PARTS = frozenset(
    {
        ":=",
        "->",
        "=>",
        "==",
        "!=",
        ">=",
        "<=",
        "&&",
        "||",
        "+=",
        "-=",
        "^=",
        "&=",
        "|=",
        "%=",
        "+",
        "-",
        "*",
        "/",
        "%",
        "!",
        "<",
        "<<",
        "<<=",
        ">",
        "=",
        "|",
        "&",
        "^",
        ":",
        "**",
    }
)


def _provider_export_comma_parts(
    tokens: Sequence[Token],
    *,
    allow_empty: bool,
    allow_trailing: bool,
) -> list[list[Token]] | None:
    """Split an export list with the parser's exact empty/trailing rules."""

    parts = split_top(tokens, ",", angles=False)
    if len(parts) == 1 and not parts[0]:
        return [] if allow_empty else None
    if allow_trailing and parts and not parts[-1]:
        parts = parts[:-1]
    if not parts and not allow_empty:
        return None
    if any(not part for part in parts):
        return None
    return parts


def _provider_export_path_prefix(
    tokens: Sequence[Token],
) -> tuple[tuple[str, ...], int] | None:
    """Consume the longest non-external dotted identifier prefix."""

    if not tokens or not _is_core_import_identifier(tokens[0]):
        return None
    path = [tokens[0].text]
    cursor = 1
    while (
        cursor + 1 < len(tokens)
        and tokens[cursor].text == "."
        and _is_core_import_identifier(tokens[cursor + 1])
    ):
        path.append(tokens[cursor + 1].text)
        cursor += 2
    return tuple(path), cursor


def _parse_provider_constructor_selector(
    tokens: Sequence[Token],
) -> _ProviderConstructorSelector | None:
    if (
        len(tokens) < 2
        or tokens[0].text != "("
        or matching_index(tokens, 0) != len(tokens) - 1
    ):
        return None
    inner = tokens[1:-1]
    if len(inner) == 1 and inner[0].text == "*":
        return _ProviderConstructorSelector("all")
    parts = _provider_export_comma_parts(
        inner,
        allow_empty=False,
        allow_trailing=False,
    )
    if parts is None or any(
        len(part) != 1 or not _is_core_import_identifier(part[0])
        for part in parts
    ):
        return None
    return _ProviderConstructorSelector(
        "named",
        tuple(part[0].text for part in parts),
    )


def _parse_provider_export_name(
    tokens: Sequence[Token],
) -> _ProviderExportName | None:
    if len(tokens) == 1 and tokens[0].text == "*":
        return _ProviderExportName("wildcard")
    if tokens and tokens[0].text == "(":
        if (
            matching_index(tokens, 0) != len(tokens) - 1
            or len(tokens) == 2
            or any(
                token.text not in _EXPORT_OPERATOR_PARTS
                for token in tokens[1:-1]
            )
        ):
            return None
        return _ProviderExportName(
            "operator",
            "".join(token.text for token in tokens[1:-1]),
        )
    if not tokens or not _is_core_import_identifier(tokens[0]):
        return None
    if len(tokens) == 1:
        return _ProviderExportName("name", tokens[0].text)
    constructors = _parse_provider_constructor_selector(tokens[1:])
    if constructors is None:
        return None
    return _ProviderExportName(
        "name",
        tokens[0].text,
        constructors=constructors,
    )


def _parse_provider_list_module_wildcard(
    tokens: Sequence[Token],
) -> _ProviderExportName | None:
    parsed = _provider_export_path_prefix(tokens)
    if parsed is None:
        return None
    path, cursor = parsed
    if (
        cursor + 2 != len(tokens)
        or tokens[cursor].text != "."
        or tokens[cursor + 1].text != "*"
    ):
        return None
    return _ProviderExportName("module_wildcard", path=path)


def _parse_provider_export_body(
    body: Sequence[Token],
) -> _ProviderExportSpec | None:
    if not body:
        return None
    if body[0].text == "{":
        if matching_index(body, 0) != len(body) - 1:
            return None
        parts = _provider_export_comma_parts(
            body[1:-1],
            allow_empty=True,
            allow_trailing=True,
        )
        if parts is None:
            return None
        names: list[_ProviderExportName] = []
        for part in parts:
            name = (
                _parse_provider_list_module_wildcard(part)
                or _parse_provider_export_name(part)
            )
            if name is None:
                return None
            names.append(name)
        return _ProviderExportSpec("list", names=tuple(names))

    parsed = _provider_export_path_prefix(body)
    if parsed is None:
        return None
    path, cursor = parsed
    tail = body[cursor:]
    if not tail:
        return _ProviderExportSpec("module", path=path)
    if (
        len(tail) == 2
        and tail[0].text == "as"
        and _is_core_import_identifier(tail[1])
    ):
        return _ProviderExportSpec(
            "module_as",
            path=path,
            alias=tail[1].text,
        )
    if (
        len(tail) == 2
        and tail[0].text == "."
        and tail[1].text == "*"
    ):
        return _ProviderExportSpec(
            "items_from",
            path=path,
            names=(_ProviderExportName("wildcard"),),
        )
    if (
        len(tail) >= 3
        and tail[0].text == "."
        and tail[1].text == "{"
        and matching_index(tail, 1) == len(tail) - 1
    ):
        parts = _provider_export_comma_parts(
            tail[2:-1],
            allow_empty=True,
            allow_trailing=True,
        )
        if parts is None:
            return None
        names = []
        for part in parts:
            name = _parse_provider_export_name(part)
            if name is None:
                return None
            names.append(name)
        return _ProviderExportSpec(
            "items_from",
            path=path,
            names=tuple(names),
        )
    return None


def _parse_export_specs(
    source: str,
) -> tuple[list[_ProviderExportSpec], bool]:
    """Parse the compatibility export grammar used by the Core parser."""

    tokens = significant(source)
    specs: list[_ProviderExportSpec] = []
    malformed = False
    for index, region_end in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if token.text != "export":
            if (
                token.text != "pragma"
                and any(
                    item.text == "export"
                    for item in tokens[index + 1 : region_end + 1]
                )
            ):
                malformed = True
            continue
        end = _statement_end(tokens, index)
        if end is None:
            malformed = True
            continue
        spec = _parse_provider_export_body(tokens[index + 1 : end])
        if spec is None:
            malformed = True
            continue
        specs.append(spec)
    return specs, malformed


def _has_unbalanced_structural_delimiters(
    tokens: Sequence[Token],
) -> bool:
    """Recognize provider-wide parse damage without guessing at angle roles."""

    stack: list[str] = []
    pairs = {")": "(", "]": "[", "}": "{"}
    for token in tokens:
        if token.text in {"(", "[", "{"}:
            stack.append(token.text)
        elif token.text in pairs:
            if not stack or stack[-1] != pairs[token.text]:
                return True
            stack.pop()
    return bool(stack)


def _surface_import_source(source: str) -> tuple[str, bool]:
    """Canonicalize imports without letting discovery preempt migration."""

    try:
        return migrate_imports(source), False
    except ValueError:
        # The migration pass will later report the precise source-local error
        # (or honor a keep-legacy marker).  Constructor discovery must merely
        # avoid deriving cross-file facts from the malformed surface.
        return source, True


def _provider_scan_source(source: str) -> tuple[str, bool]:
    """Canonicalize item heads before validating a provider's structure."""

    if has_comment_marker(source, KEEP_LEGACY_NEGATIVE_MARKER):
        return source, True
    passes = (
        migrate_pragmas,
        migrate_contract_type_parameters,
        migrate_data_declarations,
        migrate_incomplete_data_heads,
        migrate_aliases,
        migrate_value_type_underlying_types,
        migrate_classes,
        migrate_instances,
        migrate_canonical_trait_impl_headers,
        migrate_functions,
        migrate_special_functions,
    )
    try:
        for _ in range(4):
            before = source
            for migration in passes:
                source = migration(source)
            if source == before:
                return source, False
    except ValueError:
        return source, True
    return source, True


def _provider_list_parts(
    tokens: Sequence[Token],
    separator: str,
    *,
    angles: bool = True,
) -> list[list[Token]] | None:
    """Split a parser list while allowing only one trailing separator."""

    parts = split_top(tokens, separator, angles=angles)
    if parts and not parts[-1]:
        parts = parts[:-1]
    if any(not part for part in parts):
        return None
    return parts


def _provider_generic_end(
    tokens: Sequence[Token],
    cursor: int,
) -> int | None:
    if cursor >= len(tokens) or tokens[cursor].text != "<":
        return cursor
    close = matching_index(tokens, cursor)
    if close is None:
        return None
    binders = _provider_list_parts(tokens[cursor + 1 : close], ",")
    if (
        not binders
        or any(
            len(binder) != 1
            or not _is_core_import_identifier(binder[0])
            for binder in binders
        )
    ):
        return None
    return close + 1


def _provider_named_type_is_valid(tokens: Sequence[Token]) -> bool:
    """Validate the canonical named-type subset used by the Core parser."""

    if not tokens or not _is_core_import_identifier(tokens[0]):
        return False
    cursor = 1
    while cursor + 1 < len(tokens) and tokens[cursor].text == ".":
        if not _is_core_import_identifier(tokens[cursor + 1]):
            return False
        cursor += 2
    if cursor == len(tokens):
        return True
    if tokens[cursor].text != "<":
        return False
    close = matching_index(tokens, cursor)
    if close != len(tokens) - 1:
        return False
    arguments = _provider_list_parts(tokens[cursor + 1 : close], ",")
    return arguments is not None and all(
        _provider_canonical_type_is_valid(argument)
        for argument in arguments
    )


def _provider_canonical_type_is_valid(tokens: Sequence[Token]) -> bool:
    """Recognize the parser's canonical type grammar without diagnostics."""

    tokens = _expand_type_angle_closers(
        _split_type_angle_operator_tokens(tokens)
    )
    if not tokens:
        return False

    if tokens[0].text == "comptime":
        return _provider_canonical_type_is_valid(tokens[1:])
    if tokens[0].text == "@":
        return (
            len(tokens) > 1
            and tokens[1].text != "comptime"
            and _provider_canonical_type_is_valid(tokens[1:])
        )

    base = list(tokens)
    if (
        len(base) > 1
        and base[-1].text in LOCATIONS
    ):
        base.pop()
    while base and base[-1].text == "]":
        open_index = next(
            (
                index
                for index in range(len(base) - 2, -1, -1)
                if base[index].text == "["
                and matching_index(base, index) == len(base) - 1
            ),
            None,
        )
        if open_index is None:
            return False
        length = base[open_index + 1 : -1]
        if length:
            if len(length) != 1 or length[0].kind != "number":
                return False
            try:
                radix = (
                    16
                    if length[0].text.lower().startswith("0x")
                    else 10
                )
                value = int(length[0].text, radix)
            except ValueError:
                return False
            if value == 0 or value > (1 << 64) - 1:
                return False
        base = base[:open_index]
    if not base:
        return False

    if is_wrapped(base, "(", ")"):
        elements = _provider_list_parts(base[1:-1], ",")
        return elements is not None and all(
            _provider_canonical_type_is_valid(element)
            for element in elements
        )

    if len(base) >= 2 and base[0].text == "function":
        if base[1].text != "(":
            return False
        close = matching_index(base, 1)
        if close is None:
            return False
        parameters = _provider_list_parts(base[2:close], ",")
        if parameters is None or not all(
            _provider_canonical_type_is_valid(parameter)
            for parameter in parameters
        ):
            return False
        cursor = close + 1
        if (
            cursor < len(base)
            and base[cursor].text in FUNCTION_TYPE_VISIBILITIES
        ):
            cursor += 1
        if (
            cursor < len(base)
            and base[cursor].text in FUNCTION_TYPE_MUTABILITIES
        ):
            cursor += 1
        if cursor < len(base) and base[cursor].text == "returns":
            cursor += 1
            if cursor >= len(base) or base[cursor].text != "(":
                return False
            close = matching_index(base, cursor)
            if close is None:
                return False
            returns = _provider_list_parts(
                base[cursor + 1 : close], ","
            )
            if returns is None or not all(
                _provider_canonical_type_is_valid(result)
                for result in returns
            ):
                return False
            cursor = close + 1
        return cursor == len(base)

    if (
        len(base) >= 3
        and base[0].text == "mapping"
        and base[1].text == "("
        and matching_index(base, 1) == len(base) - 1
    ):
        arguments = split_top(base[2:-1], "=>")
        return (
            len(arguments) == 2
            and all(arguments)
            and all(
                _provider_canonical_type_is_valid(argument)
                for argument in arguments
            )
        )

    return _provider_named_type_is_valid(base)


def _provider_trait_ref_is_valid(
    tokens: Sequence[Token],
    *,
    require_arguments: bool = False,
) -> bool:
    """Validate the unqualified trait references accepted by the parser."""

    tokens = _expand_type_angle_closers(
        _split_type_angle_operator_tokens(tokens)
    )
    if not tokens or not _is_core_import_identifier(tokens[0]):
        return False
    if len(tokens) == 1:
        return not require_arguments
    if tokens[1].text != "<":
        return False
    close = matching_index(tokens, 1)
    if close != len(tokens) - 1:
        return False
    arguments = _provider_list_parts(tokens[2:close], ",")
    return (
        bool(arguments)
        and all(
            _provider_canonical_type_is_valid(argument)
            for argument in arguments
        )
    )


def _provider_type_is_valid(tokens: Sequence[Token]) -> bool:
    if (
        not tokens
        or (
            tokens[0].kind != "word"
            and tokens[0].text not in {"@", "(", "["}
        )
        or any(
            token.text in {"?", "=", ":=", "{", "}", ";"}
            for token in tokens
        )
    ):
        return False
    try:
        rendered = render_type(tokens)
    except ValueError:
        return False
    return bool(rendered) and _provider_canonical_type_is_valid(
        significant(rendered)
    )


def _provider_params_are_valid(tokens: Sequence[Token]) -> bool:
    parts = _provider_list_parts(tokens, ",")
    if parts is None:
        return False
    for part in parts:
        cursor = 0
        if part[cursor].text == "comptime":
            cursor += 1
        if (
            cursor >= len(part)
            or not _is_core_import_identifier(part[cursor])
        ):
            return False
        cursor += 1
        if cursor == len(part):
            continue
        if (
            part[cursor].text != ":"
            or not _provider_type_is_valid(part[cursor + 1 :])
        ):
            return False
    return True


def _provider_returns_are_valid(tokens: Sequence[Token]) -> bool:
    parts = _provider_list_parts(tokens, ",")
    if parts is None:
        return False
    for part in parts:
        colon = find_top(part, ":")
        if colon is None:
            if not _provider_type_is_valid(part):
                return False
        elif (
            colon != 1
            or not _is_core_import_identifier(part[0])
            or not _provider_type_is_valid(part[colon + 1 :])
        ):
            return False
    return True


def _provider_predicates_are_valid(tokens: Sequence[Token]) -> bool:
    parts = _provider_list_parts(tokens, ",")
    if not parts:
        return False
    for part in parts:
        colon = find_top(part, ":")
        if (
            colon is None
            or not _provider_type_is_valid(part[:colon])
            or not _provider_trait_ref_is_valid(part[colon + 1 :])
        ):
            return False
    return True


def _provider_function_header_is_valid(
    header: Sequence[Token],
) -> bool:
    if (
        len(header) < 4
        or header[0].text != "function"
        or not _is_core_import_identifier(header[1])
    ):
        return False
    cursor = _provider_generic_end(header, 2)
    if (
        cursor is None
        or cursor >= len(header)
        or header[cursor].text != "("
    ):
        return False
    close = matching_index(header, cursor)
    if (
        close is None
        or not _provider_params_are_valid(header[cursor + 1 : close])
    ):
        return False
    cursor = close + 1

    modifiers: list[str] = []
    while cursor < len(header) and header[cursor].text in MODIFIERS:
        modifiers.append(header[cursor].text)
        cursor += 1
    visibility = [
        modifier
        for modifier in modifiers
        if modifier in {"public", "external", "internal", "private"}
    ]
    mutability = [
        modifier
        for modifier in modifiers
        if modifier in {"pure", "view", "payable"}
    ]
    if (
        len(visibility) > 1
        or len(mutability) > 1
        or len(set(modifiers)) != len(modifiers)
    ):
        return False

    if cursor < len(header) and header[cursor].text == "returns":
        cursor += 1
        if cursor >= len(header) or header[cursor].text != "(":
            return False
        close = matching_index(header, cursor)
        if (
            close is None
            or not _provider_returns_are_valid(
                header[cursor + 1 : close]
            )
        ):
            return False
        cursor = close + 1

    if cursor < len(header) and header[cursor].text == "where":
        if not _provider_predicates_are_valid(header[cursor + 1 :]):
            return False
        cursor = len(header)
    return cursor == len(header)


def _provider_nominal_header_is_valid(
    kind: str,
    header: Sequence[Token],
) -> bool:
    cursor = 1
    if cursor >= len(header) or not _is_core_import_identifier(
        header[cursor]
    ):
        return False
    has_type_parameters = (
        cursor + 1 < len(header)
        and header[cursor + 1].text == "<"
    )
    cursor = _provider_generic_end(header, cursor + 1)
    if cursor is None:
        return False
    if kind == "trait":
        if not has_type_parameters:
            return False
        if cursor < len(header):
            if header[cursor].text != "where":
                return False
            return _provider_predicates_are_valid(
                header[cursor + 1 :]
            )
    return cursor == len(header)


def _provider_impl_header_is_valid(
    header: Sequence[Token],
) -> bool:
    cursor = 0
    if header and header[0].text == "default":
        cursor += 1
    if cursor >= len(header) or header[cursor].text != "impl":
        return False
    cursor = _provider_generic_end(header, cursor + 1)
    if cursor is None or cursor >= len(header):
        return False
    where = find_top(header[cursor:], "where")
    if where is None:
        head = header[cursor:]
        predicates: Sequence[Token] = ()
    else:
        where += cursor
        head = header[cursor:where]
        predicates = header[where + 1 :]
    valid_head = _provider_trait_ref_is_valid(
        head,
        require_arguments=True,
    )
    if where is None:
        return valid_head
    return valid_head and _provider_predicates_are_valid(predicates)


def _provider_special_modifiers_are_valid(
    tokens: Sequence[Token],
    *,
    visibilities: frozenset[str],
    mutabilities: frozenset[str],
) -> bool:
    if any(token.text not in MODIFIERS for token in tokens):
        return False
    visibility = [
        token.text
        for token in tokens
        if token.text
        in {"public", "external", "internal", "private"}
    ]
    mutability = [
        token.text
        for token in tokens
        if token.text in {"pure", "view", "payable"}
    ]
    return (
        len(visibility) <= 1
        and len(mutability) <= 1
        and len({token.text for token in tokens}) == len(tokens)
        and set(visibility).issubset(visibilities)
        and set(mutability).issubset(mutabilities)
    )


def _provider_constructor_header_is_valid(
    header: Sequence[Token],
) -> bool:
    if len(header) < 3 or header[0].text != "constructor":
        return False
    if header[1].text != "(":
        return False
    close = matching_index(header, 1)
    return (
        close is not None
        and _provider_params_are_valid(header[2:close])
        and _provider_special_modifiers_are_valid(
            header[close + 1 :],
            visibilities=frozenset(),
            mutabilities=frozenset({"payable"}),
        )
    )


def _provider_unit_type_is_valid(tokens: Sequence[Token]) -> bool:
    try:
        rendered = render_type(tokens)
    except ValueError:
        return False
    canonical = significant(rendered)
    if not is_wrapped(canonical, "(", ")"):
        return False
    elements = _provider_list_parts(canonical[1:-1], ",")
    if elements is None:
        return False
    if not elements:
        return True
    return (
        len(elements) == 1
        and _provider_unit_type_is_valid(elements[0])
    )


def _provider_returns_are_unit(tokens: Sequence[Token]) -> bool:
    returns = _provider_list_parts(tokens, ",")
    if returns is None:
        return False
    if not returns:
        return True
    if len(returns) != 1:
        return False
    result = returns[0]
    colon = find_top(result, ":")
    if colon is None:
        result_type = result
    elif (
        colon == 1
        and _is_core_import_identifier(result[0])
    ):
        result_type = result[colon + 1 :]
    else:
        return False
    return _provider_unit_type_is_valid(result_type)


def _provider_fallback_header_is_valid(
    header: Sequence[Token],
) -> bool:
    if (
        len(header) < 3
        or header[0].text != "fallback"
        or header[1].text != "("
    ):
        return False
    close = matching_index(header, 1)
    if close is None or close != 2:
        return False
    cursor = close + 1
    modifier_start = cursor
    while cursor < len(header) and header[cursor].text in MODIFIERS:
        cursor += 1
    if not _provider_special_modifiers_are_valid(
        header[modifier_start:cursor],
        visibilities=frozenset({"external"}),
        mutabilities=frozenset({"payable"}),
    ):
        return False
    if cursor < len(header) and header[cursor].text == "returns":
        cursor += 1
        if cursor >= len(header) or header[cursor].text != "(":
            return False
        close = matching_index(header, cursor)
        if (
            close is None
            or not _provider_returns_are_unit(
                header[cursor + 1 : close]
            )
        ):
            return False
        cursor = close + 1
    return cursor == len(header)


def _provider_function_header_modifiers(
    header: Sequence[Token],
) -> tuple[str, ...] | None:
    if (
        len(header) < 4
        or header[0].text != "function"
        or not _is_core_import_identifier(header[1])
    ):
        return None
    cursor = _provider_generic_end(header, 2)
    if (
        cursor is None
        or cursor >= len(header)
        or header[cursor].text != "("
    ):
        return None
    close = matching_index(header, cursor)
    if close is None:
        return None
    cursor = close + 1
    modifiers: list[str] = []
    while cursor < len(header) and header[cursor].text in MODIFIERS:
        modifiers.append(header[cursor].text)
        cursor += 1
    return tuple(modifiers)


def _provider_member_statement_end(
    tokens: Sequence[Token],
    start: int,
) -> int | None:
    """Find a member semicolon without treating comparisons as generics."""

    stack: list[str] = []
    for index in range(start, len(tokens)):
        if not stack and tokens[index].text == ";":
            return index
        _depth_step(stack, tokens[index].text, angles=False)
    return None


def _provider_function_member(
    tokens: Sequence[Token],
    start: int,
) -> tuple[int, Sequence[Token], bool] | None:
    boundary = _header_boundary(tokens, start + 1)
    if boundary is None:
        return None
    header = tokens[start:boundary]
    if not _provider_function_header_is_valid(header):
        return None
    if tokens[boundary].text == ";":
        return boundary + 1, header, False
    close = matching_index(tokens, boundary)
    if close is None:
        return None
    return close + 1, header, True


def _provider_special_function_member(
    tokens: Sequence[Token],
    start: int,
) -> tuple[int, str] | None:
    boundary = _header_boundary(tokens, start + 1)
    if (
        boundary is None
        or tokens[boundary].text != "{"
    ):
        return None
    header = tokens[start:boundary]
    kind = tokens[start].text
    valid = (
        _provider_constructor_header_is_valid(header)
        if kind == "constructor"
        else _provider_fallback_header_is_valid(header)
    )
    close = matching_index(tokens, boundary)
    if not valid or close is None:
        return None
    return close + 1, kind


def _provider_data_member_end(
    tokens: Sequence[Token],
    start: int,
) -> int | None:
    data_body = _provider_data_body(tokens, start)
    if data_body is None:
        return None
    _, body_open, body_close = data_body
    kind = tokens[start].text
    if not _provider_nominal_header_is_valid(
        kind,
        tokens[start:body_open],
    ):
        return None
    if kind == "struct":
        valid_body = _provider_struct_fields_are_valid(
            tokens[body_open + 1 : body_close]
        )
    else:
        valid_body = (
            _provider_enum_constructors(
                tokens[body_open + 1 : body_close]
            )
            is not None
        )
    if not valid_body:
        return None
    end = body_close + 1
    if end < len(tokens) and tokens[end].text == ";":
        end += 1
    return end


def _provider_alias_member_end(
    tokens: Sequence[Token],
    start: int,
) -> int | None:
    end = _statement_end(tokens, start)
    if (
        end is None
        or not _provider_statement_is_valid(
            tokens[start].text,
            tokens[start + 1 : end],
        )
    ):
        return None
    return end + 1


_PROVIDER_SIMPLE_BINARY_OPERATORS = frozenset(
    {
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
    }
)


def _provider_string_literal_is_valid(text: str) -> bool:
    if len(text) < 2 or text[0] != '"' or text[-1] != '"':
        return False
    cursor = 1
    while cursor < len(text) - 1:
        if text[cursor] != "\\":
            cursor += 1
            continue
        cursor += 1
        if (
            cursor >= len(text) - 1
            or text[cursor] not in {'n', 't', '"', "\\"}
        ):
            return False
        cursor += 1
    return True


def _provider_simple_expression_is_valid(
    tokens: Sequence[Token],
) -> bool:
    """Validate a conservative, assignment-free Core expression subset."""

    def expression_list_is_valid(
        items: Sequence[Token],
        *,
        allow_trailing: bool,
    ) -> bool:
        if not items:
            return True
        parts = split_top(items, ",", angles=False)
        if parts and not parts[-1]:
            if not allow_trailing:
                return False
            parts = parts[:-1]
        return all(
            part and _provider_simple_expression_is_valid(part)
            for part in parts
        )

    def operand_end(start: int) -> int | None:
        cursor = start
        while cursor < len(tokens) and tokens[cursor].text == "!":
            cursor += 1
        if cursor >= len(tokens):
            return None

        token = tokens[cursor]
        if (
            (
                token.kind == "number"
                and not token.text.startswith("0X")
            )
            or (
                token.kind == "string"
                and _provider_string_literal_is_valid(token.text)
            )
            or token.text in {"true", "false"}
            or _is_core_import_identifier(token)
        ):
            cursor += 1
        elif token.text == "(":
            close = matching_index(tokens, cursor)
            if (
                close is None
                or not expression_list_is_valid(
                    tokens[cursor + 1 : close],
                    allow_trailing=True,
                )
            ):
                return None
            cursor = close + 1
        else:
            return None

        while cursor < len(tokens):
            if tokens[cursor].text == ".":
                if (
                    cursor + 1 >= len(tokens)
                    or not _is_core_import_identifier(
                        tokens[cursor + 1]
                    )
                ):
                    return None
                cursor += 2
                continue
            if tokens[cursor].text == "(":
                close = matching_index(tokens, cursor)
                if (
                    close is None
                    or not expression_list_is_valid(
                        tokens[cursor + 1 : close],
                        allow_trailing=False,
                    )
                ):
                    return None
                cursor = close + 1
                continue
            if tokens[cursor].text == "[":
                close = matching_index(tokens, cursor)
                if (
                    close is None
                    or not _provider_simple_expression_is_valid(
                        tokens[cursor + 1 : close]
                    )
                ):
                    return None
                cursor = close + 1
                continue
            break
        return cursor

    operators: list[str] = []
    cursor = operand_end(0)
    if cursor is None:
        return False
    while cursor < len(tokens):
        if tokens[cursor].text not in _PROVIDER_SIMPLE_BINARY_OPERATORS:
            return False
        operators.append(tokens[cursor].text)
        cursor = operand_end(cursor + 1)
        if cursor is None:
            return False

    logical_segments: list[list[str]] = [[]]
    for operator in operators:
        if operator in {"&&", "||"}:
            logical_segments.append([])
        else:
            logical_segments[-1].append(operator)
    for logical_segment in logical_segments:
        if sum(
            operator in {"==", "!="}
            for operator in logical_segment
        ) > 1:
            return False
        relational_segments: list[list[str]] = [[]]
        for operator in logical_segment:
            if operator in {"==", "!="}:
                relational_segments.append([])
            else:
                relational_segments[-1].append(operator)
        if any(
            sum(
                operator in {"<", ">", "<=", ">="}
                for operator in relational_segment
            )
            > 1
            for relational_segment in relational_segments
        ):
            return False
    return True


def _provider_field_member_end(
    tokens: Sequence[Token],
    start: int,
) -> int | None:
    end = _provider_member_statement_end(tokens, start)
    if end is None:
        return None
    field = tokens[start:end]
    if (
        len(field) < 3
        or not _is_core_import_identifier(field[0])
        or field[1].text != ":"
    ):
        return None
    equals = find_top(field[2:], "=")
    if equals is not None:
        equals += 2
        if (
            equals == len(field) - 1
            or not _provider_simple_expression_is_valid(
                field[equals + 1 :]
            )
        ):
            return None
        type_tokens = field[2:equals]
    else:
        type_tokens = field[2:]
    if not _provider_type_is_valid(type_tokens):
        return None
    return end + 1


def _provider_container_body_is_valid(
    kind: str,
    tokens: Sequence[Token],
) -> bool:
    cursor = 0
    while cursor < len(tokens):
        member_kind = tokens[cursor].text

        if kind == "trait":
            member = _provider_function_member(tokens, cursor)
            if (
                member_kind != "function"
                or member is None
                or member[2]
            ):
                return False
            cursor = member[0]
            continue

        if kind == "impl":
            member = _provider_function_member(tokens, cursor)
            if (
                member_kind != "function"
                or member is None
                or not member[2]
            ):
                return False
            cursor = member[0]
            continue

        if member_kind == "function":
            member = _provider_function_member(tokens, cursor)
            if member is None:
                return False
            end, header, has_body = member
            if kind == "interface":
                modifiers = _provider_function_header_modifiers(header)
                if (
                    has_body
                    or modifiers is None
                    or "external" not in modifiers
                ):
                    return False
            elif not has_body:
                return False
            cursor = end
            continue

        if member_kind in {"constructor", "fallback"}:
            member = _provider_special_function_member(tokens, cursor)
            if kind != "contract" or member is None:
                return False
            cursor = member[0]
            continue

        if member_kind in {"alias", "type"}:
            end = _provider_alias_member_end(tokens, cursor)
            if end is None:
                return False
            cursor = end
            continue

        if member_kind in {"enum", "struct"}:
            end = _provider_data_member_end(tokens, cursor)
            if end is None:
                return False
            cursor = end
            continue

        end = _provider_field_member_end(tokens, cursor)
        if kind != "contract" or end is None:
            return False
        cursor = end
    return True


def _provider_statement_is_valid(
    kind: str,
    body: Sequence[Token],
) -> bool:
    if not body:
        return False
    if kind == "pragma":
        if body[0].text in {"solidity", "abicoder"}:
            return True
        if (
            body[0].text != "solcore"
            or len(body) < 2
            or not _is_core_import_identifier(body[1])
        ):
            return False
        items = _provider_list_parts(
            body[2:],
            ",",
            angles=False,
        )
        return items is not None and all(
            len(item) == 1
            and _is_core_import_identifier(item[0])
            for item in items
        )
    if kind == "export" and body[0].text == "{":
        close = matching_index(body, 0)
        if close != len(body) - 1:
            return False
        return _provider_list_parts(
            body[1:close],
            ",",
            angles=False,
        ) is not None
    if kind not in {"alias", "type"}:
        return True
    if not _is_core_import_identifier(body[0]):
        return False
    cursor = _provider_generic_end(body, 1)
    operator = "=" if kind == "alias" else "is"
    return (
        cursor is not None
        and cursor < len(body)
        and body[cursor].text == operator
        and _provider_type_is_valid(body[cursor + 1 :])
    )


def _has_invalid_provider_items(source: str) -> bool:
    """Reject provider facts when canonical top-level items do not parse."""

    canonical, conversion_failed = _provider_scan_source(source)
    if conversion_failed:
        return True
    tokens = significant(canonical)
    brace_pairs, enclosing = _classic_brace_context(tokens)
    statement_items = {"import", "export", "pragma", "alias", "type"}
    nominal_items = {
        "enum",
        "struct",
        "trait",
        "contract",
        "interface",
        "library",
    }
    braced_items = nominal_items | {"impl", "function"}
    cursor = 0
    while cursor < len(tokens):
        if enclosing[cursor] is not None:
            return True
        kind = tokens[cursor].text
        if kind in statement_items:
            end = _provider_statement_end(tokens, cursor)
            if (
                end is None
                or not _provider_statement_is_valid(
                    kind, tokens[cursor + 1 : end]
                )
            ):
                return True
            cursor = end + 1
            continue

        start = cursor
        if kind == "default":
            if (
                cursor + 1 >= len(tokens)
                or tokens[cursor + 1].text != "impl"
            ):
                return True
            kind = "impl"
            cursor += 1
        if kind not in braced_items:
            return True
        boundary = _header_boundary(tokens, cursor + 1)
        if (
            boundary is None
            or tokens[boundary].text != "{"
            or boundary not in brace_pairs
        ):
            return True
        header = tokens[start:boundary]
        if kind == "function":
            valid_header = _provider_function_header_is_valid(header)
        elif kind == "impl":
            valid_header = _provider_impl_header_is_valid(header)
        else:
            valid_header = _provider_nominal_header_is_valid(
                kind, header
            )
        if not valid_header:
            return True
        body_close = brace_pairs[boundary]
        if (
            kind
            in {
                "trait",
                "impl",
                "contract",
                "interface",
                "library",
            }
            and not _provider_container_body_is_valid(
                kind,
                tokens[boundary + 1 : body_close],
            )
        ):
            return True
        cursor = body_close + 1
        if (
            kind in {"enum", "struct"}
            and cursor < len(tokens)
            and tokens[cursor].text == ";"
        ):
            cursor += 1
    return False


def _provider_data_body(
    tokens: Sequence[Token],
    index: int,
) -> tuple[str, int, int] | None:
    """Return a validated data name and body bounds."""

    if (
        index + 1 >= len(tokens)
        or not _is_core_import_identifier(tokens[index + 1])
    ):
        return None
    name = tokens[index + 1].text
    cursor = _provider_generic_end(tokens, index + 2)
    if (
        cursor is None
        or cursor >= len(tokens)
        or tokens[cursor].text != "{"
    ):
        return None
    close = matching_index(tokens, cursor)
    if close is None:
        return None
    return name, cursor, close


def _provider_struct_fields_are_valid(
    tokens: Sequence[Token],
) -> bool:
    if tokens and tokens[-1].text != ";":
        return False
    fields = _provider_list_parts(tokens, ";")
    if fields is None:
        return False
    for field in fields:
        colon = find_top(field, ":")
        if (
            colon != 1
            or not _is_core_import_identifier(field[0])
            or not _provider_type_is_valid(field[colon + 1 :])
        ):
            return False
    return True


def _provider_enum_constructors(
    tokens: Sequence[Token],
) -> frozenset[str] | None:
    constructors = _provider_list_parts(tokens, ",")
    if constructors is None:
        return None
    names: set[str] = set()
    for constructor in constructors:
        if not _is_core_import_identifier(constructor[0]):
            return None
        name = constructor[0].text
        if len(constructor) == 1:
            pass
        elif constructor[1].text == "(":
            close = matching_index(constructor, 1)
            if close != len(constructor) - 1:
                return None
            fields = _provider_list_parts(constructor[2:close], ",")
            if fields is None or any(
                not _provider_type_is_valid(field) for field in fields
            ):
                return None
        else:
            return None
        if name in names:
            return None
        names.add(name)
    return frozenset(names)


def _provider_local_declarations(
    source: str,
    provider: Path,
) -> tuple[
    dict[str, list[_ExportedDataType]],
    dict[str, list[ConstructorOrigin]],
    dict[str, list[ConstructorOrigin]],
    set[str],
    set[str],
    dict[str, list[str]],
]:
    """Collect direct module items without treating contract members as exports."""

    canonical = migrate_incomplete_data_heads(
        migrate_data_declarations(source)
    )
    tokens = significant(canonical)
    data_types: dict[str, list[_ExportedDataType]] = {}
    type_origins: dict[str, list[ConstructorOrigin]] = {}
    term_origins: dict[str, list[ConstructorOrigin]] = {}
    public_names: set[str] = set()
    malformed_data: set[str] = set()
    type_families: dict[str, list[str]] = {}

    named_items = {
        "alias",
        "class",
        "contract",
        "enum",
        "interface",
        "library",
        "struct",
        "trait",
        "type",
    }
    for index, _ in _provider_top_level_item_regions(tokens):
        token = tokens[index]
        if (
            token.text == "function"
            and index + 1 < len(tokens)
            and tokens[index + 1].kind == "word"
        ):
            name = tokens[index + 1].text
            term_origins.setdefault(name, []).append(
                ConstructorOrigin(
                    str(_absolute_lexical_path(provider)),
                    name,
                    tokens[index + 1].start,
                )
            )
            public_names.add(name)
            continue
        if (
            token.text not in named_items
            or index + 1 >= len(tokens)
            or tokens[index + 1].kind != "word"
        ):
            continue
        name = tokens[index + 1].text
        public_names.add(name)
        origin = ConstructorOrigin(
            str(_absolute_lexical_path(provider)),
            name,
            token.start,
        )
        type_origins.setdefault(name, []).append(origin)
        if token.text in {"enum", "struct"}:
            family = "adt"
        elif token.text in {"contract", "interface", "library"}:
            family = "contract"
        elif token.text in {"class", "trait"}:
            family = "class"
        else:
            family = "alias"
        type_families.setdefault(name, []).append(family)
        if token.text not in {"enum", "struct"}:
            continue

        data_body = _provider_data_body(tokens, index)
        if data_body is None:
            malformed_data.add(name)
            continue
        _, body_open, body_close = data_body
        if token.text == "struct":
            if not _provider_struct_fields_are_valid(
                tokens[body_open + 1 : body_close]
            ):
                malformed_data.add(name)
                continue
            constructors = frozenset({name})
        else:
            parsed_constructors = _provider_enum_constructors(
                tokens[body_open + 1 : body_close]
            )
            if parsed_constructors is None:
                malformed_data.add(name)
                continue
            constructors = parsed_constructors

        data_types.setdefault(name, []).append(
            _ExportedDataType(origin, name, constructors)
        )
    return (
        data_types,
        type_origins,
        term_origins,
        public_names,
        malformed_data,
        type_families,
    )


def _empty_provider_interface() -> _ProviderInterface:
    return _ProviderInterface(
        {},
        {},
        {},
        frozenset(),
        frozenset(),
        False,
    )


def _provider_interface_from_facts(
    data: Mapping[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ],
    type_origins: Mapping[str, set[ConstructorOrigin]],
    term_origins: Mapping[str, set[ConstructorOrigin]],
    *,
    unknown: bool = False,
) -> _ProviderInterface:
    grouped: dict[str, list[_ExportedDataType]] = {}
    for (
        public_name,
        origin,
        source_name,
    ), constructors in data.items():
        grouped.setdefault(public_name, []).append(
            _ExportedDataType(
                origin,
                source_name,
                frozenset(constructors),
            )
        )
    public_names = (
        set(grouped)
        | set(type_origins)
        | set(term_origins)
    )
    return _ProviderInterface(
        {
            name: tuple(
                sorted(
                    items,
                    key=lambda item: (
                        item.origin,
                        item.source_name,
                    ),
                )
            )
            for name, items in grouped.items()
        },
        {
            name: tuple(sorted(origins))
            for name, origins in type_origins.items()
        },
        {
            name: tuple(sorted(origins))
            for name, origins in term_origins.items()
        },
        frozenset(term_origins),
        frozenset(public_names),
        unknown,
    )


def _provider_add_data_fact(
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ],
    type_origins: dict[str, set[ConstructorOrigin]],
    public_name: str,
    data_type: _ExportedDataType,
    constructors: Iterable[str],
) -> None:
    data.setdefault(
        (
            public_name,
            data_type.origin,
            data_type.source_name,
        ),
        set(),
    ).update(constructors)
    type_origins.setdefault(public_name, set()).add(
        data_type.origin
    )


def _provider_selected_constructors(
    data_type: _ExportedDataType,
    selector: _ProviderConstructorSelector,
) -> frozenset[str]:
    if selector.kind == "all":
        return data_type.constructors
    return frozenset(selector.names) & data_type.constructors


def _provider_copy_interface_name(
    interface: _ProviderInterface,
    source_name: str,
    public_name: str,
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ],
    type_origins: dict[str, set[ConstructorOrigin]],
    term_origins: dict[str, set[ConstructorOrigin]],
    *,
    opaque: bool = False,
    selector: _ProviderConstructorSelector | None = None,
) -> bool:
    """Copy one public item, preserving its ultimate declaration origin."""

    if selector is not None:
        matches = interface.data_types.get(source_name, ())
        for data_type in matches:
            _provider_add_data_fact(
                data,
                type_origins,
                public_name,
                data_type,
                _provider_selected_constructors(
                    data_type,
                    selector,
                ),
            )
        return bool(matches)

    for origin in interface.type_origins.get(source_name, ()):
        type_origins.setdefault(public_name, set()).add(origin)
    for origin in interface.term_origins.get(source_name, ()):
        term_origins.setdefault(public_name, set()).add(origin)
    for data_type in interface.data_types.get(source_name, ()):
        _provider_add_data_fact(
            data,
            type_origins,
            public_name,
            data_type,
            () if opaque else data_type.constructors,
        )
    return source_name in interface.public_names


def _provider_copy_whole_interface(
    interface: _ProviderInterface,
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ],
    type_origins: dict[str, set[ConstructorOrigin]],
    term_origins: dict[str, set[ConstructorOrigin]],
) -> None:
    for name in interface.public_names:
        _provider_copy_interface_name(
            interface,
            name,
            name,
            data,
            type_origins,
            term_origins,
        )


def _provider_add_local_name(
    plan: _ProviderPlan,
    name: str,
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ],
    type_origins: dict[str, set[ConstructorOrigin]],
    term_origins: dict[str, set[ConstructorOrigin]],
) -> bool:
    for origin in plan.local_type_origins.get(name, ()):
        type_origins.setdefault(name, set()).add(origin)
    for origin in plan.local_term_origins.get(name, ()):
        term_origins.setdefault(name, set()).add(origin)
    for data_type in plan.local_data.get(name, ()):
        _provider_add_data_fact(
            data,
            type_origins,
            name,
            data_type,
            (),
        )
    return name in plan.local_public_names


def _provider_add_all_locals(
    plan: _ProviderPlan,
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ],
    type_origins: dict[str, set[ConstructorOrigin]],
    term_origins: dict[str, set[ConstructorOrigin]],
) -> None:
    for name in plan.local_public_names:
        _provider_add_local_name(
            plan,
            name,
            data,
            type_origins,
            term_origins,
        )


def _provider_plan(source: str, provider: Path) -> _ProviderPlan:
    canonical, import_conversion_failed = _surface_import_source(source)
    (
        local_data,
        local_type_origins,
        local_term_origins,
        local_public_names,
        malformed_data,
        local_type_families,
    ) = _provider_local_declarations(canonical, provider)
    imports, malformed_imports = _parse_import_specs(canonical)
    exports, malformed_exports = _parse_export_specs(canonical)
    duplicate_local_item = (
        any(
            not (
                len(families) == 2
                and set(families) == {"adt", "contract"}
            )
            for families in local_type_families.values()
            if len(families) > 1
        )
        or any(
            len(origins) > 1
            for origins in local_term_origins.values()
        )
    )
    return _ProviderPlan(
        {
            name: tuple(items)
            for name, items in local_data.items()
        },
        {
            name: tuple(origins)
            for name, origins in local_type_origins.items()
        },
        {
            name: tuple(origins)
            for name, origins in local_term_origins.items()
        },
        frozenset(local_public_names),
        tuple(imports),
        tuple(exports),
        (
            import_conversion_failed
            or malformed_imports
            or malformed_exports
            or _has_core_lex_errors(canonical)
            or _has_unbalanced_structural_delimiters(
                _provider_structural_tokens(
                    significant(canonical)
                )
            )
            or _has_invalid_provider_items(canonical)
            or bool(malformed_data)
            or duplicate_local_item
        ),
    )


def _provider_locally_exported_import_names(
    plan: _ProviderPlan,
) -> frozenset[str]:
    """Names whose local-list export can depend on selected imports."""

    names: set[str] = set()
    for export in plan.exports:
        if export.kind != "list":
            continue
        for name in export.names:
            if name.kind not in {"name", "operator"}:
                continue
            if (
                name.constructors is not None
                and plan.local_data.get(name.name)
            ):
                continue
            names.add(name.name)
    return frozenset(names)


def _provider_import_is_relevant(
    plan: _ProviderPlan,
    spec: _ImportSpec,
) -> bool:
    names = _provider_locally_exported_import_names(plan)
    if not names or spec.kind == "namespace":
        return False
    if spec.kind == "open":
        return True
    return any(
        local_name in names
        for _, local_name in spec.selections
    )


def _provider_imported_interface(
    provider: Path,
    plan: _ProviderPlan,
    selected: Mapping[Path, tuple[Path, ...]],
    interfaces: Mapping[Path, _ProviderInterface],
) -> _ProviderInterface:
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ] = {}
    type_origins: dict[str, set[ConstructorOrigin]] = {}
    term_origins: dict[str, set[ConstructorOrigin]] = {}
    for spec in plan.imports:
        if not _provider_import_is_relevant(plan, spec):
            continue
        target = _resolve_selected_import(
            provider,
            spec,
            selected,
        )
        if target is None:
            continue
        interface = interfaces[target]
        if spec.kind == "open":
            _provider_copy_whole_interface(
                interface,
                data,
                type_origins,
                term_origins,
            )
            continue
        for source_name, local_name in spec.selections:
            _provider_copy_interface_name(
                interface,
                source_name,
                local_name,
                data,
                type_origins,
                term_origins,
            )
    return _provider_interface_from_facts(
        data,
        type_origins,
        term_origins,
    )


def _evaluate_provider_facts(
    provider: Path,
    plan: _ProviderPlan,
    selected: Mapping[Path, tuple[Path, ...]],
    interfaces: Mapping[Path, _ProviderInterface],
) -> _ProviderInterface:
    """Evaluate one lenient monotone step without missing-name diagnostics."""

    imported = _provider_imported_interface(
        provider,
        plan,
        selected,
        interfaces,
    )
    data: dict[
        tuple[str, ConstructorOrigin, str],
        set[str],
    ] = {}
    type_origins: dict[str, set[ConstructorOrigin]] = {}
    term_origins: dict[str, set[ConstructorOrigin]] = {}

    def copy_target_name(
        target: _ProviderInterface,
        name: _ProviderExportName,
    ) -> None:
        if name.kind == "wildcard":
            _provider_copy_whole_interface(
                target,
                data,
                type_origins,
                term_origins,
            )
            return
        _provider_copy_interface_name(
            target,
            name.name,
            name.name,
            data,
            type_origins,
            term_origins,
            opaque=name.constructors is None,
            selector=name.constructors,
        )

    for export in plan.exports:
        if export.kind == "list":
            for name in export.names:
                if name.kind == "wildcard":
                    _provider_add_all_locals(
                        plan,
                        data,
                        type_origins,
                        term_origins,
                    )
                    continue
                if name.kind == "module_wildcard":
                    target = _resolve_selected_module_path(
                        provider,
                        name.path,
                        selected,
                    )
                    if target is not None:
                        _provider_copy_whole_interface(
                            interfaces[target],
                            data,
                            type_origins,
                            term_origins,
                        )
                    continue
                if (
                    name.constructors is not None
                    and plan.local_data.get(name.name)
                ):
                    for data_type in plan.local_data[name.name]:
                        _provider_add_data_fact(
                            data,
                            type_origins,
                            name.name,
                            data_type,
                            _provider_selected_constructors(
                                data_type,
                                name.constructors,
                            ),
                        )
                    continue
                if name.constructors is None:
                    _provider_add_local_name(
                        plan,
                        name.name,
                        data,
                        type_origins,
                        term_origins,
                    )
                _provider_copy_interface_name(
                    imported,
                    name.name,
                    name.name,
                    data,
                    type_origins,
                    term_origins,
                    opaque=name.constructors is None,
                    selector=name.constructors,
                )
            continue

        if export.kind != "items_from":
            # Module aliases occupy a distinct namespace.  Until that
            # namespace is represented, strict validation fails closed.
            continue
        target = _resolve_selected_module_path(
            provider,
            export.path,
            selected,
        )
        if target is None:
            continue
        target_interface = interfaces[target]
        for name in export.names:
            copy_target_name(target_interface, name)

    return _provider_interface_from_facts(
        data,
        type_origins,
        term_origins,
    )


def _provider_selector_is_valid(
    data_types: Sequence[_ExportedDataType],
    selector: _ProviderConstructorSelector,
) -> bool:
    if not data_types:
        return False
    origins = {data_type.origin for data_type in data_types}
    if len(origins) != 1:
        return False
    if selector.kind == "all":
        return True
    visible = {
        constructor
        for data_type in data_types
        for constructor in data_type.constructors
    }
    return set(selector.names).issubset(visible)


def _provider_dependencies(
    provider: Path,
    plan: _ProviderPlan,
    selected: Mapping[Path, tuple[Path, ...]],
) -> tuple[frozenset[Path], bool]:
    dependencies: set[Path] = set()
    unresolved = False
    for spec in plan.imports:
        if not _provider_import_is_relevant(plan, spec):
            continue
        target = _resolve_selected_import(provider, spec, selected)
        if target is None:
            unresolved = True
        else:
            dependencies.add(target)
    for export in plan.exports:
        paths: list[tuple[str, ...]] = []
        if export.kind in {"items_from", "module", "module_as"}:
            paths.append(export.path)
        elif export.kind == "list":
            paths.extend(
                name.path
                for name in export.names
                if name.kind == "module_wildcard"
            )
        for path in paths:
            target = _resolve_selected_module_path(
                provider,
                path,
                selected,
            )
            if target is None:
                unresolved = True
            else:
                dependencies.add(target)
    return frozenset(dependencies), unresolved


def _provider_strictly_unknown(
    provider: Path,
    plan: _ProviderPlan,
    selected: Mapping[Path, tuple[Path, ...]],
    interfaces: Mapping[Path, _ProviderInterface],
) -> bool:
    dependencies, unresolved = _provider_dependencies(
        provider,
        plan,
        selected,
    )
    del dependencies
    unknown = plan.direct_unknown or unresolved
    imported = _provider_imported_interface(
        provider,
        plan,
        selected,
        interfaces,
    )

    relevant_names = _provider_locally_exported_import_names(plan)
    for spec in plan.imports:
        if (
            spec.kind != "selective"
            or not _provider_import_is_relevant(plan, spec)
        ):
            continue
        target = _resolve_selected_import(provider, spec, selected)
        if target is None:
            continue
        available = interfaces[target].public_names
        if any(
            local_name in relevant_names
            and source_name not in available
            for source_name, local_name in spec.selections
        ):
            unknown = True

    for export in plan.exports:
        if export.kind in {"module", "module_as"}:
            unknown = True
            continue
        if export.kind == "list":
            for name in export.names:
                if name.kind == "wildcard":
                    continue
                if name.kind == "module_wildcard":
                    if (
                        _resolve_selected_module_path(
                            provider,
                            name.path,
                            selected,
                        )
                        is None
                    ):
                        unknown = True
                    continue
                if name.constructors is None:
                    if (
                        name.name not in plan.local_public_names
                        and name.name not in imported.public_names
                    ):
                        unknown = True
                    continue
                candidates = plan.local_data.get(name.name, ())
                if not candidates:
                    candidates = imported.data_types.get(
                        name.name,
                        (),
                    )
                if not _provider_selector_is_valid(
                    candidates,
                    name.constructors,
                ):
                    unknown = True
            continue

        target = _resolve_selected_module_path(
            provider,
            export.path,
            selected,
        )
        if target is None:
            unknown = True
            continue
        target_interface = interfaces[target]
        for name in export.names:
            if name.kind == "wildcard":
                continue
            if name.constructors is None:
                if name.name not in target_interface.public_names:
                    unknown = True
                continue
            if not _provider_selector_is_valid(
                target_interface.data_types.get(name.name, ()),
                name.constructors,
            ):
                unknown = True

    interface = interfaces[provider]
    if any(
        len({data_type.origin for data_type in data_types}) > 1
        for data_types in interface.data_types.values()
    ):
        unknown = True
    for name in interface.public_names:
        definition_families = {
            (origin.provider, origin.type_name)
            for origin in (
                *interface.type_origins.get(name, ()),
                *interface.term_origins.get(name, ()),
            )
        }
        if len(definition_families) > 1:
            unknown = True
    return unknown


def _compute_provider_interfaces(
    sources: Mapping[Path, str],
    selected: Mapping[Path, tuple[Path, ...]],
) -> tuple[
    dict[Path, _ProviderInterface],
    dict[Path, _ProviderPlan],
]:
    """Compute re-export facts to a least fixed point, then fail closed."""

    plans = {
        provider: _provider_plan(source, provider)
        for provider, source in sources.items()
    }
    interfaces = {
        provider: _empty_provider_interface()
        for provider in sources
    }
    while True:
        changed = False
        next_interfaces: dict[Path, _ProviderInterface] = {}
        for provider, plan in plans.items():
            interface = _evaluate_provider_facts(
                provider,
                plan,
                selected,
                interfaces,
            )
            next_interfaces[provider] = interface
            changed |= interface != interfaces[provider]
        interfaces = next_interfaces
        if not changed:
            break

    dependencies = {
        provider: _provider_dependencies(
            provider,
            plan,
            selected,
        )[0]
        for provider, plan in plans.items()
    }
    unknown = {
        provider: _provider_strictly_unknown(
            provider,
            plan,
            selected,
            interfaces,
        )
        for provider, plan in plans.items()
    }
    while True:
        changed = False
        for provider, targets in dependencies.items():
            if (
                not unknown[provider]
                and any(unknown[target] for target in targets)
            ):
                unknown[provider] = True
                changed = True
        if not changed:
            break

    interfaces = {
        provider: _ProviderInterface(
            interface.data_types,
            interface.type_origins,
            interface.term_origins,
            interface.terms,
            interface.public_names,
            unknown[provider],
        )
        for provider, interface in interfaces.items()
    }
    return interfaces, plans


def _local_qualified_term_winners(
    tokens: Sequence[Token],
) -> dict[str, str]:
    """Track first source-local constructor or promoted library term paths."""

    brace_pairs, enclosing = _classic_brace_context(tokens)
    winners: dict[str, str] = {}
    for index, token in enumerate(tokens):
        if enclosing[index] is not None:
            continue
        if (
            token.text in {"enum", "struct"}
            and index + 1 < len(tokens)
            and tokens[index + 1].kind == "word"
        ):
            body = _provider_data_body(tokens, index)
            if body is None:
                continue
            name, body_open, body_close = body
            if token.text == "struct":
                if not _provider_struct_fields_are_valid(
                    tokens[body_open + 1 : body_close]
                ):
                    continue
                constructors = frozenset({name})
            else:
                parsed = _provider_enum_constructors(
                    tokens[body_open + 1 : body_close]
                )
                if parsed is None:
                    continue
                constructors = parsed
            for constructor in constructors:
                winners.setdefault(
                    f"{name}.{constructor}",
                    "constructor",
                )
            continue
        if token.text != "library":
            continue
        boundary = _header_boundary(tokens, index + 1)
        if (
            index + 1 >= len(tokens)
            or tokens[index + 1].kind != "word"
            or boundary is None
            or tokens[boundary].text != "{"
            or boundary not in brace_pairs
        ):
            continue
        library_name = tokens[index + 1].text
        for cursor in range(boundary + 1, brace_pairs[boundary]):
            if (
                tokens[cursor].text != "function"
                or enclosing[cursor] != boundary
                or cursor + 1 >= len(tokens)
                or tokens[cursor + 1].kind != "word"
            ):
                continue
            function_boundary = _header_boundary(tokens, cursor + 1)
            if function_boundary is None:
                continue
            header = tokens[cursor + 1 : function_boundary]
            if any(item.text == "private" for item in header):
                continue
            winners.setdefault(
                f"{library_name}.{tokens[cursor + 1].text}",
                "term",
            )
    return winners


def _resolve_selected_module_path(
    consumer: Path,
    path: tuple[str, ...],
    selected: Mapping[Path, tuple[Path, ...]],
) -> Path | None:
    """Resolve an unambiguous relative module path within selected files."""

    if (
        not path
        or path[0] in {"lib", "std"}
    ):
        return None
    base = _absolute_lexical_path(consumer).parent.joinpath(*path)
    matches: list[Path] = []
    for suffix in (".solc", ".sol"):
        matches.extend(selected.get(base.with_suffix(suffix), ()))
    unique = list(dict.fromkeys(matches))
    return unique[0] if len(unique) == 1 else None


def _resolve_selected_import(
    consumer: Path,
    spec: _ImportSpec,
    selected: Mapping[Path, tuple[Path, ...]],
) -> Path | None:
    """Resolve only unambiguous ordinary imports within selected paths."""

    if spec.external:
        return None
    return _resolve_selected_module_path(
        consumer,
        spec.path,
        selected,
    )


def build_constructor_import_surfaces(
    sources: Mapping[Path, str],
) -> dict[Path, ConstructorImportSurface]:
    """Build a constructor surface for each selected source consumer."""

    selected: dict[Path, list[Path]] = {}
    for path in sources:
        selected.setdefault(_absolute_lexical_path(path), []).append(path)
    selected_index = {
        path: tuple(paths) for path, paths in selected.items()
    }
    interfaces, plans = _compute_provider_interfaces(
        sources,
        selected_index,
    )
    surfaces: dict[Path, ConstructorImportSurface] = {}

    for consumer, source in sources.items():
        canonical, import_conversion_failed = _surface_import_source(source)
        specs, malformed = _parse_import_specs(canonical)
        bare: dict[str, set[ConstructorBinding]] = {}
        dot: dict[str, set[ConstructorBinding]] = {}
        owner_claims: dict[str, set[ConstructorOwnerClaim]] = {}
        namespace_qualifier_targets: dict[str, set[str]] = {}
        qualified_namespace_term_targets: dict[str, set[str]] = {}
        qualified_import_term_winners: dict[str, str] = {}
        imported_terms: set[str] = set()
        unknown_terms: set[str] = set()
        unknown_unqualified_terms = malformed or import_conversion_failed
        unknown_unqualified_constructors = (
            malformed or import_conversion_failed
        )
        unknown_constructors = malformed or import_conversion_failed

        def mark_unknown(spec: _ImportSpec) -> None:
            nonlocal unknown_unqualified_terms
            nonlocal unknown_unqualified_constructors
            nonlocal unknown_constructors
            unknown_constructors = True
            if spec.kind == "open":
                unknown_unqualified_terms = True
            if spec.kind in {"open", "selective", "namespace"}:
                unknown_unqualified_constructors = True
            if spec.kind == "selective":
                unknown_terms.update(
                    local for _, local in spec.selections
                )

        def add_data(
            data_type: _ExportedDataType,
            owner: str,
        ) -> None:
            binding = ConstructorBinding(data_type.origin, owner)
            for constructor in data_type.constructors:
                qualified_import_term_winners.setdefault(
                    f"{owner}.{constructor}",
                    "constructor",
                )
                dot.setdefault(constructor, set()).add(binding)
                if (
                    constructor not in BUILTIN_CONSTRUCTORS
                    and not (
                        constructor in BUILTIN_TYPE_NAMES
                        and constructor != data_type.source_name
                    )
                ):
                    bare.setdefault(constructor, set()).add(binding)

        def add_owner_claims(
            origins: Iterable[ConstructorOrigin],
            owner: str,
            visible_through: str,
            *,
            local: bool = False,
        ) -> None:
            origins = tuple(origins)
            if not origins:
                return
            claims = owner_claims.setdefault(owner, set())
            claims.update(
                ConstructorOwnerClaim(
                    visible_through,
                    origin,
                    local,
                )
                for origin in origins
            )

        local_type_origins = plans[consumer].local_type_origins
        consumer_surface = str(_absolute_lexical_path(consumer))
        for name, origins in local_type_origins.items():
            add_owner_claims(
                origins,
                name,
                consumer_surface,
                local=True,
            )

        for spec in specs:
            provider = _resolve_selected_import(
                consumer,
                spec,
                selected_index,
            )
            if provider is None:
                mark_unknown(spec)
                continue
            interface = interfaces[provider]
            if interface.unknown:
                mark_unknown(spec)
                continue
            import_target = str(_absolute_lexical_path(provider))

            if spec.kind == "open":
                imported_terms.update(interface.terms)
                for public_name, origins in (
                    interface.type_origins.items()
                ):
                    add_owner_claims(
                        origins,
                        public_name,
                        import_target,
                    )
                for public_name, data_types in interface.data_types.items():
                    for data_type in data_types:
                        add_data(data_type, public_name)
                continue

            if spec.kind == "namespace":
                assert spec.qualifier is not None
                namespace_qualifier_targets.setdefault(
                    spec.qualifier,
                    set(),
                ).add(import_target)
                for term in interface.terms:
                    qualified_term = f"{spec.qualifier}.{term}"
                    qualified_namespace_term_targets.setdefault(
                        qualified_term,
                        set(),
                    ).add(import_target)
                    qualified_import_term_winners.setdefault(
                        qualified_term,
                        "namespace",
                    )
                for public_name, origins in (
                    interface.type_origins.items()
                ):
                    add_owner_claims(
                        origins,
                        f"{spec.qualifier}.{public_name}",
                        import_target,
                    )
                for public_name, data_types in interface.data_types.items():
                    owner = f"{spec.qualifier}.{public_name}"
                    for data_type in data_types:
                        add_data(data_type, owner)
                continue

            for source_name, local_name in spec.selections:
                matched = source_name in interface.public_names
                if source_name in interface.terms:
                    imported_terms.add(local_name)
                add_owner_claims(
                    interface.type_origins.get(source_name, ()),
                    local_name,
                    import_target,
                )
                for data_type in interface.data_types.get(
                    source_name, ()
                ):
                    add_data(data_type, local_name)
                if not matched:
                    # The compiler records a selected name missing from a
                    # complete interface as unknown in every namespace.
                    unknown_terms.add(local_name)

        surfaces[consumer] = ConstructorImportSurface(
            bare_candidates={
                leaf: frozenset(bindings)
                for leaf, bindings in bare.items()
            },
            dot_candidates={
                leaf: frozenset(bindings)
                for leaf, bindings in dot.items()
            },
            owner_claims={
                owner: frozenset(claims)
                for owner, claims in owner_claims.items()
            },
            namespace_qualifier_targets={
                qualifier: frozenset(targets)
                for qualifier, targets in namespace_qualifier_targets.items()
            },
            qualified_namespace_term_targets={
                term: frozenset(targets)
                for term, targets in (
                    qualified_namespace_term_targets.items()
                )
            },
            qualified_import_term_winners=dict(
                qualified_import_term_winners
            ),
            imported_terms=frozenset(imported_terms),
            unknown_imported_terms=frozenset(unknown_terms),
            has_unknown_unqualified_terms=unknown_unqualified_terms,
            has_unknown_unqualified_constructors=(
                unknown_unqualified_constructors
            ),
            has_unknown_constructors=unknown_constructors,
        )
    return surfaces


def _deduplicate_lexical_paths(paths: Iterable[Path]) -> list[Path]:
    """Keep the first spelling of each absolute lexical source path."""

    unique: dict[Path, Path] = {}
    for path in paths:
        unique.setdefault(_absolute_lexical_path(path), path)
    return list(unique.values())


def _directory_source_paths(
    root: Path,
    suffixes: frozenset[str],
) -> list[Path]:
    """Walk a directory without hiding traversal errors or special files."""

    files: list[Path] = []
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            with os.scandir(directory) as iterator:
                entries = sorted(iterator, key=lambda entry: entry.name)
        except OSError as error:
            raise ValueError(
                f"cannot traverse source directory {directory}: {error}"
            ) from error

        child_directories: list[Path] = []
        for entry in entries:
            path = Path(entry.path)
            try:
                if entry.is_dir(follow_symlinks=False):
                    child_directories.append(path)
                elif (
                    path.suffix in suffixes
                    and entry.is_file(follow_symlinks=True)
                ):
                    files.append(path)
            except OSError as error:
                raise ValueError(
                    f"cannot inspect source path {path}: {error}"
                ) from error
        pending.extend(reversed(child_directories))
    return files


def _argument_path_kind(path: Path) -> str:
    try:
        mode = path.stat().st_mode
    except OSError as error:
        raise ValueError(f"cannot inspect source path {path}: {error}") from error
    if stat.S_ISDIR(mode):
        return "directory"
    if stat.S_ISREG(mode):
        return "file"
    return "special"


def source_paths(arguments: Sequence[str]) -> list[Path]:
    paths: list[Path] = []
    for argument in arguments:
        path = Path(argument)
        kind = _argument_path_kind(path)
        if kind == "directory":
            paths.extend(
                _directory_source_paths(
                    path, frozenset({".sol", ".solc"})
                )
            )
        elif kind == "file" and path.suffix in {".sol", ".solc"}:
            paths.append(path)
        else:
            raise ValueError(f"not a Solcore source path: {path}")
    return _deduplicate_lexical_paths(paths)


def rust_source_paths(arguments: Sequence[str]) -> list[Path]:
    paths: list[Path] = []
    for argument in arguments:
        path = Path(argument)
        kind = _argument_path_kind(path)
        if kind == "directory":
            paths.extend(
                _directory_source_paths(path, frozenset({".rs"}))
            )
        elif kind == "file" and path.suffix == ".rs":
            paths.append(path)
        elif (
            kind == "file"
            and path.suffix in {".sol", ".solc"}
        ):
            # Retain the legacy CLI acceptance, but this mode edits only Rust
            # files and builds no cross-file constructor table.
            continue
        else:
            raise ValueError(f"not a Rust source path: {path}")
    return _deduplicate_lexical_paths(paths)


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


def _rust_file_code_start(source: str) -> int:
    """Skip Rust's optional BOM and non-attribute shebang."""

    cursor = 1 if source.startswith("\ufeff") else 0
    if source.startswith("#!", cursor) and not source.startswith(
        "#![",
        cursor,
    ):
        newline = source.find("\n", cursor + 2)
        return len(source) if newline < 0 else newline + 1
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


def _rust_identifier_continues(character: str) -> bool:
    return character == "_" or ("a" + character).isidentifier()


def _rust_raw_literal(
    source: str, start: int
) -> tuple[int, int, int, bool] | None:
    """Return body bounds, literal end, and lexical validity."""

    if start and _rust_identifier_continues(source[start - 1]):
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
        return body_start, len(source), len(source), False
    return (
        body_start,
        body_end,
        body_end + len(terminator),
        len(hashes) <= 255,
    )


def _rust_ordinary_literal(
    source: str, start: int
) -> tuple[int, int, int, bool] | None:
    """Return body bounds, literal end, and lexical validity."""

    if start >= len(source):
        return None
    if source[start] == '"':
        quote = start
    elif source.startswith(("b\"", "c\""), start):
        if (
            start
            and _rust_identifier_continues(source[start - 1])
        ):
            return None
        quote = start + 1
    else:
        return None
    literal_end = _scan_quoted(source, quote, '"')
    quote_index = literal_end - 1
    backslashes = 0
    cursor = quote_index - 1
    while cursor > quote and source[cursor] == "\\":
        backslashes += 1
        cursor -= 1
    terminated = (
        quote_index > quote
        and source[quote_index] == '"'
        and backslashes % 2 == 0
    )
    body_end = quote_index if terminated else len(source)
    return quote + 1, body_end, literal_end, terminated


def _decode_rust_ordinary_body(
    body: str,
    literal_kind: str = "unicode",
) -> str | None:
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
            if body.startswith("\r\n", cursor):
                decoded.append("\n")
                cursor += 2
                continue
            if body[cursor] == "\r":
                return None
            if (
                literal_kind == "byte"
                and ord(body[cursor]) > 0x7F
            ):
                return None
            if literal_kind == "c" and body[cursor] == "\0":
                return None
            decoded.append(body[cursor])
            cursor += 1
            continue
        if cursor + 1 >= len(body):
            return None

        escaped = body[cursor + 1]
        if escaped in simple_escapes:
            value = simple_escapes[escaped]
            if literal_kind == "c" and value == "\0":
                return None
            decoded.append(value)
            cursor += 2
            continue
        if escaped == "x" and cursor + 3 < len(body):
            digits = body[cursor + 2 : cursor + 4]
            if all(digit in "0123456789abcdefABCDEF" for digit in digits):
                value = int(digits, 16)
                if value > 0x7F:
                    return None
                if literal_kind == "c" and value == 0:
                    return None
                decoded.append(chr(value))
                cursor += 4
                continue
        if escaped == "u" and cursor + 2 < len(body) and body[cursor + 2] == "{":
            if literal_kind == "byte":
                return None
            close = body.find("}", cursor + 3)
            digits = body[cursor + 3 : close] if close >= 0 else ""
            normalized = digits.replace("_", "")
            if (
                digits
                and digits[0] != "_"
                and normalized
                and len(normalized) <= 6
                and all(
                    digit in "0123456789abcdefABCDEF"
                    for digit in normalized
                )
            ):
                value = int(normalized, 16)
                if value <= 0x10FFFF and not 0xD800 <= value <= 0xDFFF:
                    if literal_kind == "c" and value == 0:
                        return None
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
            while (
                cursor < len(body)
                and body[cursor] in {" ", "\t", "\n", "\r"}
            ):
                cursor += 1
            continue

        return None
    return "".join(decoded)


def _encode_rust_ordinary_body(
    body: str,
    literal_kind: str = "unicode",
) -> str:
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
        elif (
            literal_kind == "byte"
            and (ord(char) < 0x20 or ord(char) == 0x7F)
        ):
            encoded.append(f"\\x{ord(char):02x}")
        elif ord(char) < 0x20 or ord(char) == 0x7F:
            encoded.append(f"\\u{{{ord(char):x}}}")
        else:
            encoded.append(char)
    return "".join(encoded)


def _rust_literal_semantic_body(
    body: str,
    is_raw: bool,
    literal_kind: str = "unicode",
) -> str | None:
    """Return one literal body's semantic text for every migration phase."""

    if not is_raw:
        return _decode_rust_ordinary_body(body, literal_kind)
    normalized = body.replace("\r\n", "\n")
    if "\r" in normalized:
        return None
    if (
        literal_kind == "byte"
        and any(ord(character) > 0x7F for character in normalized)
    ):
        return None
    if literal_kind == "c" and "\0" in normalized:
        return None
    return normalized


def _rust_literal_kind(source: str, start: int) -> str:
    if source.startswith(("br", 'b"'), start):
        return "byte"
    if source.startswith(("cr", 'c"'), start):
        return "c"
    return "unicode"


@dataclass(frozen=True)
class RustStringLiteral:
    literal_start: int
    body_start: int
    body_end: int
    literal_end: int
    is_raw: bool


@dataclass(frozen=True)
class RustConcatInvocation:
    start: int
    end: int
    literals: tuple[RustStringLiteral, ...] | None


def _rust_skip_trivia(source: str, start: int) -> int:
    cursor = start
    while cursor < len(source):
        if source[cursor] in RUST_PATTERN_WHITESPACE:
            cursor += 1
            continue
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", cursor):
            cursor = _rust_block_comment_end(source, cursor)
            continue
        break
    return cursor


def _rust_token_tree_end(source: str, open_start: int) -> int | None:
    """Return the end of one balanced Rust macro token tree."""

    closing = {"(": ")", "[": "]", "{": "}"}
    opener = source[open_start] if open_start < len(source) else ""
    if opener not in closing:
        return None

    stack = [closing[opener]]
    cursor = open_start + 1
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
        if literal is None:
            literal = _rust_ordinary_literal(source, cursor)
        if literal is not None:
            cursor = literal[2]
            continue

        token = source[cursor]
        if token in closing:
            stack.append(closing[token])
            cursor += 1
            continue
        if token in closing.values():
            if token != stack[-1]:
                return None
            stack.pop()
            cursor += 1
            if not stack:
                return cursor
            continue
        cursor += 1
    return None


def _rust_unicode_string_literal(
    source: str, start: int
) -> RustStringLiteral | None:
    """Recognize a direct Unicode string literal accepted by ``concat!``."""

    raw = _rust_raw_literal(source, start)
    if raw is not None and source[start] == "r":
        body_start, body_end, literal_end, valid = raw
        if not valid:
            return None
        return RustStringLiteral(
            start,
            body_start,
            body_end,
            literal_end,
            True,
        )

    if start >= len(source) or source[start] != '"':
        return None
    ordinary = _rust_ordinary_literal(source, start)
    if ordinary is None:
        return None
    body_start, body_end, literal_end, valid = ordinary
    if not valid:
        return None
    return RustStringLiteral(
        start,
        body_start,
        body_end,
        literal_end,
        False,
    )


def _rust_identifier_token_end(source: str, start: int) -> int | None:
    """Return the end of one Rust identifier, including a raw identifier."""

    cursor = start
    if source.startswith("r#", cursor):
        cursor += 2
    if (
        cursor >= len(source)
        or not (
            source[cursor] == "_"
            or source[cursor].isidentifier()
        )
    ):
        return None
    cursor += 1
    while (
        cursor < len(source)
        and _rust_identifier_continues(source[cursor])
    ):
        cursor += 1
    return cursor


def _rust_concat_invocation_at(
    source: str,
    start: int,
    preceding_code: str,
) -> RustConcatInvocation | None:
    """Parse a bare ``concat!`` and its direct string-literal operands."""

    cursor = _rust_skip_trivia(source, start + len("concat"))
    if cursor >= len(source) or source[cursor] != "!":
        return None
    cursor = _rust_skip_trivia(source, cursor + 1)
    if cursor >= len(source) or source[cursor] not in "([{":
        return None

    end = _rust_token_tree_end(source, cursor)
    if end is None:
        return RustConcatInvocation(start, len(source), None)
    close = end - 1

    # Qualified and raw-identifier macro names are not proven to be the
    # built-in concat macro. Protect their contents from literal fallback.
    if preceding_code in {"::", "r#"}:
        return RustConcatInvocation(start, end, None)

    literals: list[RustStringLiteral] = []
    argument = _rust_skip_trivia(source, cursor + 1)
    if argument == close:
        return RustConcatInvocation(start, end, ())

    while argument < close:
        literal = _rust_unicode_string_literal(source, argument)
        if literal is None or literal.literal_end > close:
            return RustConcatInvocation(start, end, None)
        semantic = _rust_literal_semantic_body(
            source[literal.body_start : literal.body_end],
            literal.is_raw,
        )
        if semantic is None:
            return RustConcatInvocation(start, end, None)
        literals.append(literal)

        argument = _rust_skip_trivia(source, literal.literal_end)
        if argument == close:
            break
        if source[argument] != ",":
            return RustConcatInvocation(start, end, None)
        argument = _rust_skip_trivia(source, argument + 1)
        if argument == close:
            break

    return RustConcatInvocation(start, end, tuple(literals))


def _rust_macro_token_tree_ranges(source: str) -> list[tuple[int, int]]:
    """Locate macro token trees so nested ``concat!`` stays opaque."""

    non_path_keywords = {
        "as",
        "async",
        "await",
        "break",
        "const",
        "continue",
        "dyn",
        "else",
        "enum",
        "extern",
        "false",
        "fn",
        "for",
        "if",
        "impl",
        "in",
        "let",
        "loop",
        "match",
        "mod",
        "move",
        "mut",
        "pub",
        "ref",
        "return",
        "static",
        "struct",
        "trait",
        "true",
        "type",
        "unsafe",
        "use",
        "where",
        "while",
    }
    closing = {"(": ")", "[": "]", "{": "}"}
    ranges: list[tuple[int, int]] = []
    macro_openings: dict[int, int] = {}
    delimiter_stack: list[tuple[str, int | None]] = []
    cursor = _rust_file_code_start(source)
    previous_identifier: str | None = None
    while cursor < len(source):
        if source[cursor] in RUST_PATTERN_WHITESPACE:
            cursor += 1
            continue
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
                previous_identifier = None
                cursor = char_end
                continue

        literal = _rust_raw_literal(source, cursor)
        if literal is None:
            literal = _rust_ordinary_literal(source, cursor)
        if literal is not None:
            previous_identifier = None
            cursor = literal[2]
            continue

        identifier_end = _rust_identifier_token_end(source, cursor)
        if identifier_end is not None:
            previous_identifier = source[cursor:identifier_end]
            cursor = identifier_end
            continue

        is_macro_name = (
            previous_identifier is not None
            and (
                previous_identifier.startswith("r#")
                or previous_identifier not in non_path_keywords
            )
        )
        if source[cursor] == "!" and is_macro_name:
            open_start = _rust_skip_trivia(source, cursor + 1)
            if previous_identifier == "macro_rules":
                name_end = _rust_identifier_token_end(
                    source,
                    open_start,
                )
                if name_end is not None:
                    open_start = _rust_skip_trivia(source, name_end)
            if (
                open_start < len(source)
                and source[open_start] in closing
            ):
                macro_openings[open_start] = cursor
            previous_identifier = None
            cursor += 1
            continue

        token = source[cursor]
        if token in closing:
            delimiter_stack.append(
                (closing[token], macro_openings.get(cursor))
            )
        elif token in closing.values():
            if delimiter_stack and delimiter_stack[-1][0] == token:
                _, bang = delimiter_stack.pop()
                if bang is not None:
                    ranges.append((bang, cursor + 1))
            elif delimiter_stack:
                ranges.extend(
                    (bang, len(source))
                    for _, bang in delimiter_stack
                    if bang is not None
                )
                delimiter_stack.clear()
        previous_identifier = None
        cursor += 1
    ranges.extend(
        (bang, len(source))
        for _, bang in delimiter_stack
        if bang is not None
    )
    return sorted(set(ranges))


def _rust_has_explicit_concat_shadow(source: str) -> bool:
    """Conservatively detect source-local bindings named ``concat``."""

    def identifier_text(start: int) -> tuple[str, int, bool] | None:
        end = _rust_identifier_token_end(source, start)
        if end is None:
            return None
        text = source[start:end]
        is_raw = text.startswith("r#")
        return (text[2:] if is_raw else text, end, is_raw)

    def attribute_has_macro_use(
        start: int,
    ) -> tuple[bool, int | None]:
        cursor = _rust_skip_trivia(source, start + 1)
        if cursor < len(source) and source[cursor] == "!":
            cursor = _rust_skip_trivia(source, cursor + 1)
        if cursor >= len(source) or source[cursor] != "[":
            return False, None
        end = _rust_token_tree_end(source, cursor)
        if end is None:
            end = len(source)
        cursor += 1
        while cursor < end:
            cursor = _rust_skip_trivia(source, cursor)
            if cursor >= end:
                break
            identifier = identifier_text(cursor)
            if identifier is not None:
                text, cursor, _ = identifier
                if text == "macro_use":
                    return True, end
                continue
            literal = _rust_raw_literal(source, cursor)
            if literal is None:
                literal = _rust_ordinary_literal(source, cursor)
            if literal is not None:
                cursor = literal[2]
                continue
            cursor += 1
        return False, end

    cursor = _rust_file_code_start(source)
    while cursor < len(source):
        if source[cursor] in RUST_PATTERN_WHITESPACE:
            cursor += 1
            continue
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", cursor):
            cursor = _rust_block_comment_end(source, cursor)
            continue
        if source[cursor] == "#":
            has_macro_use, attribute_end = attribute_has_macro_use(
                cursor
            )
            if has_macro_use:
                return True
            if attribute_end is not None:
                cursor = attribute_end
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

        identifier = identifier_text(cursor)
        if identifier is None:
            cursor += 1
            continue
        text, end, is_raw = identifier
        if not is_raw and text == "macro_rules":
            bang = _rust_skip_trivia(source, end)
            if bang < len(source) and source[bang] == "!":
                name_start = _rust_skip_trivia(source, bang + 1)
                name = identifier_text(name_start)
                if name is not None and name[0] == "concat":
                    return True
        elif not is_raw and text == "macro":
            name_start = _rust_skip_trivia(source, end)
            name = identifier_text(name_start)
            if name is not None and name[0] == "concat":
                return True
        elif not is_raw and text == "use":
            use_cursor = end
            while use_cursor < len(source):
                use_cursor = _rust_skip_trivia(source, use_cursor)
                if (
                    use_cursor >= len(source)
                    or source[use_cursor] == ";"
                ):
                    break
                if source[use_cursor] == "'":
                    char_end = _rust_char_end(source, use_cursor)
                    if char_end is not None:
                        use_cursor = char_end
                        continue
                literal = _rust_raw_literal(source, use_cursor)
                if literal is None:
                    literal = _rust_ordinary_literal(
                        source,
                        use_cursor,
                    )
                if literal is not None:
                    use_cursor = literal[2]
                    continue
                use_identifier = identifier_text(use_cursor)
                if use_identifier is not None:
                    if use_identifier[0] == "concat":
                        return True
                    use_cursor = use_identifier[1]
                    continue
                use_cursor += 1
            cursor = use_cursor
            continue
        cursor = end
    return False


def _rust_concat_invocations(
    source: str,
    *,
    assume_concat_shadowed: bool = False,
) -> list[RustConcatInvocation]:
    """Locate non-overlapping ``concat!`` token trees outside Rust trivia."""

    macro_ranges: list[tuple[int, int]] = []
    for start, end in _rust_macro_token_tree_ranges(source):
        if macro_ranges and start <= macro_ranges[-1][1]:
            previous_start, previous_end = macro_ranges[-1]
            macro_ranges[-1] = (
                previous_start,
                max(previous_end, end),
            )
        else:
            macro_ranges.append((start, end))
    concat_is_shadowed = (
        assume_concat_shadowed
        or _rust_has_explicit_concat_shadow(source)
    )
    invocations: list[RustConcatInvocation] = []
    cursor = _rust_file_code_start(source)
    preceding_code = ""
    macro_range_index = 0
    while cursor < len(source):
        if source[cursor] in RUST_PATTERN_WHITESPACE:
            cursor += 1
            continue
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
                preceding_code = "?"
                cursor = char_end
                continue

        literal = _rust_raw_literal(source, cursor)
        if literal is None:
            literal = _rust_ordinary_literal(source, cursor)
        if literal is not None:
            preceding_code = "?"
            cursor = literal[2]
            continue

        if (
            source.startswith("concat", cursor)
            and (
                cursor == 0
                or not _rust_identifier_continues(source[cursor - 1])
            )
            and (
                cursor + len("concat") == len(source)
                or not _rust_identifier_continues(
                    source[cursor + len("concat")]
                )
            )
        ):
            invocation = _rust_concat_invocation_at(
                source,
                cursor,
                preceding_code,
            )
            if invocation is not None:
                while (
                    macro_range_index < len(macro_ranges)
                    and macro_ranges[macro_range_index][1] <= cursor
                ):
                    macro_range_index += 1
                nested = (
                    macro_range_index < len(macro_ranges)
                    and macro_ranges[macro_range_index][0] < cursor
                    < macro_ranges[macro_range_index][1]
                )
                if concat_is_shadowed or nested:
                    invocation = RustConcatInvocation(
                        invocation.start,
                        invocation.end,
                        None,
                    )
                invocations.append(invocation)
                cursor = invocation.end
                preceding_code = "?"
                continue
        preceding_code = (preceding_code + source[cursor])[-2:]
        cursor += 1
    return invocations


def _rust_concat_semantic_body(
    source: str, invocation: RustConcatInvocation
) -> tuple[str, ...] | None:
    if invocation.literals is None:
        return None
    bodies: list[str] = []
    for literal in invocation.literals:
        semantic = _rust_literal_semantic_body(
            source[literal.body_start : literal.body_end],
            literal.is_raw,
        )
        if semantic is None:
            return None
        bodies.append(semantic)
    return tuple(bodies)


def _partition_rust_concat_body(
    migrated: str, original_bodies: Sequence[str]
) -> tuple[str, ...]:
    """Split a migrated semantic value while preserving operand count."""

    if not original_bodies:
        return ()
    chunks: list[str] = []
    cursor = 0
    for original in original_bodies[:-1]:
        end = min(len(migrated), cursor + len(original))
        chunks.append(migrated[cursor:end])
        cursor = end
    chunks.append(migrated[cursor:])
    result = tuple(chunks)
    if "".join(result) != migrated:
        raise AssertionError("concat! migration partition changed its value")
    return result


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
_RUST_TEMPLATE_HOLE_RE = re.compile(
    r"(?<!\{)\{"
    r"(?:[A-Za-z_$][A-Za-z0-9_$]*|[0-9]+)"
    r"(?::[^{}\s]*)?"
    r"\}(?!\})"
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


def _rust_special_function_fragment_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    """Recognize isolated constructor and fallback source fragments."""

    prefixed_start = _rust_classic_prefix_start(tokens, index)
    source_boundary = _rust_source_boundary(source, tokens, index)
    if prefixed_start is not None:
        source_boundary = source_boundary or _rust_source_boundary(
            source, tokens, prefixed_start
        )
    if (
        not source_boundary
        or index + 1 >= len(tokens)
        or tokens[index + 1].text != "("
    ):
        return False
    params_close = matching_index(tokens, index + 1)
    if params_close is None:
        return False
    boundary = _header_boundary(tokens, params_close + 1)
    if boundary is None or not _rust_function_trailer_is_source_like(
        tokens[params_close + 1 : boundary]
    ):
        return False
    if tokens[boundary].text == ";":
        return _rust_source_line_ends_after(source, tokens[boundary])
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


def _rust_let_binding_is_source_like(tokens: Sequence[Token]) -> bool:
    """Recognize a complete scalar or tuple binding, not prose after `let`."""

    binding = list(tokens)
    if binding and binding[0].text == "comptime":
        binding = binding[1:]
    if len(binding) == 1:
        return binding[0].kind == "word"
    return (
        bool(binding)
        and binding[0].text == "("
        and matching_index(binding, 0) == len(binding) - 1
    )


def _rust_classic_type_signal(tokens: Sequence[Token]) -> bool:
    """Distinguish Classic Solcore type syntax from embedded Rust snippets."""

    base = 0
    while base < len(tokens) and tokens[base].text in {"@", "comptime"}:
        base += 1
    name_end = _qualified_name_end(tokens[base:])
    application_open = base + name_end
    if (
        name_end
        and application_open < len(tokens)
        and tokens[application_open].text == "("
        and tokens[application_open - 1].text != "fn"
    ):
        # An outer `Option(...)`/`pkg.Option(...)` shell is unambiguously
        # Classic type syntax even if one of its arguments is named `fn`.
        return True

    for index, token in enumerate(tokens):
        if (
            token.text == "fn"
            and index + 1 < len(tokens)
            and tokens[index + 1].text == "("
        ):
            return False
        if (
            token.text in {"dyn", "impl"}
            and index + 2 < len(tokens)
            and tokens[index + 1].text in {"Fn", "FnMut", "FnOnce"}
            and tokens[index + 2].text == "("
        ):
            return False
    if any(
        token.text
        in {
            "->",
            "@",
            "comptime",
            "function",
            "mapping",
            "memory",
            "storage",
            "calldata",
            "word",
            "integer",
        }
        for token in tokens
    ):
        return True
    return any(
        token.kind == "word"
        and index + 1 < len(tokens)
        and tokens[index + 1].text == "("
        for index, token in enumerate(tokens)
    )


def _rust_migratable_let_is_source_like(
    source: str, tokens: Sequence[Token], index: int
) -> bool:
    if not _rust_source_boundary(source, tokens, index):
        return False
    end = _statement_end(tokens, index)
    if end is None:
        end = next(
            (
                cursor
                for cursor in range(index + 1, len(tokens))
                if tokens[cursor].text == ";"
            ),
            None,
        )
    if end is None or not _rust_source_line_ends_after(source, tokens[end]):
        return False
    declaration = tokens[index + 1 : end]
    walrus = find_top(declaration, ":=", angles=False)
    walrus_type_colon = (
        find_top(declaration[:walrus], ":", angles=False)
        if walrus is not None
        else None
    )
    if walrus is not None:
        if (
            walrus_type_colon is None
            or walrus_type_colon == 0
            or walrus_type_colon + 1 >= walrus
            or walrus + 1 >= len(declaration)
            or not _rust_let_binding_is_source_like(
                declaration[:walrus_type_colon]
            )
        ):
            # An untyped standalone walrus declaration is valid Yul syntax.
            # Require a Classic-only type signal before classifying the
            # literal as Solcore; an enclosing Solcore fragment is detected
            # independently and still migrates its complete source.
            return False
        type_tokens = declaration[walrus_type_colon + 1 : walrus]
        return (
            type_tokens[0].text == "comptime"
            or _rust_classic_type_signal(type_tokens)
        )
    equals = find_top(declaration, "=", angles=False)
    if equals is not None:
        binding_end = find_top(
            declaration[:equals], ":", angles=False
        )
        binding = declaration[
            : equals if binding_end is None else binding_end
        ]
        if not _rust_let_binding_is_source_like(binding):
            return False
        for cursor in range(equals + 1, len(declaration)):
            if (
                declaration[cursor].text == ":"
                and not _is_ternary_colon(declaration, cursor)
            ):
                return True
    colon = find_top(declaration, ":", angles=False)
    if colon is None or colon == 0 or colon + 1 >= len(declaration):
        return False
    if equals is not None and colon > equals:
        return False
    canonical = declaration[0].text == "comptime"
    classic = declaration[colon + 1].text == "comptime"
    binding = declaration[:colon]
    if not _rust_let_binding_is_source_like(binding):
        return False
    if canonical or classic:
        return True

    type_end = equals if equals is not None else len(declaration)
    type_tokens = list(declaration[colon + 1 : type_end])
    if not type_tokens or not _rust_classic_type_signal(type_tokens):
        return False
    try:
        rendered = render_type(type_tokens)
    except ValueError:
        # The complete `let` shell and Classic-only type tokens already prove
        # this is embedded source.  Keep it classified so migration reports
        # the malformed type instead of silently treating the Rust file clean.
        return True
    return [token.text for token in significant(rendered)] != [
        token.text for token in type_tokens
    ]


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
    body_open = _expression_block_boundary(tokens, index + 1)
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
        if part[0].text == "(":
            close = matching_index(part, 0)
            if (
                close is not None
                and close > 1
                and all(item.kind == "symbol" for item in part[1:close])
                and (
                    close == len(part) - 1
                    or (
                        close + 2 == len(part) - 1
                        and part[close + 1].text == "as"
                        and part[close + 2].kind == "word"
                    )
                )
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


def _rust_unsupported_solidity_fragment_is_source_like(
    source: str, tokens: Sequence[Token]
) -> bool:
    """Recognize isolated omitted-Solidity declarations and statements."""

    unsupported = _unsupported_solidity_construct(source)
    if unsupported is None:
        return False
    token, construct, _ = unsupported
    index = next(
        (
            candidate
            for candidate, item in enumerate(tokens)
            if item.start == token.start and item.end == token.end
        ),
        None,
    )
    if index is None or not _rust_source_boundary(source, tokens, index):
        return False
    if "declaration" in construct:
        return True
    end = _statement_end(tokens, index)
    return end is not None and _rust_source_line_ends_after(
        source, tokens[end]
    )


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
    if _RUST_TEMPLATE_HOLE_RE.search(source) is not None:
        return False

    if _rust_unsupported_solidity_fragment_is_source_like(source, tokens):
        return True

    for index, token in enumerate(tokens):
        if token.text == "pragma" and _rust_pragma_fragment_is_source_like(
            source, tokens, index
        ):
            return True
        if token.text == "function" and _rust_function_fragment_is_source_like(
            source, tokens, index
        ):
            return True
        if (
            token.text in {"constructor", "fallback"}
            and _rust_special_function_fragment_is_source_like(
                source, tokens, index
            )
        ):
            return True
        if token.text in {"data", "type", "alias"} and (
            _rust_assignment_declaration_is_source_like(source, tokens, index)
        ):
            return True
        if token.text == "let" and _rust_migratable_let_is_source_like(
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


def _looks_like_solcore_concat_value(source: str) -> bool:
    """Require a complete source boundary for a joined macro value."""

    tokens = significant(source)
    return (
        bool(tokens)
        and tokens[-1].text in {";", "}"}
        and _rust_source_text_is_trivia(source[tokens[-1].end :])
        and _looks_like_solcore_literal(source)
    )


def _rust_solcore_literals(
    source: str,
    excluded_spans: Sequence[tuple[int, int]] | None = None,
) -> list[tuple[int, int, bool, str]]:
    """Locate Rust string bodies that look like embedded Solcore programs.

    The third result field is true for a raw string and the fourth records its
    Unicode, byte, or C-string kind. Ordinary strings are decoded before
    detection so an escaped newline terminates a Solcore line comment and does
    not hide the rest of the embedded program. Bodies inside any ``concat!``
    token tree are excluded because their semantic context is the joined macro
    value, not an individual operand.
    """

    if excluded_spans is None:
        excluded_spans = [
            (invocation.start, invocation.end)
            for invocation in _rust_concat_invocations(source)
        ]
    ordered_excluded_spans = sorted(excluded_spans)
    excluded_index = 0

    literals: list[tuple[int, int, bool, str]] = []
    cursor = _rust_file_code_start(source)
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

        body_start, body_end, literal_end, valid = literal
        body = source[body_start:body_end]
        literal_kind = _rust_literal_kind(source, cursor)
        semantic_body = _rust_literal_semantic_body(
            body,
            is_raw,
            literal_kind,
        )
        while (
            excluded_index < len(ordered_excluded_spans)
            and ordered_excluded_spans[excluded_index][1] <= body_start
        ):
            excluded_index += 1
        excluded = (
            excluded_index < len(ordered_excluded_spans)
            and ordered_excluded_spans[excluded_index][0]
            <= body_start
            < ordered_excluded_spans[excluded_index][1]
        )
        if (
            valid
            and not excluded
            and semantic_body is not None
            and _looks_like_solcore_literal(semantic_body)
        ):
            literals.append(
                (body_start, body_end, is_raw, literal_kind)
            )
        cursor = literal_end
    return literals


def _rust_solcore_literal_spans(source: str) -> list[tuple[int, int]]:
    return [
        (body_start, body_end)
        for body_start, body_end, _, _ in _rust_solcore_literals(source)
    ]


def has_rust_comment_marker(source: str, marker: str) -> bool:
    """Find a marker in Rust comments without inspecting literal bodies."""

    cursor = _rust_file_code_start(source)
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
    assume_concat_shadowed: bool = False,
) -> str:
    """Migrate Solcore programs embedded in Rust string literals.

    Standalone literals and direct Unicode string-only ``concat!`` groups are
    rewritten only when combined token, boundary, and declaration signals make
    them source-like. Unrelated regex, format templates, and prose remain
    byte-for-byte unchanged. Migrated ordinary strings are decoded and safely
    re-encoded; migrated concat operands become ordinary Unicode strings while
    retaining their operand count and intervening Rust comments.

    Set ``assume_concat_shadowed`` when a bare ``concat!`` may resolve to a
    custom macro outside this source. The CLI derives that flag across all
    selected Rust files from explicit imports, macro definitions, and
    ``macro_use`` attributes.
    """

    if has_rust_comment_marker(source, KEEP_RUST_FILE_MARKER):
        return source

    concat_invocations = _rust_concat_invocations(
        source,
        assume_concat_shadowed=assume_concat_shadowed,
    )
    concat_spans = [
        (invocation.start, invocation.end)
        for invocation in concat_invocations
    ]
    replacements: list[tuple[int, int, str]] = []
    for invocation in concat_invocations:
        original_bodies = _rust_concat_semantic_body(source, invocation)
        if (
            original_bodies is None
            or has_rust_comment_marker(
                source[invocation.start : invocation.end],
                KEEP_RUST_CONCAT_MARKER,
            )
        ):
            continue
        original_body = "".join(original_bodies)
        if not _looks_like_solcore_concat_value(original_body):
            continue

        body = original_body
        if classic_bare_imports:
            body = migrate_classic_bare_imports(body)
        migrated = migrate_source(
            body,
            global_constructor_owners,
            global_dot_constructor_candidates,
        )
        if migrated == original_body:
            continue

        chunks = _partition_rust_concat_body(
            migrated,
            original_bodies,
        )
        if invocation.literals is None:
            raise AssertionError("supported concat! lost its literals")
        for literal, chunk in zip(invocation.literals, chunks, strict=True):
            replacement = '"' + _encode_rust_ordinary_body(chunk) + '"'
            if (
                replacement
                != source[literal.literal_start : literal.literal_end]
            ):
                replacements.append(
                    (
                        literal.literal_start,
                        literal.literal_end,
                        replacement,
                    )
                )

    for body_start, body_end, is_raw, literal_kind in _rust_solcore_literals(
        source,
        concat_spans,
    ):
        encoded_body = source[body_start:body_end]
        body = _rust_literal_semantic_body(
            encoded_body,
            is_raw,
            literal_kind,
        )
        if body is None:
            continue
        original_body = body
        if classic_bare_imports:
            body = migrate_classic_bare_imports(body)
        migrated = migrate_source(
            body,
            global_constructor_owners,
            global_dot_constructor_candidates,
        )
        if migrated == original_body:
            continue
        if not is_raw:
            migrated = _encode_rust_ordinary_body(
                migrated,
                literal_kind,
            )
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
            "migrate Solcore programs inside Rust strings; each standalone "
            "literal or direct string-only concat group is an isolated source"
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
            originals[path] = source_bytes.decode(
                _default_text_encoding()
            )
        except Exception as error:
            failures.append((path, error))

    rust_concat_shadowed = (
        args.rust_strings
        and any(
            _rust_has_explicit_concat_shadow(source)
            for source in originals.values()
        )
    )
    prepared_sources: dict[Path, str] = {}
    for path, original in originals.items():
        try:
            prepared_sources[path] = (
                migrate_classic_bare_imports(original)
                if args.classic_bare_imports and not args.rust_strings
                else original
            )
        except Exception as error:
            failures.append((path, error))

    constructor_surfaces: dict[Path, ConstructorImportSurface] = {}
    if not args.rust_strings:
        try:
            constructor_surfaces = build_constructor_import_surfaces(
                prepared_sources
            )
        except Exception as error:
            print(
                "error: failed to build import-aware constructor surfaces: "
                f"{error}",
                file=sys.stderr,
            )
            return 2

    changed: list[Path] = []
    migrations: dict[Path, str] = {}
    for path, original in originals.items():
        if path not in prepared_sources:
            continue
        try:
            prepared = prepared_sources[path]
            migrated = (
                migrate_rust_strings(
                    original,
                    classic_bare_imports=args.classic_bare_imports,
                    assume_concat_shadowed=rust_concat_shadowed,
                )
                if args.rust_strings
                else migrate_source(
                    prepared,
                    constructor_import_surface=constructor_surfaces.get(path),
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
