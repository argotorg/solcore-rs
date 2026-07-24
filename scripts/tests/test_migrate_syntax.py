from __future__ import annotations

import importlib.util
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "migrate_syntax.py"
SPEC = importlib.util.spec_from_file_location("migrate_syntax", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MIGRATE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MIGRATE
SPEC.loader.exec_module(MIGRATE)


class DotConstructorMigrationTests(unittest.TestCase):
    def test_rewrites_payload_and_nullary_dot_constructors_idempotently(self) -> None:
        classic = """\
data Option(a) = None | Some(a);

function wrap(x: word) -> Option(word) {
  return .Some(x);
}

function empty() -> Option(word) {
  return .None;
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertIn("enum Option<a> { None, Some(a) }", migrated)
        self.assertIn("return Option.Some(x);", migrated)
        self.assertIn("return Option.None;", migrated)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_comments_inside_dot_constructor(self) -> None:
        classic = """\
enum Option<T> { None, Some(T) }
function wrap(x: word) returns (Option<word>) {
  return . /* owner comment */ Some(x);
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertIn(
            "return Option. /* owner comment */ Some(x);",
            migrated,
        )

    def test_rewrites_dot_constructors_in_classic_match_patterns(self) -> None:
        classic = """\
data Option(a) = None | Some(a);
function unwrap(value: Option(word)) -> word {
  return match value {
    | .Some(x) => x
    | .None => 0
  };
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertIn("case Option.Some(x)", migrated)
        self.assertIn("case Option.None", migrated)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_rewrites_dot_constructors_in_ternary_branches(self) -> None:
        classic = """\
data Option(a) = None | Some(a);
function choose(flag: bool, x: word) -> Option(word) {
  return flag ? .Some(x) : .None;
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertIn(
            "return flag ? Option.Some(x) : Option.None;",
            migrated,
        )

    def test_rewrites_dot_constructor_after_spaced_comparisons(self) -> None:
        source = """\
enum T { T(word) }
function compare(a: word, b: word, x: word) returns (bool) {
  return a < b > .T(x);
}
"""
        expected = """\
enum T { T(word) }
function compare(a: word, b: word, x: word) returns (bool) {
  return a < b > T.T(x);
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_spaced_proxy_type_member_access(self) -> None:
        source = """\
enum member { member(word) }
function project() {
  return @pkg.Box < word > .member;
}
"""

        self.assertEqual(MIGRATE.migrate_source(source), source)

    def test_does_not_rewrite_member_access(self) -> None:
        canonical = """\
enum Option<T> { None, Some(T) }
function project(value: Option<word>) returns (word) {
  return value.Some;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_does_not_qualify_locations_in_generic_conversion_types(
        self,
    ) -> None:
        canonical = """\
enum memory<T> { memory(word) }
function convert(x: word) {
  return memory.memory(0) as Box<T> memory;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_qualifies_non_same_name_local_constructors(self) -> None:
        source = """\
enum Option { Some(word) }
function use(x: Option, y: word) {
  Some(y);
  match (x) { case Some(value) { return; } }
}
"""
        expected = """\
enum Option { Some(word) }
function use(x: Option, y: word) {
  Option.Some(y);
  match (x) { case Option.Some(value) { return; } }
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_local_term_only_shadows_constructor_expressions(self) -> None:
        source = """\
enum Option { Some(word) }
function Some(x: word) returns (word) { return x; }
function use(x: Option, y: word) {
  Some(y);
  match (x) { case Some(value) { return; } }
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertIn("  Some(y);", migrated)
        self.assertIn("case Option.Some(value)", migrated)

    def test_rejects_ambiguous_dot_constructor(self) -> None:
        source = """\
enum Left { Some(word) }
enum Right { Some(word) }
function choose(x: word) returns (Left) {
  return .Some(x);
}
"""

        with self.assertRaisesRegex(
            ValueError,
            r"ambiguous legacy dot-constructor \.Some.*Left, Right",
        ):
            MIGRATE.migrate_source(source)

    def test_resolves_owner_across_cli_input(self) -> None:
        declaration = "data Option(a) = None | Some(a);\n"
        use = """\
function wrap(x: word) -> Option(word) {
  return .Some(x);
}
"""
        owners = MIGRATE.collect_global_dot_constructor_candidates(
            [declaration, use]
        )

        migrated = MIGRATE.migrate_source(use, {}, owners)

        self.assertIn("return Option.Some(x);", migrated)
        self.assertEqual(MIGRATE.migrate_source(migrated, {}, owners), migrated)

    def test_check_rejects_unresolved_dot_constructor(self) -> None:
        source = """\
function wrap(x: word) returns (Option<word>) {
  return .Some(x);
}
"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "unresolved.solc"
            path.write_text(source)

            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(result.returncode, 2)
        self.assertIn(
            "cannot resolve legacy dot-constructor .Some",
            result.stderr,
        )
        self.assertIn("line 2, column 10", result.stderr)

    def test_cli_migration_reaches_a_clean_check_fixed_point(self) -> None:
        source = """\
data Option(a) = None | Some(a);
function wrap(x: word) -> Option(word) { return .Some(x); }
"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "option.solc"
            path.write_text(source)

            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            check = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = path.read_text()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(check.returncode, 0, check.stderr)
        self.assertIn("return Option.Some(x);", migrated)
        self.assertIn("0 file(s) need migration", check.stdout)


class UnicodeIdentifierMigrationTests(unittest.TestCase):
    def test_lexer_recognizes_unicode_and_legacy_identifiers(self) -> None:
        tokens = MIGRATE.significant("_x $x λ fλ λ2")

        self.assertEqual(
            [(token.kind, token.text) for token in tokens],
            [
                ("word", "_x"),
                ("word", "$x"),
                ("word", "λ"),
                ("word", "fλ"),
                ("word", "λ2"),
            ],
        )

    def test_migrates_unicode_declarations_and_constructors(self) -> None:
        classic = """\
data λ(a) = λ(a);
function fλ(x: word) -> λ(word) { return λ(x); }
"""
        expected = """\
enum λ<a> { λ(a) }
function fλ(x: word) returns (λ<word>) { return λ.λ(x); }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_resolves_unicode_import_paths_and_constructors(self) -> None:
        sources = {
            Path("/workspace/πάροχος.solc"): """\
enum Τ { Τ(word) }
export {Τ(*)};
""",
            Path("/workspace/main.solc"): """\
import πάροχος;
function make(x: word) { Τ(x); }
""",
        }
        main = Path("/workspace/main.solc")
        surfaces = MIGRATE.build_constructor_import_surfaces(sources)

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("Τ.Τ(x)", migrated)
        self.assertFalse(surfaces[main].has_unknown_constructors)


class ImportAwareConstructorSurfaceTests(unittest.TestCase):
    def surfaces(
        self,
        sources: dict[str, str],
    ) -> tuple[
        dict[Path, str],
        dict[Path, MIGRATE.ConstructorImportSurface],
    ]:
        rooted = {
            Path("/workspace") / name: source
            for name, source in sources.items()
        }
        return rooted, MIGRATE.build_constructor_import_surfaces(rooted)

    def test_requires_an_explicit_constructor_export(self) -> None:
        consumer = """\
import provider;
function wrap(x: word) returns (T) { return .Some(x); }
"""
        providers = [
            "enum T { Some(word) }\n",
            "enum T { Some(word) }\nexport {T};\n",
            "enum T { Some(word) }\nexport {*};\n",
        ]

        for provider in providers:
            with self.subTest(provider=provider):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": provider,
                        "main.solc": consumer,
                    }
                )
                main = Path("/workspace/main.solc")
                with self.assertRaisesRegex(
                    ValueError,
                    r"cannot resolve legacy dot-constructor \.Some",
                ):
                    MIGRATE.migrate_source(
                        sources[main],
                        constructor_import_surface=surfaces[main],
                    )

    def test_import_keyword_table_matches_the_parser_lexer(self) -> None:
        lexer = (ROOT / "crates" / "parser" / "src" / "lexer.rs").read_text()
        lexer_words = frozenset(
            re.findall(
                r'#\[token\("([A-Za-z][A-Za-z0-9_]*)"\)\]',
                lexer,
            )
        )

        self.assertEqual(MIGRATE.CORE_LEXER_WORD_TOKENS, lexer_words)

    def test_reserved_import_identifiers_fail_closed(self) -> None:
        invalid_imports = (
            "import function;",
            "import pkg.function;",
            "import * as function from provider;",
            "import {function} from provider;",
            "import {T as function} from provider;",
            "import true;",
            "import fallback;",
        )
        for import_declaration in invalid_imports:
            with self.subTest(import_declaration=import_declaration):
                sources, surfaces = self.surfaces(
                    {
                        "function.solc": """\
enum T { T(word) }
export {T(*)};
""",
                        "pkg/function.solc": """\
enum T { T(word) }
export {T(*)};
""",
                        "provider.solc": """\
enum T { T(word) }
export {T(*)};
""",
                        "main.solc": f"""\
{import_declaration}
function make(x: word) {{ T(x); }}
""",
                    }
                )
                main = Path("/workspace/main.solc")

                migrated = MIGRATE.migrate_source(
                    sources[main],
                    constructor_import_surface=surfaces[main],
                )

                self.assertEqual(migrated, sources[main])
                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )

    def test_contextual_and_retired_import_words_remain_identifiers(
        self,
    ) -> None:
        for module_name in ("from", "data", "class", "memory"):
            with self.subTest(module_name=module_name):
                sources, surfaces = self.surfaces(
                    {
                        f"{module_name}.solc": """\
enum T { T(word) }
export {T(*)};
""",
                        "main.solc": f"""\
import {module_name};
function make(x: word) {{ T(x); }}
""",
                    }
                )
                main = Path("/workspace/main.solc")

                migrated = MIGRATE.migrate_source(
                    sources[main],
                    constructor_import_surface=surfaces[main],
                )

                self.assertIn("T.T(x)", migrated)
                self.assertFalse(
                    surfaces[main].has_unknown_constructors
                )

        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum from { from(word) }
export {from(*)};
""",
                "main.solc": """\
import {from as data} from provider;
function make(x: word) { from(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")
        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )
        self.assertIn("data.from(x)", migrated)

        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { Some(word) }
export {T(*)};
""",
                "main.solc": """\
import * as data from provider;
function make(x: word) { return .Some(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")
        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )
        self.assertIn("data.T.Some(x)", migrated)

    def test_open_import_exposes_selected_constructors(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T, Some(word) }
export {T(*)};
""",
                "main.solc": """\
import provider;
function make(x: word) returns (T) {
  let nested = id(T);
  match (nested) { case T { return .Some(x); } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("let nested = id(T.T);", migrated)
        self.assertIn("case T.T", migrated)
        self.assertIn("return T.Some(x);", migrated)

    def test_selective_alias_preserves_the_constructor_leaf(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word) }
export {T(*)};
""",
                "main.solc": """\
import {T as U} from provider;
function make(x: word) returns (U) { return T(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("return U.T(x);", migrated)

    def test_selective_import_allows_a_trailing_comma(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word), Some(word) }
export {T(*)};
""",
                "main.solc": """\
import {T,} from provider;
function make(x: word) returns (T) {
  T(x);
  return .Some(x);
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("  T.T(x);", migrated)
        self.assertIn("return T.Some(x);", migrated)
        self.assertFalse(surfaces[main].has_unknown_constructors)

    def test_duplicate_selective_import_names_fail_closed(self) -> None:
        for import_declaration in (
            "import {T, T} from provider;",
            "import {T as U, T as V} from provider;",
            "import {T as U, X as U} from provider;",
        ):
            with self.subTest(import_declaration=import_declaration):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": """\
enum T { T(word) }
enum X { X(word) }
export {T(*), X(*)};
""",
                        "main.solc": f"""\
{import_declaration}
function make(x: word) {{
  T(x);
  X(x);
}}
""",
                    }
                )
                main = Path("/workspace/main.solc")

                migrated = MIGRATE.migrate_source(
                    sources[main],
                    constructor_import_surface=surfaces[main],
                )

                self.assertEqual(migrated, sources[main])
                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )

    def test_namespace_import_uses_the_full_type_qualifier(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word), Some(word) }
export {T(*)};
""",
                "main.solc": """\
import * as P from provider;
function make(x: word) returns (P.T) {
  T(x);
  return .Some(x);
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("  P.T.T(x);", migrated)
        self.assertIn("return P.T.Some(x);", migrated)
        self.assertIn("T", surfaces[main].bare_candidates)

    def test_imported_term_wins_in_expressions_but_not_patterns(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word) }
function T(x: word) returns (word) { return x; }
export {T(*)};
export {T};
""",
                "main.solc": """\
import {T} from provider;
function use(x: T) {
  match (x) { case T(y) { T(y); } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("case T.T(y)", migrated)
        self.assertIn("{ T(y); }", migrated)

    def test_imported_function_shadows_a_source_local_constructor_term(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
function T(x: word) returns (word) { return x; }
export {T};
""",
                "main.solc": """\
import {T} from provider;
enum T { T(word) }
function use(x: T, y: word) {
  T(y);
  match (x) { case T(value) { return; } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("  T(y);", migrated)
        self.assertIn("case T.T(value)", migrated)

    def test_imported_constructor_does_not_shadow_builtin_pair(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum pair { pair(word) }
export {pair(*)};
""",
                "main.solc": """\
import provider;
function make(a: word, b: word) {
  pair(a, b);
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertEqual(migrated, sources[main])
        self.assertNotIn("pair", surfaces[main].bare_candidates)

    def test_local_and_imported_constructor_origins_are_ambiguous(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word) }
export {T(*)};
""",
                "main.solc": """\
import provider;
enum T { T(word) }
function make(x: word) returns (T) { return T(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertEqual(migrated, sources[main])

    def test_unrelated_files_do_not_seed_constructor_owners(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word) }
export {T(*)};
""",
                "main.solc": """\
function make(x: word) returns (word) { return T(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertEqual(migrated, sources[main])

    def test_same_spelling_from_two_origins_remains_ambiguous(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "a.solc": "enum T { Some(word) }\nexport {T(*)};\n",
                "b.solc": "enum T { Some(word) }\nexport {T(*)};\n",
                "main.solc": """\
import a;
import b;
function make(x: word) returns (T) { return .Some(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")

        with self.assertRaisesRegex(
            ValueError,
            r"ambiguous legacy dot-constructor \.Some.*a\.solc.*b\.solc",
        ):
            MIGRATE.migrate_source(
                sources[main],
                constructor_import_surface=surfaces[main],
            )

    def test_unresolved_import_blocks_terms_but_not_local_patterns(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "main.solc": """\
import missing;
enum T { T(word) }
function use(x: T, y: word) {
  T(y);
  match (x) { case T(value) { return; } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("  T(y);", migrated)
        self.assertIn("case T.T(value)", migrated)
        self.assertTrue(
            surfaces[main].has_unknown_unqualified_terms
        )

    def test_unresolved_import_blocks_matching_imported_owner(
        self,
    ) -> None:
        for unresolved in (
            "import missing;",
            "import {S} from missing;",
            "import {S as U} from missing;",
        ):
            with self.subTest(unresolved=unresolved):
                sources, surfaces = self.surfaces(
                    {
                        "known.solc": """\
enum S { S(word) }
export {S(*)};
""",
                        "main.solc": f"""\
import known;
{unresolved}
function use(x: S, y: word) {{
  S(y);
  match (x) {{ case S(value) {{ return; }} }}
}}
""",
                    }
                )
                main = Path("/workspace/main.solc")

                migrated = MIGRATE.migrate_source(
                    sources[main],
                    constructor_import_surface=surfaces[main],
                )

                self.assertEqual(migrated, sources[main])
                self.assertTrue(
                    surfaces[main].has_unknown_unqualified_constructors
                )

    def test_unresolved_selective_import_may_hide_a_reexported_leaf(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "base.solc": """\
enum T { T(word) }
export {T(*)};
""",
                "facade.solc": """\
import {T as S} from base;
export {S(*)};
""",
                "main.solc": """\
import base;
import {S} from facade;
function use(x: T, y: word) {
  T(y);
  match (x) { case T(value) { return; } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertEqual(migrated, sources[main])
        self.assertTrue(
            surfaces[main].has_unknown_unqualified_constructors
        )

    def test_unresolved_namespace_hides_a_bare_imported_owner(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "known.solc": """\
enum S { S(word) }
export {S(*)};
""",
                "main.solc": """\
import known;
import * as P from missing;
function use(x: S, y: word) {
  S(y);
  match (x) { case S(value) { return; } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertEqual(migrated, sources[main])
        self.assertTrue(
            surfaces[main].has_unknown_unqualified_constructors
        )

    def test_qualifies_every_visible_imported_constructor_leaf(
        self,
    ) -> None:
        cases = (
            (
                "import provider;",
                "Option",
                "Option",
            ),
            (
                "import {Option as Maybe} from provider;",
                "Maybe",
                "Maybe",
            ),
            (
                "import * as P from provider;",
                "P.Option",
                "P.Option",
            ),
        )
        for declaration, parameter_type, owner in cases:
            with self.subTest(declaration=declaration):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": """\
enum Option { Some(word) }
export {Option(*)};
""",
                        "main.solc": f"""\
{declaration}
function use(x: {parameter_type}, y: word) {{
  Some(y);
  match (x) {{ case Some(value) {{ return; }} }}
}}
""",
                    }
                )
                main = Path("/workspace/main.solc")

                migrated = MIGRATE.migrate_source(
                    sources[main],
                    constructor_import_surface=surfaces[main],
                )

                self.assertIn(f"  {owner}.Some(y);", migrated)
                self.assertIn(
                    f"case {owner}.Some(value)",
                    migrated,
                )

    def test_imported_term_only_shadows_constructor_expressions(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum Option { Some(word) }
function Some(x: word) returns (word) { return x; }
export {Option(*)};
export {Some};
""",
                "main.solc": """\
import provider;
function use(x: Option, y: word) {
  Some(y);
  match (x) { case Some(value) { return; } }
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("  Some(y);", migrated)
        self.assertIn("case Option.Some(value)", migrated)

    def test_cli_import_surface_reaches_a_fixed_point(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            provider = root / "provider.solc"
            main = root / "main.solc"
            provider.write_text(
                "enum T { T, Some(word) }\nexport {T(*)};\n"
            )
            main.write_text(
                "import provider;\n"
                "function make(x: word) returns (T) {\n"
                "  let nested = id(T);\n"
                "  return .Some(x);\n"
                "}\n"
            )

            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(root)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            check = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--check",
                    str(root),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = main.read_text()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(check.returncode, 0, check.stderr)
        self.assertIn("id(T.T)", migrated)
        self.assertIn("return T.Some(x);", migrated)
        self.assertIn("0 file(s) need migration", check.stdout)

    def test_cli_deduplicates_relative_and_absolute_source_paths(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            provider = root / "provider.solc"
            main = root / "main.solc"
            provider.write_text(
                "enum T { T(word) }\nexport {T(*)};\n"
            )
            main.write_text(
                "import provider;\n"
                "function make(x: word) { T(x); }\n"
            )

            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    root.name,
                    str(provider),
                ],
                cwd=root.parent,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = main.read_text()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertIn("T.T(x);", migrated)
        self.assertIn("2 file(s) examined", migration.stdout)

    def test_special_and_colliding_paths_fail_closed(self) -> None:
        cases = [
            {
                "std/opcodes.solc": (
                    "enum byte { byte(word) }\nexport {byte(*)};\n"
                ),
                "main.solc": """\
import std.opcodes;
function pair(a: word, b: word) { byte(a, b); }
""",
            },
            {
                "provider.sol": (
                    "enum T { T(word) }\nexport {T(*)};\n"
                ),
                "provider.solc": (
                    "enum T { T(word) }\nexport {T(*)};\n"
                ),
                "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
            },
        ]

        for case in cases:
            with self.subTest(paths=sorted(case)):
                sources, surfaces = self.surfaces(case)
                main = Path("/workspace/main.solc")
                migrated = MIGRATE.migrate_source(
                    sources[main],
                    constructor_import_surface=surfaces[main],
                )
                self.assertEqual(migrated, sources[main])
                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )

    def test_malformed_import_and_provider_fail_closed(self) -> None:
        cases = [
            {
                "provider.solc": (
                    "enum T { T(word) }\nexport {T(*)};\n"
                ),
                "main.solc": """\
import {,T} from provider;
function make(x: word) { T(x); }
""",
            },
            {
                "provider.solc": """\
enum T { T(word) }
export {T(*)};
function broken( {
""",
                "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
            },
        ]

        for case in cases:
            with self.subTest(case=case):
                sources, surfaces = self.surfaces(case)
                main = Path("/workspace/main.solc")
                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )
                self.assertEqual(
                    MIGRATE.migrate_source(
                        sources[main],
                        constructor_import_surface=surfaces[main],
                    ),
                    sources[main],
                )

    def test_balanced_invalid_provider_items_fail_closed(self) -> None:
        invalid_items = (
            "this is not syntax;",
            "function broken() frobnicate;",
            "function broken ?;",
            "struct Broken { x: }",
            "function broken() frobnicate {}",
        )
        for invalid_item in invalid_items:
            with self.subTest(invalid_item=invalid_item):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": f"""\
enum T {{ T(word), Some(word) }}
export {{T(*)}};
{invalid_item}
""",
                        "bare.solc": """\
import provider;
function make(x: word) { T(x); }
""",
                        "dot.solc": """\
import provider;
function make(x: word) { return .Some(x); }
""",
                    }
                )
                bare = Path("/workspace/bare.solc")
                dot = Path("/workspace/dot.solc")

                self.assertTrue(
                    surfaces[bare].has_unknown_constructors
                )
                self.assertFalse(surfaces[bare].bare_candidates)
                self.assertEqual(
                    MIGRATE.migrate_source(
                        sources[bare],
                        constructor_import_surface=surfaces[bare],
                    ),
                    sources[bare],
                )
                with self.assertRaisesRegex(
                    ValueError,
                    r"cannot resolve legacy dot-constructor \.Some",
                ):
                    MIGRATE.migrate_source(
                        sources[dot],
                        constructor_import_surface=surfaces[dot],
                    )

    def test_reserved_keywords_in_provider_types_fail_closed(self) -> None:
        invalid_items = (
            "function bad(x: return) {}",
            "struct Bad { x: return; }",
            "enum Bad { Bad(return) }",
            "alias Bad = return;",
            "alias Bad = Option<return>;",
            "alias Bad = function(return);",
            "alias Bad = comptime return;",
            "alias Bad = @comptime word;",
            "function bad() returns (return) {}",
            "function bad<A>(x: A) where return: Eq {}",
            "function bad<A>(x: A) where A: return {}",
        )
        for invalid_item in invalid_items:
            with self.subTest(invalid_item=invalid_item):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": f"""\
enum T {{ T(word) }}
export {{T(*)}};
{invalid_item}
""",
                        "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
                    }
                )
                main = Path("/workspace/main.solc")

                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )
                self.assertEqual(
                    MIGRATE.migrate_source(
                        sources[main],
                        constructor_import_surface=surfaces[main],
                    ),
                    sources[main],
                )

    def test_provider_type_keywords_match_the_parser(self) -> None:
        for keyword in MIGRATE.CORE_LEXER_WORD_TOKENS - {"from"}:
            with self.subTest(keyword=keyword):
                self.assertFalse(
                    MIGRATE._provider_type_is_valid(
                        MIGRATE.significant(keyword)
                    )
                )
        self.assertTrue(
            MIGRATE._provider_type_is_valid(
                MIGRATE.significant("from")
            )
        )

    def test_malformed_provider_export_lists_fail_closed(self) -> None:
        for declaration in (
            "export {, T(*)};",
            "export {T(*),,};",
            "export {T(*),,,};",
        ):
            with self.subTest(declaration=declaration):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": (
                            "enum T { T(word) }\n"
                            "export {T(*)};\n"
                            f"{declaration}\n"
                        ),
                        "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
                    }
                )
                main = Path("/workspace/main.solc")

                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )
                self.assertEqual(
                    MIGRATE.migrate_source(
                        sources[main],
                        constructor_import_surface=surfaces[main],
                    ),
                    sources[main],
                )

    def test_provider_export_list_allows_one_trailing_comma(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T { T(word) }
export {T(*),};
""",
                "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertFalse(surfaces[main].has_unknown_constructors)
        self.assertIn("T.T(x)", migrated)

    def test_invalid_provider_pragma_and_trait_heads_fail_closed(
        self,
    ) -> None:
        invalid_items = (
            "pragma nonsense foo;",
            "pragma ?;",
            "pragma solcore;",
            "pragma solcore noCoverageCondition , A;",
            "pragma solcore noCoverageCondition A,, B;",
            "trait Bad {}",
            "impl Bad {}",
        )
        for invalid_item in invalid_items:
            with self.subTest(invalid_item=invalid_item):
                sources, surfaces = self.surfaces(
                    {
                        "provider.solc": f"""\
enum T {{ T(word) }}
export {{T(*)}};
{invalid_item}
""",
                        "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
                    }
                )
                main = Path("/workspace/main.solc")

                self.assertTrue(
                    surfaces[main].has_unknown_constructors
                )
                self.assertEqual(
                    MIGRATE.migrate_source(
                        sources[main],
                        constructor_import_surface=surfaces[main],
                    ),
                    sources[main],
                )

    def test_valid_provider_pragma_and_trait_heads_remain_trusted(
        self,
    ) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
pragma solidity ^0.8.23;
pragma abicoder v2;
pragma solcore noCoverageCondition A, B,;
trait Good<A> {}
impl Good<word> {}
enum T { T(word) }
export {T(*)};
""",
                "main.solc": """\
import provider;
function make(x: word) { T(x); }
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertFalse(surfaces[main].has_unknown_constructors)
        self.assertIn("T.T(x)", migrated)

    def test_valid_complex_provider_items_remain_trusted(self) -> None:
        sources, surfaces = self.surfaces(
            {
                "provider.solc": """\
enum T<A> { T(A), Some(A), }
struct Box<A> { value: A; }
alias Contextual = from;
alias Nested = Option<from>;
alias CompileProxy = comptime @from;
alias Callback = function(from) internal pure returns (from);
function helper<A>(x: A) pure returns (result: A)
where A: Eq {
  return x;
}
export {T(*)};
""",
                "main.solc": """\
import provider;
function make(x: word) returns (T<word>) {
  T(x);
  return .Some(x);
}
""",
            }
        )
        main = Path("/workspace/main.solc")

        migrated = MIGRATE.migrate_source(
            sources[main],
            constructor_import_surface=surfaces[main],
        )

        self.assertIn("  T.T(x);", migrated)
        self.assertIn("return T.Some(x);", migrated)
        self.assertFalse(surfaces[main].has_unknown_constructors)


class ImportHidingMigrationTests(unittest.TestCase):
    def test_filters_selected_imports_by_their_local_alias(self) -> None:
        cases = [
            (
                "import M.{f as g, h} hiding {g};\n",
                "import {h} from M;\n",
            ),
            (
                "import M.{f as g, h} hiding {f};\n",
                "import {f as g, h} from M;\n",
            ),
            (
                "import M.{f, g, h as local} hiding {f, local};\n",
                "import {g} from M;\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_rejects_hiding_that_removes_every_selected_name(self) -> None:
        cases = [
            "import M.{f} hiding {f};\n",
            "import M.{f, g} hiding {f, g};\n",
            "import M.{f as g} hiding {g};\n",
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate empty selective import",
                ):
                    MIGRATE.migrate_source(classic)

    def test_rejects_empty_result_in_isolated_rust_imports(self) -> None:
        rust = (
            'const SOURCE: &str = '
            'r#"import M.{f as g} hiding {g};"#;\n'
        )

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 1)
        with self.assertRaisesRegex(
            ValueError,
            "cannot migrate empty selective import",
        ):
            MIGRATE.migrate_rust_strings(rust)


class OperatorImportMigrationTests(unittest.TestCase):
    def test_rejects_classic_operator_import_selectors(self) -> None:
        cases = [
            "import math.{pow, (^^)};\n",
            "import math.{(^^) as power};\n",
            "import math.{pow} hiding {(^^)};\n",
            "import {(^^) as power} from math;\n",
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate operator import selector",
                ):
                    MIGRATE.migrate_source(classic)

    def test_keeps_identifier_only_core_imports(self) -> None:
        canonical = "import {pow, power as renamed} from math;\n"

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_non_identifier_classic_import_selectors(self) -> None:
        cases = [
            "import M.{T(*)};\n",
            "import M.{T(T)};\n",
            "import M.{value} hiding {T(*)};\n",
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate non-identifier",
                ):
                    MIGRATE.migrate_source(classic)

    def test_leaves_non_operator_negative_selector_fixtures_unchanged(
        self,
    ) -> None:
        negative = "import {D(C)} from lib;\n"

        self.assertEqual(MIGRATE.migrate_source(negative), negative)

    def test_rejects_operator_imports_in_isolated_rust_literals(self) -> None:
        rust = r'''
const RAW: &str = r#"import math.{pow, (^^)};"#;
const ORDINARY: &str = "import {(^^) as power} from math;";
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 2)
        with self.assertRaisesRegex(
            ValueError,
            "cannot migrate operator import selector",
        ):
            MIGRATE.migrate_rust_strings(rust)


class StringImportMigrationTests(unittest.TestCase):
    def test_rejects_all_solidity_string_import_forms(self) -> None:
        cases = [
            'import "M/N.sol";\n',
            'import {f, g as h} from /* path */ "M/N.sol";\n',
            'import * as M from "M/N.sol";\n',
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate string import",
                ):
                    MIGRATE.migrate_source(classic)

    def test_accepts_dotted_core_imports_and_unrelated_strings(self) -> None:
        canonical = """\
import std.dispatch;
import {f} from @ext.foo;
function message() returns (string) { return "M/N.sol"; }
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_string_imports_in_isolated_rust_literals(self) -> None:
        cases = [
            'const SOURCE: &str = r##"import "M/N.sol";"##;\n',
            (
                'const SOURCE: &str = '
                '"import {f} from \\"M/N.sol\\";";\n'
            ),
            (
                'const SOURCE: &str = '
                'r##"import * as M from "M/N.sol";"##;\n'
            ),
            (
                'const SOURCE: &str = '
                'r##"import "M/N.sol" as M;"##;\n'
            ),
        ]

        for rust in cases:
            with self.subTest(rust=rust):
                self.assertEqual(
                    len(MIGRATE._rust_solcore_literal_spans(rust)),
                    1,
                )
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate string import",
                ):
                    MIGRATE.migrate_rust_strings(rust)


class ClassicBareImportMigrationTests(unittest.TestCase):
    def test_disambiguates_generated_aliases_with_the_same_leaf(self) -> None:
        classic = """\
import alpha.util;
import beta.util;

function read() -> word {
  return alpha.util.first() + beta.util.second();
}
"""
        expected = """\
import * as alpha_util from alpha.util;
import * as beta_util from beta.util;

function read() -> word {
  return alpha_util.first() + beta_util.second();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_avoids_capturing_a_local_binding_with_generated_alias(self) -> None:
        classic = """\
import foo.bar;

function read(bar: Receiver) -> word {
  return foo.bar.read();
}
"""
        expected = """\
import * as foo_bar from foo.bar;

function read(bar: Receiver) -> word {
  return foo_bar.read();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_rewrites_overlapping_paths_longest_first(self) -> None:
        classic = """\
import foo.bar;
import foo.bar.baz;

function read() -> word {
  return foo.bar.read() + foo.bar.baz.read();
}
"""
        expected = """\
import * as bar from foo.bar;
import * as baz from foo.bar.baz;

function read() -> word {
  return bar.read() + baz.read();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_parameter_shadowing_preserves_receiver_access(self) -> None:
        classic = """\
import foo.bar;

function shadowed(foo: Receiver) -> word {
  let value: foo.bar.Value;
  return foo.bar();
}

function namespaceUse() -> word {
  return foo.bar.run();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertIn("import * as bar from foo.bar;", migrated)
        self.assertIn("let value: bar.Value;", migrated)
        self.assertIn("return foo.bar();", migrated)
        self.assertIn("return bar.run();", migrated)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_named_result_shadowing_preserves_receiver_access(self) -> None:
        classic = """\
import foo.bar;

function shadowed() returns (foo: Receiver) {
  return foo.bar();
}
"""
        expected = """\
import * as bar from foo.bar;

function shadowed() returns (foo: Receiver) {
  return foo.bar();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_local_shadowing_respects_nested_block_scope(self) -> None:
        classic = """\
import foo.bar;

function nested() -> word {
  {
    let foo = foo.bar.make();
    foo.bar();
  }
  return foo.bar.run();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertIn("let foo = bar.make();", migrated)
        self.assertIn("foo.bar();", migrated)
        self.assertIn("return bar.run();", migrated)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_contract_field_shadows_namespace_in_methods(self) -> None:
        classic = """\
import foo.bar;

contract Wallet {
  foo: Receiver;
  cached: word = foo.bar();

  function read() -> word {
    return foo.bar();
  }
}

function namespaceUse() -> word {
  return foo.bar.read();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertIn("cached: word = foo.bar();", migrated)
        self.assertIn("return foo.bar();", migrated)
        self.assertIn("return bar.read();", migrated)

    def test_contract_method_does_not_shadow_imported_namespace_path(
        self,
    ) -> None:
        classic = """\
import foo.bar;
contract C {
  function foo(x: word) returns (word) { return x; }
  function use() returns (word) { return foo.bar.run(); }
}
"""
        expected = """\
import * as bar from foo.bar;
contract C {
  function foo(x: word) returns (word) { return x; }
  function use() returns (word) { return bar.run(); }
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_match_binder_shadows_only_its_classic_arm(self) -> None:
        classic = """\
import foo.bar;

function read(value: pair(Receiver, word)) -> word {
  return match value {
    | (foo, _) => foo.bar()
    | _ => foo.bar.read()
  };
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertIn("| (foo, _) => foo.bar()", migrated)
        self.assertIn("| _ => bar.read()", migrated)

    def test_match_comparison_preserves_binder_shadow_scope(self) -> None:
        classic = """\
import foo.bar;

function read(x: word, y: word) -> word {
  return match x < y {
    | foo => foo.bar()
    | _ => foo.bar.read()
  };
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertIn("| foo => foo.bar()", migrated)
        self.assertIn("| _ => bar.read()", migrated)

    def test_for_binding_scope_ends_after_loop_body(self) -> None:
        classic = """\
import foo.bar;

function read() -> word {
  for (let foo = receiver(); foo.ready(); foo.step()) {
    foo.bar();
  }
  return foo.bar.read();
}
"""

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertIn("foo.bar();", migrated)
        self.assertIn("return bar.read();", migrated)

    def test_cli_flag_preserves_shadowed_receiver_and_reaches_fixed_point(
        self,
    ) -> None:
        source = """\
import foo.bar;

function shadowed(foo: Receiver) -> word {
  return foo.bar();
}

function namespaceUse() -> word {
  return foo.bar.run();
}
"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "imports.solc"
            path.write_text(source)

            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--classic-bare-imports",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            check = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--check",
                    "--classic-bare-imports",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = path.read_text()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(check.returncode, 0, check.stderr)
        self.assertIn("import * as bar from foo.bar;", migrated)
        self.assertIn("return foo.bar();", migrated)
        self.assertIn("return bar.run();", migrated)
        self.assertIn("0 file(s) need migration", check.stdout)


class FunctionStyleCallPreservationTests(unittest.TestCase):
    def test_preserves_elementary_spelling_calls_byte_for_byte(self) -> None:
        canonical = (
            "function uint256(x: word) returns (word) { return x; }\r\n"
            "function address() returns (word) { return 0; }\r\n"
            "function use(x: word) {\r\n"
            "  let nested = uint256 /* target */ (uint256(x));\r\n"
            "  let nullary = address();\r\n"
            "  let binary = uint256(x, x);\r\n"
            "}\r\n"
        )

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_qualifies_only_proven_same_name_constructors(self) -> None:
        source = """\
enum uint256 { uint256(word) }
enum Other { address(word) }
function use(x: word) {
  let wrapped = uint256(x);
  let unresolved = address(x);
}
"""
        expected = """\
enum uint256 { uint256(word) }
enum Other { address(word) }
function use(x: word) {
  let wrapped = uint256.uint256(x);
  let unresolved = address(x);
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_comparisons_do_not_hide_constructor_calls(self) -> None:
        source = """\
enum T { T(word) }
function between(a: word, b: word, x: word) returns (bool) {
  return a < T(x) > b;
}
"""
        expected = """\
enum T { T(word) }
function between(a: word, b: word, x: word) returns (bool) {
  return a < T.T(x) > b;
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_calls_from_open_and_selective_imports(self) -> None:
        canonical = """\
import std.opcodes;
import {balance as uint256} from std.opcodes;
function use(x: word) {
  let opcode = address();
  let imported = uint256(x);
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_does_not_turn_type_namespace_callees_into_conversions(self) -> None:
        canonical = """\
import * as pkg from types;
alias Amount = word;
alias Box<T> = T;
type Wad is word;
function use(x: word) {
  let builtin = word(x);
  let aliasCall = Amount(x);
  let valueTypeCall = Wad(x);
  let genericCall = Box<word>(x);
  let classicGenericCall = Box(word)(x);
  let qualifiedCall = pkg.Result<word, Error>(x);
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_named_results_shadow_same_name_constructors(self) -> None:
        canonical = """\
enum T { T(word) }
function use(x: word) returns (T: word) {
  T = x;
  return T;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_constructor_shadowing_uses_precise_let_scopes(self) -> None:
        source = """\
enum T { T(word) }
function use(x: word) {
  let before = T(x);
  {
    let T = T(x);
    T(x);
  }
  let after = T(x);
  {
    let T = callable;
    T(x);
  }
  let final = T(x);
}
"""
        expected = """\
enum T { T(word) }
function use(x: word) {
  let before = T.T(x);
  {
    let T = T.T(x);
    T(x);
  }
  let after = T.T(x);
  {
    let T = callable;
    T(x);
  }
  let final = T.T(x);
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_constructor_term_shadows_stay_in_their_contract(self) -> None:
        source = """\
enum T { T(word) }
contract FieldShadow {
  T: Callback;
  function use(x: word) { T(x); }
}
contract MethodShadow {
  function T(x: word) returns (word) { return x; }
  function use(x: word) { T(x); }
}
contract Unshadowed {
  function use(x: word) { T(x); }
}
function make(x: word) returns (T) { return T(x); }
"""
        expected = """\
enum T { T(word) }
contract FieldShadow {
  T: Callback;
  function use(x: word) { T(x); }
}
contract MethodShadow {
  function T(x: word) returns (word) { return x; }
  function use(x: word) { T(x); }
}
contract Unshadowed {
  function use(x: word) { T.T(x); }
}
function make(x: word) returns (T) { return T.T(x); }
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_constructor_term_shadows_apply_in_field_initializers(
        self,
    ) -> None:
        cases = [
            """\
enum T { T(word) }
contract C {
  T: Callback;
  value: word = T(1);
}
""",
            """\
enum T { T(word) }
function T(x: word) returns (word) { return x; }
contract C {
  value: word = T(1);
}
""",
        ]

        for source in cases:
            with self.subTest(source=source):
                migrated = MIGRATE.migrate_source(source)
                self.assertEqual(migrated, source)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_qualifies_nested_constructor_in_field_initializer(
        self,
    ) -> None:
        source = """\
enum T { T(word) }
contract C {
  value: T = id(T(1));
}
"""
        expected = """\
enum T { T(word) }
contract C {
  value: T = id(T.T(1));
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_class_methods_shadow_constructor_fallback_module_wide(
        self,
    ) -> None:
        cases = [
            (
                """\
enum T { T(word) }
trait C<a> {
  function T(x: word) returns (a);
}
function use(x: word) returns (word) { return T(x); }
""",
                """\
enum T { T(word) }
trait C<a> {
  function T(x: word) returns (a);
}
function use(x: word) returns (word) { return T(x); }
""",
            ),
            (
                """\
enum T { T(word) }
trait C<a> {
  function T(x: a) returns (a);
}
impl C<word> {
  function T(x: word) returns (word) { return x; }
}
function use(x: word) returns (word) { return T(x); }
""",
                """\
enum T { T(word) }
trait C<a> {
  function T(x: a) returns (a);
}
impl C<word> {
  function T(x: word) returns (word) { return x; }
}
function use(x: word) returns (word) { return T(x); }
""",
            ),
            (
                """\
enum T { T(word) }
trait C<a> {
  function apply(x: a) returns (a);
}
impl C<word> {
  function T(x: word) returns (word) { return x; }
}
function use(x: word) returns (T) { return T(x); }
""",
                """\
enum T { T(word) }
trait C<a> {
  function apply(x: a) returns (a);
}
impl C<word> {
  function T(x: word) returns (word) { return x; }
}
function use(x: word) returns (T) { return T.T(x); }
""",
            ),
            (
                """\
enum T { T(word) }
trait C<a> {
  function T(x: word) returns (a);
}
trait D<a> {
  function T(x: word) returns (a);
}
function use(x: word) returns (T) { return T(x); }
""",
                """\
enum T { T(word) }
trait C<a> {
  function T(x: word) returns (a);
}
trait D<a> {
  function T(x: word) returns (a);
}
function use(x: word) returns (T) { return T.T(x); }
""",
            ),
        ]

        for source, expected in cases:
            with self.subTest(source=source):
                migrated = MIGRATE.migrate_source(source)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_lowercase_match_patterns_follow_constructor_visibility(
        self,
    ) -> None:
        cases = [
            (
                """\
enum Other { Other(word) }
function use(x: word) returns (word) {
  match (x) { case t { return t; } }
}
""",
                """\
enum Other { Other(word) }
function use(x: word) returns (word) {
  match (x) { case t { return t; } }
}
""",
            ),
            (
                """\
data t = t(word);
function use(x: word) -> word {
  match x { | t => return t; }
}
""",
                """\
enum t { t(word) }
function use(x: word) returns (word) {
  match (x) {
case t.t {
return t.t;
}
}
}
""",
            ),
            (
                """\
enum t { t }
function use(x: t) returns (word) {
  match (x) { case t { return 0; } }
}
""",
                """\
enum t { t }
function use(x: t) returns (word) {
  match (x) { case t.t { return 0; } }
}
""",
            ),
            (
                """\
enum t { t(word) }
function use(x: t) returns (word) {
  match (x) { case t(value) { return value; } }
}
""",
                """\
enum t { t(word) }
function use(x: t) returns (word) {
  match (x) { case t.t(value) { return value; } }
}
""",
            ),
            (
                """\
enum T { T }
function use(x: T) returns (T) {
  match (x) { case T { return T; } }
}
""",
                """\
enum T { T }
function use(x: T) returns (T) {
  match (x) { case T.T { return T.T; } }
}
""",
            ),
            (
                """\
enum tag { tag }
function use(x: tag) returns (tag) {
  match (x) {
    case comptime tag { return tag; }
    default { return tag; }
  }
}
""",
                """\
enum tag { tag }
function use(x: tag) returns (tag) {
  match (x) {
    case comptime tag.tag { return tag.tag; }
    default { return tag.tag; }
  }
}
""",
            ),
        ]

        for source, expected in cases:
            with self.subTest(source=source):
                migrated = MIGRATE.migrate_source(source)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_match_constructor_heads_ignore_value_shadowing(self) -> None:
        cases = [
            (
                """\
enum T { T }
function use(x: T, T: Callback) {
  match (x) { case T { T(); } }
}
""",
                """\
enum T { T }
function use(x: T, T: Callback) {
  match (x) { case T.T { T(); } }
}
""",
            ),
            (
                """\
enum T { T(word) }
function use(x: T, T: Callback) {
  match (x) { case T(y) { T(y); } }
}
""",
                """\
enum T { T(word) }
function use(x: T, T: Callback) {
  match (x) { case T.T(y) { T(y); } }
}
""",
            ),
            (
                """\
enum t { t(word) }
function use(x: t) {
  let t = callback;
  match (x) { case t(y) { t(y); } }
}
""",
                """\
enum t { t(word) }
function use(x: t) {
  let t = callback;
  match (x) { case t.t(y) { t(y); } }
}
""",
            ),
        ]

        for source, expected in cases:
            with self.subTest(source=source):
                migrated = MIGRATE.migrate_source(source)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_contract_local_constructor_owners_stay_in_their_contract(
        self,
    ) -> None:
        source = """\
function outsideBefore(x: word) returns (word) { return T(x); }
contract A {
  function before(x: word) returns (T) { return T(x); }
  enum T { T(word) }
  function after(x: word) returns (T) { return T(x); }
}
contract B {
  function sibling(x: word) returns (word) { return T(x); }
}
function outsideAfter(x: word) returns (word) { return T(x); }
"""
        expected = """\
function outsideBefore(x: word) returns (word) { return T(x); }
contract A {
  function before(x: word) returns (T) { return T.T(x); }
  enum T { T(word) }
  function after(x: word) returns (T) { return T.T(x); }
}
contract B {
  function sibling(x: word) returns (word) { return T(x); }
}
function outsideAfter(x: word) returns (word) { return T(x); }
"""
        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_external_contract_method_does_not_shadow_local_owner(
        self,
    ) -> None:
        source = """\
contract A {
  enum T { T(word) }
  function T(x: word) external returns (word);
  function make(x: word) returns (T) { return T(x); }
}
"""
        expected = """\
contract A {
  enum T { T(word) }
  function T(x: word) external returns (word);
  function make(x: word) returns (T) { return T.T(x); }
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_function_type_external_does_not_hide_internal_method(
        self,
    ) -> None:
        source = """\
contract A {
  enum T { T(word) }
  function T(cb: function(word) external returns (word))
    returns (word) { return 0; }
  function make(x: word) returns (T) { return T(x); }
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, source)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_generic_function_type_external_does_not_hide_method(
        self,
    ) -> None:
        source = """\
enum Box<a> { Box(a) }
trait C<a> {}
contract A {
  enum T { T(word) }
  function T<a>(x: a) returns (word)
    where Box<function(word) external returns (word)>: C { return 0; }
  function make(x: word) returns (T) { return T(x); }
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, source)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_where_subject_external_does_not_hide_public_method(
        self,
    ) -> None:
        source = """\
trait C<a> {}
contract A {
  enum T { T(word) }
  function T(x: word) public returns (word)
    where function(word) external returns (word): C { return 0; }
  function make(x: word) returns (T) { return T(x); }
}
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, source)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_contract_local_struct_owner_stays_in_its_library(self) -> None:
        source = """\
library L {
  function before(x: word) returns (T) { return T(x); }
  struct T { value: word; }
  function after(x: word) returns (T) { return T(x); }
}
function outside(x: word) returns (word) { return T(x); }
"""
        expected = """\
library L {
  function before(x: word) returns (T) { return T.T(x); }
  struct T { value: word; }
  function after(x: word) returns (T) { return T.T(x); }
}
function outside(x: word) returns (word) { return T(x); }
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_module_constructor_owner_is_visible_in_contracts(self) -> None:
        source = """\
enum T { T(word) }
contract T {
  function value(x: word) returns (word) { return x; }
}
contract A {
  alias T = word;
  function inside(x: word) returns (word) { return T(x); }
}
function outside(x: word) returns (T) { return T(x); }
"""
        expected = """\
enum T { T(word) }
contract T {
  function value(x: word) returns (word) { return x; }
}
contract A {
  alias T = word;
  function inside(x: word) returns (word) { return T.T(x); }
}
function outside(x: word) returns (T) { return T.T(x); }
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_contract_local_owners_do_not_seed_other_sources(self) -> None:
        source = """\
contract A {
  enum T { T(word) }
  enum Option { Some(word) }
}
"""

        self.assertEqual(
            MIGRATE.collect_global_constructor_owners([source]),
            {},
        )
        self.assertEqual(
            MIGRATE.collect_global_dot_constructor_candidates([source]),
            {},
        )

    def test_dot_constructor_owners_respect_contract_scope(self) -> None:
        source = """\
enum Global { Other(word) }
contract A {
  enum Local { Some(word) }
  function inside(x: word) returns (Local) { return .Some(x); }
}
function outside(x: word) returns (Global) { return .Other(x); }
"""
        expected = """\
enum Global { Other(word) }
contract A {
  enum Local { Some(word) }
  function inside(x: word) returns (Local) { return Local.Some(x); }
}
function outside(x: word) returns (Global) { return Global.Other(x); }
"""

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_dot_constructor_rejects_visible_module_and_local_owners(
        self,
    ) -> None:
        source = """\
enum Global { Some(word) }
contract A {
  enum Local { Some(word) }
  function inside(x: word) returns (Global) { return .Some(x); }
}
"""

        with self.assertRaisesRegex(
            ValueError,
            r"ambiguous legacy dot-constructor \.Some.*Global, Local",
        ):
            MIGRATE.migrate_source(source)

    def test_preserves_calls_and_prose_in_rust_literals(self) -> None:
        rust = r'''
const SOURCE: &str = r#"function uint256(x: word) returns (word) {
  return uint256(x);
}"#;
const TYPE_NAMESPACES: &str = r#"alias Amount = word;
type Wad is word;
function use(x: word) {
  let builtin = word(x);
  let aliasCall = Amount(x);
  let valueTypeCall = Wad(x);
}"#;
const PROSE: &str = "Use uint256(value) when porting Solidity.";
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 2)
        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)


class RustStringMigrationTests(unittest.TestCase):
    def assert_rust_syntax(self, source: str) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "embedded.rs"
            output = root / "embedded.rmeta"
            path.write_text(source)
            checked = subprocess.run(
                [
                    "rustc",
                    "--crate-name",
                    "embedded_migration_test",
                    "--crate-type",
                    "lib",
                    "--emit",
                    "metadata",
                    "-o",
                    str(output),
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_nested_block_comment_can_keep_a_rust_file_unmigrated(self) -> None:
        rust = """\
/* outer /* inner */ migrate-syntax: keep-rust-file */
const SOURCE: &str =
    r#"function f(x: word) -> word { return x; }"#;
"""

        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)

    def test_marker_text_in_a_raw_string_does_not_keep_the_rust_file(
        self,
    ) -> None:
        rust = '''\
const PROSE: &str =
    r##"quoted " /* migrate-syntax: keep-rust-file */"##;
const SOURCE: &str =
    r#"function f(x: word) -> word { return x; }"#;
'''

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn("migrate-syntax: keep-rust-file", migrated)
        self.assertIn(
            "function f(x: word) returns (word)",
            migrated,
        )

    def test_preserves_prose_format_strings_and_sql(self) -> None:
        rust = r'''
const HELP: &str = r#"prefer function f(x: T) -> T syntax"#;
const DIAGNOSTIC: &str = "expected function f(x: T) -> T";
const FORMAT: &str = r#"function f(x: T) -> T {{ return {value}; }}"#;
const TYPE_FORMAT: &str = r#"type {name} = {ty};"#;
const IMPORT_FORMAT: &str = r#"import {{name}} from {path};"#;
const IMPORT_PROSE: &str = r#"run import foo.bar; to continue"#;
const DECLARATION_TEMPLATE: &str =
    "function f({keyword}: word) { let {keyword}: word = 0; }";
const FIELD_TEMPLATE: &str = "struct S { {keyword}: word; }";
const UPPER_TEMPLATE: &str =
    "function f({TYPE}: word) -> word { return 0; }";
const POSITIONAL_TEMPLATE: &str =
    "function f(x: {0}) -> word { return 0; }";
const FORMAT_SPEC_TEMPLATE: &str =
    "function f(x: word) -> word { return {name:?}; }";
const SQL: &str =
    r#"select type, alias, comptime from function where match = ?;"#;
const SQL_IMPORT: &str = r#"import records from audit;"#;
'''

        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)
        self.assertEqual(MIGRATE._rust_solcore_literal_spans(rust), [])

    def test_template_holes_exclude_doubled_format_braces(self) -> None:
        for template in ("{TYPE}", "{0}", "{name:?}"):
            with self.subTest(template=template):
                self.assertIsNotNone(
                    MIGRATE._RUST_TEMPLATE_HOLE_RE.search(template)
                )
        for escaped in ("{{}}", "{{TYPE}}", "{{0}}", "{{name:?}}"):
            with self.subTest(escaped=escaped):
                self.assertIsNone(
                    MIGRATE._RUST_TEMPLATE_HOLE_RE.search(escaped)
                )

    def test_recognizes_and_migrates_structured_imports(self) -> None:
        rust = r'''
const BARE: &str = r#"import foo.bar;"#;
const NAMESPACE: &str = r#"import * as bar from foo.bar;"#;
const SELECTIVE: &str = r#"import {Thing, value as renamed} from foo;"#;
const CLASSIC_SELECTIVE: &str =
    r#"import foo.{Thing, value as renamed};"#;
const PACKAGE: &str = r#"import {Thing} from @ext.foo;"#;
const RUST_ESCAPES: &str = "import {Thing} from foo;\x20\n";
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 6)

        migrated = MIGRATE.migrate_rust_strings(
            rust,
            classic_bare_imports=True,
        )

        self.assertIn("import * as bar from foo.bar;", migrated)
        self.assertIn(
            "import {Thing, value as renamed} from foo;",
            migrated,
        )
        self.assertIn("import {Thing} from @ext.foo;", migrated)
        self.assertEqual(
            MIGRATE.migrate_rust_strings(
                migrated,
                classic_bare_imports=True,
            ),
            migrated,
        )

    def test_migrates_raw_classic_fragments_idempotently(self) -> None:
        rust = r'''
const FUNCTION: &str =
    r#"function f(x: word) -> word { return x; }"#;
const MATCH: &str = r#"match x { | _ => 0; }"#;
const ALIAS: &str = r#"type Amount = word;"#;
const COMPTIME: &str = r#"let x : comptime word = 1;"#;
const CONTRACT: &str =
    r#"contract C { public function get() -> word { return 1; } }"#;
const PRAGMA: &str = r#"pragma no-coverage-condition;"#;
const GENERIC_FUNCTION: &str =
    r#"forall T. function id(x: T) -> T { return x; }"#;
const CLASS: &str = r#"forall T. class T: Eq {}"#;
const INSTANCE: &str = r#"forall T. instance T: Eq {}"#;
const COMMENTED: &str =
    r#"/* outer /* inner */ done */ function commented() -> word { return 1; }"#;
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 10)

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn(
            "function f(x: word) returns (word) { return x; }",
            migrated,
        )
        self.assertIn("match (x)", migrated)
        self.assertIn("alias Amount = word;", migrated)
        self.assertIn("let comptime x: word = 1;", migrated)
        self.assertIn("function get() public returns (word)", migrated)
        self.assertIn("pragma solcore noCoverageCondition;", migrated)
        self.assertIn(
            "function id<T>(x: T) returns (T) { return x; }",
            migrated,
        )
        self.assertIn("trait Eq<T> {}", migrated)
        self.assertIn("impl<T> Eq<T> {}", migrated)
        self.assertIn("function commented() returns (word)", migrated)
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)

    def test_migrates_isolated_typed_lets_with_classic_types(self) -> None:
        rust = r'''
const RAW: &str =
    r#"let x /* binding */: Option(/* argument */ word) = value;"#;
const ORDINARY: &str =
    "let table: mapping(word, Option(word)) = value;";
const NESTED_FN_IDENTIFIER: &str =
    r#"let nested: Option(fn) = value;"#;
const NESTED_FN_APPLICATION: &str =
    r#"let nestedCallback: Option(fn(word)) = value;"#;
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 4)

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn(
            "let x /* binding */ : Option</* argument */ word> = value;",
            migrated,
        )
        self.assertIn(
            "let table: mapping(word => Option<word>) = value;",
            migrated,
        )
        self.assertIn("let nested: Option<fn> = value;", migrated)
        self.assertIn(
            "let nestedCallback: Option<fn<word>> = value;",
            migrated,
        )
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)
        self.assert_rust_syntax(migrated)

    def test_does_not_treat_typed_let_prose_as_embedded_source(self) -> None:
        rust = r'''
const MULTIWORD: &str =
    r#"let us consider x: Option(word) = value;"#;
const PREFIXED: &str =
    r#"Use let x: Option(word) = value;"#;
const TRAILING: &str =
    r#"let x: Option(word) = value; in docs"#;
const CANONICAL: &str =
    r#"let x: Option<word> = value;"#;
const TEMPLATE: &str =
    r#"let {name}: Option(word) = value;"#;
const RUST_FN: &str =
    r#"let f: fn(u8) -> u8 = callback;"#;
const RUST_FN_UNIT: &str =
    r#"let f: fn(u8) = callback;"#;
const RUST_DYN: &str =
    r#"let f: &dyn Fn(u8) -> u8 = callback;"#;
'''

        self.assertEqual(MIGRATE._rust_solcore_literal_spans(rust), [])
        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)

    def test_rejects_malformed_types_in_isolated_typed_lets(self) -> None:
        cases = [
            r'const SOURCE: &str = r#"let x: A -> = value;"#;' + "\n",
            r'const SOURCE: &str = "let x: Box<(word] = value;";' + "\n",
        ]

        for rust in cases:
            with self.subTest(rust=rust):
                self.assertEqual(
                    len(MIGRATE._rust_solcore_literal_spans(rust)),
                    1,
                )
                with self.assertRaisesRegex(
                    ValueError,
                    "malformed Classic type arrow|malformed type delimiters",
                ):
                    MIGRATE.migrate_rust_strings(rust)

    def test_recognizes_canonical_fragments_without_rewriting(self) -> None:
        rust = r'''
const FUNCTION: &str =
    r#"function f(x: word) returns (word) { return x; }"#;
const MATCH: &str =
    r#"match (x) { case _ { return 0; } }"#;
const ALIAS: &str = r#"alias Amount = word;"#;
const COMPTIME: &str = r#"let comptime x: word = 1;"#;
const CONTRACT: &str =
    r#"contract C { function get() public returns (word) { return 1; } }"#;
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 5)
        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)

    def test_migrates_ordinary_escaped_string_and_preserves_escapes(self) -> None:
        rust = (
            'const SOURCE: &str = "function message() -> string '
            '{\\n  return \\"ok\\";\\n}";\n'
        )

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn(
            'function message() returns (string) {\\n'
            '  return \\"ok\\";\\n}',
            migrated,
        )
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)

    def test_migrates_rust_specific_escapes_semantically(self) -> None:
        rust = r'''
const FUNCTION: &str =
    "public function\x20f(x: word) -> word { return x; }";
const COMPTIME: &str = "let x : comptime\u{20}word = 1;";
const IMPORT: &str = "import \
    foo.{Thing};";
const PROSE: &str =
    "prefer function\x20f(x: T) -> T syntax";
'''

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn(
            '"function f(x: word) public returns (word) { return x; }"',
            migrated,
        )
        self.assertIn('"let comptime x: word = 1;"', migrated)
        self.assertIn('"import {Thing} from foo;"', migrated)
        self.assertIn(
            '"prefer function\\x20f(x: T) -> T syntax"',
            migrated,
        )
        self.assertNotIn("\\ x20", migrated)
        self.assertNotIn("\\ u{20}", migrated)
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)
        self.assert_rust_syntax(migrated)

    def test_cli_does_not_share_owners_between_rust_literals(self) -> None:
        source = r'''
const DECLARATION: &str =
    "data\x20Option(a) = None | Some(a);";
const USE: &str =
    "function wrap(x: word) -> Option(word) { return .Some(x); }";
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "embedded.rs"
            path.write_text(source)
            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--rust-strings",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = path.read_text()

        self.assertEqual(migration.returncode, 2)
        self.assertEqual(migrated, source)
        self.assertIn(
            "cannot resolve legacy dot-constructor .Some",
            migration.stderr,
        )

    def test_cli_rust_string_check_reports_then_reaches_fixed_point(self) -> None:
        source = r'''
const HELP: &str = r#"prefer function f(x: T) -> T syntax"#;
const SOURCE: &str =
    r#"function f(x: word) -> word { return x; }"#;
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "embedded.rs"
            path.write_text(source)

            needs_migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--check",
                    "--rust-strings",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            unchanged = path.read_text()
            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--rust-strings",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = path.read_text()
            clean = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--check",
                    "--rust-strings",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(needs_migration.returncode, 1, needs_migration.stderr)
        self.assertEqual(unchanged, source)
        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(clean.returncode, 0, clean.stderr)
        self.assertIn("1 file(s) need migration", needs_migration.stdout)
        self.assertIn("function f(x: word) returns (word)", migrated)
        self.assertIn("prefer function f(x: T) -> T syntax", migrated)
        self.assertIn("0 file(s) need migration", clean.stdout)

    def test_cli_deduplicates_relative_and_absolute_rust_paths(
        self,
    ) -> None:
        source = (
            'const SOURCE: &str = '
            'r#"function f(x: word) -> word { return x; }"#;\n'
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            path = root / "embedded.rs"
            path.write_text(source)

            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--rust-strings",
                    root.name,
                    str(path),
                ],
                cwd=root.parent,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = path.read_text()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertIn("function f(x: word) returns (word)", migrated)
        self.assertIn("1 file(s) examined", migration.stdout)


class AtomicCliMigrationTests(unittest.TestCase):
    def test_directory_discovery_excludes_special_source_paths(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.solc"
            fifo = root / "blocked.solc"
            source.write_text("function f() {}\n")
            MIGRATE.os.mkfifo(fifo)

            discovered = MIGRATE.source_paths([str(root)])
            with self.assertRaisesRegex(
                ValueError,
                r"not a Solcore source path",
            ):
                MIGRATE.source_paths([str(fifo)])

        self.assertEqual(discovered, [source])

    def test_directory_discovery_errors_before_cli_writes(self) -> None:
        source = "alias Good = A -> B;\n"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "good.solc"
            denied = root / "denied"
            hidden = denied / "hidden.solc"
            denied.mkdir()
            good.write_text(source)
            hidden.write_text("alias Hidden = A -> B;\n")
            real_scandir = MIGRATE.os.scandir

            def guarded_scandir(path: object) -> object:
                if Path(path) == denied:
                    raise PermissionError("permission denied by test")
                return real_scandir(path)

            with (
                mock.patch.object(
                    MIGRATE.os,
                    "scandir",
                    side_effect=guarded_scandir,
                ),
                mock.patch.object(
                    MIGRATE.sys,
                    "argv",
                    [str(SCRIPT), str(root)],
                ),
                mock.patch.object(
                    MIGRATE.argparse.ArgumentParser,
                    "error",
                    side_effect=RuntimeError("discovery failed"),
                ),
                self.assertRaisesRegex(
                    RuntimeError, r"discovery failed"
                ),
            ):
                MIGRATE.main()
            good_after = good.read_text()

        self.assertEqual(good_after, source)

    def test_cli_preserves_solcore_source_line_endings(self) -> None:
        for newline in (b"\r\n", b"\r"):
            with self.subTest(newline=newline):
                original = (
                    b"alias F = A -> B;"
                    + newline
                    + b"// keep"
                    + newline
                )
                expected = (
                    b"alias F = function(A) returns (B);"
                    + newline
                    + b"// keep"
                    + newline
                )
                with tempfile.TemporaryDirectory() as directory:
                    path = Path(directory) / "source.solc"
                    path.write_bytes(original)

                    migration = subprocess.run(
                        [sys.executable, str(SCRIPT), str(path)],
                        cwd=ROOT,
                        text=True,
                        capture_output=True,
                        check=False,
                    )
                    migrated = path.read_bytes()

                self.assertEqual(
                    migration.returncode, 0, migration.stderr
                )
                self.assertEqual(migrated, expected)

    def test_cli_preserves_rust_source_line_endings(self) -> None:
        original = (
            b'const SOURCE: &str = r#"alias F = A -> B;"#;\r\n'
            b"// keep\r\n"
        )
        expected = (
            b'const SOURCE: &str = '
            b'r#"alias F = function(A) returns (B);"#;\r\n'
            b"// keep\r\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "embedded.rs"
            path.write_bytes(original)

            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--rust-strings",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migrated = path.read_bytes()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(migrated, expected)

    def test_successful_batch_preserves_modes_and_cleans_backups(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "source.solc"
            path.write_text("original\n")
            path.chmod(0o640)

            MIGRATE.write_migrations_atomically(
                {path: path.read_bytes()},
                {path: "migrated\n"}
            )

            migrated = path.read_text()
            mode = path.stat().st_mode & 0o777
            names = [entry.name for entry in root.iterdir()]

        self.assertEqual(migrated, "migrated\n")
        self.assertEqual(mode, 0o640)
        self.assertEqual(names, ["source.solc"])

    def test_successful_batch_preserves_symlink_and_hardlink_identity(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target.solc"
            hardlink = root / "hardlink.solc"
            symlink = root / "symlink.solc"
            target.write_text("original\n")
            MIGRATE.os.link(target, hardlink)
            symlink.symlink_to(target.name)
            inode = target.stat().st_ino

            MIGRATE.write_migrations_atomically(
                {
                    target: target.read_bytes(),
                    hardlink: hardlink.read_bytes(),
                    symlink: symlink.read_bytes(),
                },
                {
                    target: "migrated\n",
                    hardlink: "migrated\n",
                    symlink: "migrated\n",
                },
            )

            target_after = target.read_text()
            hardlink_after = hardlink.read_text()
            symlink_after = symlink.read_text()
            remains_symlink = symlink.is_symlink()
            inode_after = target.stat().st_ino

        self.assertEqual(target_after, "migrated\n")
        self.assertEqual(hardlink_after, "migrated\n")
        self.assertEqual(symlink_after, "migrated\n")
        self.assertTrue(remains_symlink)
        self.assertEqual(inode_after, inode)

    def test_unchanged_read_only_target_does_not_block_batch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            changed = root / "changed.solc"
            unchanged = root / "unchanged.solc"
            changed.write_bytes(b"changed original\n")
            unchanged.write_bytes(b"unchanged original\n")
            unchanged.chmod(0o444)

            MIGRATE.write_migrations_atomically(
                {
                    changed: b"changed original\n",
                    unchanged: b"unchanged original\n",
                },
                {changed: "changed migrated\n"},
            )

            changed_after = changed.read_bytes()
            unchanged_after = unchanged.read_bytes()
            unchanged_mode = unchanged.stat().st_mode & 0o777

        self.assertEqual(changed_after, b"changed migrated\n")
        self.assertEqual(unchanged_after, b"unchanged original\n")
        self.assertEqual(unchanged_mode, 0o444)

    def test_source_batch_validation_failure_writes_nothing(self) -> None:
        good_source = "alias Good = A -> B;\n"
        bad_source = "alias Bad = A ->;\n"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "a_good.solc"
            bad = root / "z_bad.solc"
            good.write_text(good_source)
            bad.write_text(bad_source)

            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(root)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )

            good_after = good.read_text()
            bad_after = bad.read_text()

        self.assertEqual(migration.returncode, 2)
        self.assertEqual(good_after, good_source)
        self.assertEqual(bad_after, bad_source)
        self.assertIn("0 file(s) migrated", migration.stdout)
        self.assertIn(str(bad), migration.stderr)

    def test_rust_batch_validation_failure_writes_nothing(self) -> None:
        good_source = (
            'const SOURCE: &str = r#"alias Good = A -> B;"#;\n'
        )
        bad_source = (
            'const SOURCE: &str = r#"alias Bad = A ->;"#;\n'
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "a_good.rs"
            bad = root / "z_bad.rs"
            good.write_text(good_source)
            bad.write_text(bad_source)

            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--rust-strings",
                    str(root),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )

            good_after = good.read_text()
            bad_after = bad.read_text()

        self.assertEqual(migration.returncode, 2)
        self.assertEqual(good_after, good_source)
        self.assertEqual(bad_after, bad_source)
        self.assertIn("0 file(s) migrated", migration.stdout)
        self.assertIn(str(bad), migration.stderr)

    def test_cli_honors_python_utf8_mode_for_source_encoding(
        self,
    ) -> None:
        source = "// 日本語\nalias Good = A -> B;\n"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.solc"
            path.write_text(source)
            environment = dict(MIGRATE.os.environ)
            environment.update({"LC_ALL": "C", "PYTHONUTF8": "1"})

            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
                env=environment,
            )
            migrated = path.read_text()

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertIn("// 日本語", migrated)
        self.assertIn("function(A) returns (B)", migrated)

    def test_write_failure_rolls_back_already_replaced_files(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first.solc"
            second = root / "second.solc"
            first_original = b"first original\r\n"
            second_original = b"second original\r\n"
            first.write_bytes(first_original)
            second.write_bytes(second_original)
            original_write = MIGRATE._write_binary_stream

            def fail_second_write(
                stream: object,
                source: bytes,
            ) -> None:
                if (
                    Path(stream.name) == second
                    and source == b"second migrated\n"
                ):
                    raise OSError("injected write failure")
                original_write(stream, source)

            with mock.patch.object(
                MIGRATE,
                "_write_binary_stream",
                side_effect=fail_second_write,
            ):
                with self.assertRaisesRegex(
                    OSError,
                    "atomic migration write failed",
                ):
                    MIGRATE.write_migrations_atomically(
                        {
                            first: first_original,
                            second: second_original,
                        },
                        {
                            first: "first migrated\n",
                            second: "second migrated\n",
                        }
                    )

            names = sorted(path.name for path in root.iterdir())
            first_after = first.read_bytes()
            second_after = second.read_bytes()

        self.assertEqual(first_after, first_original)
        self.assertEqual(second_after, second_original)
        self.assertEqual(names, ["first.solc", "second.solc"])

    def test_interrupt_rolls_back_already_replaced_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first.solc"
            second = root / "second.solc"
            first_original = b"first original\r\n"
            second_original = b"second original\r\n"
            first.write_bytes(first_original)
            second.write_bytes(second_original)
            original_write = MIGRATE._write_binary_stream

            def interrupt_second_write(
                stream: object,
                source: bytes,
            ) -> None:
                if (
                    Path(stream.name) == second
                    and source == b"second migrated\n"
                ):
                    raise KeyboardInterrupt
                original_write(stream, source)

            with mock.patch.object(
                MIGRATE,
                "_write_binary_stream",
                side_effect=interrupt_second_write,
            ):
                with self.assertRaises(KeyboardInterrupt):
                    MIGRATE.write_migrations_atomically(
                        {
                            first: first_original,
                            second: second_original,
                        },
                        {
                            first: "first migrated\n",
                            second: "second migrated\n",
                        },
                    )

            first_after = first.read_bytes()
            second_after = second.read_bytes()

        self.assertEqual(first_after, first_original)
        self.assertEqual(second_after, second_original)

    def test_rollback_does_not_overwrite_replaced_path_identity(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first.solc"
            moved = root / "first-moved.solc"
            second = root / "second.solc"
            first_original = b"first original\n"
            second_original = b"second original\n"
            external = b"external replacement\n"
            first.write_bytes(first_original)
            second.write_bytes(second_original)
            original_write = MIGRATE._write_binary_stream

            def replace_then_fail(
                stream: object,
                source: bytes,
            ) -> None:
                if (
                    Path(stream.name) == second
                    and source == b"second migrated\n"
                ):
                    MIGRATE.os.replace(first, moved)
                    first.write_bytes(external)
                    raise OSError("injected write failure")
                original_write(stream, source)

            with mock.patch.object(
                MIGRATE,
                "_write_binary_stream",
                side_effect=replace_then_fail,
            ):
                with self.assertRaisesRegex(
                    OSError,
                    "original preserved at",
                ):
                    MIGRATE.write_migrations_atomically(
                        {
                            first: first_original,
                            second: second_original,
                        },
                        {
                            first: "first migrated\n",
                            second: "second migrated\n",
                        },
                    )

            first_after = first.read_bytes()
            moved_after = moved.read_bytes()
            second_after = second.read_bytes()
            recovery = list(
                root.glob(".first.solc.migrate-recovery-*")
            )
            recovery_contents = [
                path.read_bytes() for path in recovery
            ]

        self.assertEqual(first_after, external)
        self.assertEqual(moved_after, b"first migrated\n")
        self.assertEqual(second_after, second_original)
        self.assertEqual(recovery_contents, [first_original])

    def test_rollback_does_not_overwrite_external_in_place_edit(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first.solc"
            second = root / "second.solc"
            first_original = b"first original\n"
            second_original = b"second original\n"
            external = b"external in-place edit\n"
            first.write_bytes(first_original)
            second.write_bytes(second_original)
            original_write = MIGRATE._write_binary_stream

            def edit_then_fail(
                stream: object,
                source: bytes,
            ) -> None:
                if (
                    Path(stream.name) == second
                    and source == b"second migrated\n"
                ):
                    first.write_bytes(external)
                    raise OSError("injected write failure")
                original_write(stream, source)

            with mock.patch.object(
                MIGRATE,
                "_write_binary_stream",
                side_effect=edit_then_fail,
            ):
                with self.assertRaisesRegex(
                    OSError,
                    "original preserved at",
                ):
                    MIGRATE.write_migrations_atomically(
                        {
                            first: first_original,
                            second: second_original,
                        },
                        {
                            first: "first migrated\n",
                            second: "second migrated\n",
                        },
                    )

            first_after = first.read_bytes()
            second_after = second.read_bytes()
            recovery = list(
                root.glob(".first.solc.migrate-recovery-*")
            )
            recovery_contents = [
                path.read_bytes() for path in recovery
            ]

        self.assertEqual(first_after, external)
        self.assertEqual(second_after, second_original)
        self.assertEqual(recovery_contents, [first_original])

    def test_external_edit_aborts_before_any_batch_write(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            changed = root / "changed.solc"
            unchanged = root / "unchanged.solc"
            expected_changed = b"changed original\n"
            expected_unchanged = b"unchanged original\n"
            external = b"external edit\n"
            changed.write_bytes(expected_changed)
            unchanged.write_bytes(expected_unchanged)
            planned = {
                changed: expected_changed,
                unchanged: expected_unchanged,
            }
            unchanged.write_bytes(external)

            with self.assertRaisesRegex(
                OSError,
                "source changed after migration planning",
            ):
                MIGRATE.write_migrations_atomically(
                    planned,
                    {changed: "migrated\n"},
                )

            changed_after = changed.read_bytes()
            unchanged_after = unchanged.read_bytes()

        self.assertEqual(changed_after, expected_changed)
        self.assertEqual(unchanged_after, external)

    def test_import_provider_is_part_of_the_atomic_snapshot(self) -> None:
        provider_source = (
            "enum T { T(word) }\nexport {T(*)};\n"
        )
        consumer_source = (
            "import provider;\n"
            "function make(x: word) returns (T) { return T(x); }\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            provider = root / "provider.solc"
            consumer = root / "main.solc"
            provider.write_text(provider_source)
            consumer.write_text(consumer_source)
            expected = {
                provider: provider.read_bytes(),
                consumer: consumer.read_bytes(),
            }
            sources = {
                provider: provider_source,
                consumer: consumer_source,
            }
            surfaces = MIGRATE.build_constructor_import_surfaces(sources)
            migrated = MIGRATE.migrate_source(
                consumer_source,
                constructor_import_surface=surfaces[consumer],
            )

            provider.write_text(provider_source + "// concurrent edit\n")
            with self.assertRaisesRegex(
                OSError,
                "source changed after migration planning",
            ):
                MIGRATE.write_migrations_atomically(
                    expected,
                    {consumer: migrated},
                )
            consumer_after = consumer.read_text()

        self.assertEqual(consumer_after, consumer_source)

    def test_recovery_staging_rejects_a_short_write(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "source.solc"
            path.write_bytes(b"original")
            original_fdopen = MIGRATE.os.fdopen

            class ShortWriter:
                def __init__(self, file_descriptor: int, mode: str) -> None:
                    self.inner = original_fdopen(file_descriptor, mode)

                def __enter__(self) -> "ShortWriter":
                    self.inner.__enter__()
                    return self

                def __exit__(self, *args: object) -> object:
                    return self.inner.__exit__(*args)

                def write(self, source: bytes) -> int:
                    self.inner.write(source[:-1])
                    return len(source) - 1

                def __getattr__(self, name: str) -> object:
                    return getattr(self.inner, name)

            with mock.patch.object(
                MIGRATE.os,
                "fdopen",
                side_effect=ShortWriter,
            ):
                with self.assertRaisesRegex(
                    OSError,
                    "short recovery write",
                ):
                    MIGRATE._stage_atomic_bytes(
                        path,
                        b"original",
                        0o600,
                        label="recovery",
                    )

            names = [entry.name for entry in root.iterdir()]

        self.assertEqual(names, ["source.solc"])

    def test_recovery_staging_cleans_up_after_interrupt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "source.solc"
            path.write_bytes(b"original")
            original_fdopen = MIGRATE.os.fdopen

            class InterruptWriter:
                def __init__(self, file_descriptor: int, mode: str) -> None:
                    self.inner = original_fdopen(file_descriptor, mode)

                def __enter__(self) -> "InterruptWriter":
                    self.inner.__enter__()
                    return self

                def __exit__(self, *args: object) -> object:
                    return self.inner.__exit__(*args)

                def write(self, source: bytes) -> int:
                    self.inner.write(source[:2])
                    raise KeyboardInterrupt

                def __getattr__(self, name: str) -> object:
                    return getattr(self.inner, name)

            with mock.patch.object(
                MIGRATE.os,
                "fdopen",
                side_effect=InterruptWriter,
            ):
                with self.assertRaises(KeyboardInterrupt):
                    MIGRATE._stage_atomic_bytes(
                        path,
                        b"original",
                        0o600,
                        label="recovery",
                    )

            names = [entry.name for entry in root.iterdir()]

        self.assertEqual(names, ["source.solc"])


class CommentPreservationMigrationTests(unittest.TestCase):
    def test_keeps_function_header_block_comments_at_their_tokens(self) -> None:
        classic = """\
public /* visibility */ function f(/* parameter */ x: Option(word))
  -> /* result */ word { return 0; }
"""
        expected = """\
function f(/* parameter */ x: Option<word>) public /* visibility */ returns (/* result */ word) { return 0; }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_line_comments_from_commenting_out_moved_header_tokens(self) -> None:
        classic = """\
public // visibility
function f(// parameter
  x: Option(word)) -> // result
  word { return 0; }
"""
        expected = """\
function f(// parameter
  x: Option<word>) public // visibility
 returns (// result
  word) { return 0; }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_comments_anchored_across_shared_header_rewrites(self) -> None:
        cases = [
            (
                "data /* kind */ Option(a) = None | Some(/* payload */ a);\n",
                "enum /* kind */ Option<a> { None, Some(/* payload */ a) }\n",
            ),
            (
                "type /* kind */ Amount = Option(/* argument */ word);\n",
                "alias /* kind */ Amount = Option</* argument */ word>;\n",
            ),
            (
                "forall T. class /* kind */ T: Eq {}\n",
                "trait /* kind */ Eq<T> {}\n",
            ),
            (
                "forall T. class T /* lhs */ : Eq {}\n",
                "trait Eq<T /* lhs */ > {}\n",
            ),
            (
                "forall T. instance /* kind */ T: Eq {}\n",
                "impl /* kind */ <T> Eq<T> {}\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_context_comments_inside_trait_and_impl_headers(self) -> None:
        cases = [
            (
                "forall a. (a: Show) => /* context */ class a: Eq {}\n",
                "trait /* context */ Eq<a> where a: Show {}\n",
            ),
            (
                "forall a. (a: Show) => /* context */ instance a: Eq {}\n",
                "impl /* context */ <a> Eq<a> where a: Show {}\n",
            ),
            (
                "(a: Show) => /* context */ instance a: Eq {}\n",
                "impl /* context */ Eq<a> where a: Show {}\n",
            ),
            (
                "forall a,b. (a: Show(b), /* second */ b: Eq)"
                " => instance a: Foo(pair(b,a), a) {}\n",
                "impl<a, b> Foo<a, pair<b, a>, a>"
                " where a: Show<b>, /* second */ b: Eq {}\n",
            ),
            (
                "forall a,b. (a: Show(pair(/* nested */ b,a)), b: Eq)"
                " => instance a: Foo(pair(b,a), a) {}\n",
                "impl<a, b> Foo<a, pair<b, a>, a>"
                " where a: Show<pair</* nested */ b, a>>, b: Eq {}\n",
            ),
            (
                "forall a,b. (a: Show(pair(// nested\n b,a)), b: Eq)"
                " => instance a: Foo(pair(b,a), a) {}\n",
                "impl<a, b> Foo<a, pair<b, a>, a>"
                " where a: Show<pair<// nested\n b, a>>, b: Eq {}\n",
            ),
            (
                "forall a. (a: Show(Option(/* context */ a)))"
                " => function f(x: Option(a)) -> Option(a) { return x; }\n",
                "function f<a>(x: Option<a>) returns (Option<a>)"
                " where a: Show<Option</* context */ a>> { return x; }\n",
            ),
            (
                "forall a. (a: Show) /* end */ => class a: Eq {}\n",
                "trait Eq<a> where a: Show /* end */ {}\n",
            ),
            (
                "forall a. (a: Show) /* end */ => instance a: Eq {}\n",
                "impl<a> Eq<a> where a: Show /* end */ {}\n",
            ),
            (
                "forall a. (a: Show) // end\n => class a: Eq {}\n",
                "trait Eq<a> where a: Show // end\n  {}\n",
            ),
            (
                "forall a. (memory /* context-location */ (a)"
                " /* context-close */ : Show)"
                " => function f(x: memory /* param-location */ (a)"
                " /* param-close */)"
                " -> memory /* result-location */ (a)"
                " /* result-close */ { return x; }\n",
                "function f<a>(x: a memory /* param-location */"
                "  /* param-close */ )"
                " returns (a memory /* result-location */"
                "  /* result-close */ )"
                " where a memory /* context-location */"
                "  /* context-close */ : Show { return x; }\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_comments_with_inserted_trait_head_arguments(self) -> None:
        cases = [
            (
                "forall a,b. class a: Eq(/* argument */ b) {}\n",
                "trait Eq<a, /* argument */ b> {}\n",
            ),
            (
                "forall a,b. instance a: Eq(/* argument */ b) {}\n",
                "impl<a, b> Eq<a, /* argument */ b> {}\n",
            ),
            (
                "forall a. instance a /* lhs */ : Eq {}\n",
                "impl<a> Eq<a /* lhs */ > {}\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_type_application_comments_out_of_return_types(self) -> None:
        classic = """\
function f(
  left: Option(/* payload */ word),
  right: pair(/* first */ word, /* second */ word)
) -> Result(/* result-arg */ word) { return 0; }
"""
        expected = """\
function f(left: Option</* payload */ word>, right: pair</* first */ word, /* second */ word>) returns (Result</* result-arg */ word>) { return 0; }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_removed_location_wrapper_comments_with_the_payload(self) -> None:
        cases = [
            (
                "function f(x: memory(/* param */ bytes)) -> word { return 0; }\n",
                "function f(x: /* param */ bytes memory) returns (word) { return 0; }\n",
            ),
            (
                "function f(x: memory(// param\n  bytes)) -> word { return 0; }\n",
                "function f(x: // param\n  bytes memory) returns (word) { return 0; }\n",
            ),
            (
                "function f(x: memory /* x */ (bytes),"
                " y: memory /* y */ (bytes))"
                " -> memory /* result */ (bytes) { return x; }\n",
                "function f(x: bytes memory /* x */ ,"
                " y: bytes memory /* y */ )"
                " returns (bytes memory /* result */ ) { return x; }\n",
            ),
            (
                "function f(x: memory // x\n (bytes),"
                " y: memory // y\n (bytes))"
                " -> memory // result\n (bytes) { return x; }\n",
                "function f(x: bytes memory // x\n ,"
                " y: bytes memory // y\n )"
                " returns (bytes memory // result\n ) { return x; }\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_comments_after_forall_binders_out_of_leading_docs(self) -> None:
        cases = [
            (
                "forall T. /* function binder */ function f(x: T) -> T { return x; }\n",
                "function f<T> /* function binder */ (x: T) returns (T) { return x; }\n",
            ),
            (
                "forall T. /* trait binder */ class T: Eq {}\n",
                "trait Eq<T> /* trait binder */ {}\n",
            ),
            (
                "forall T. /* impl binder */ instance T: Eq {}\n",
                "impl<T> /* impl binder */ Eq<T> {}\n",
            ),
            (
                "forall /* binder */ T. function f(x: T) -> T { return x; }\n",
                "function f</* binder */ T>(x: T) returns (T) { return x; }\n",
            ),
            (
                "forall /* binder */ T. class T: Eq {}\n",
                "trait Eq</* binder */ T> {}\n",
            ),
            (
                "forall /* binder */ T. instance T: Eq {}\n",
                "impl</* binder */ T> Eq<T> {}\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_independent_prefix_comments_out_of_leading_docs(self) -> None:
        cases = [
            (
                "forall T.\n// after-dot\nfunction f(x: T) -> T { return x; }\n",
                "function f<T> // after-dot\n(x: T) returns (T) { return x; }\n",
            ),
            (
                "public\n/* visibility */\nfunction f() -> word { return 0; }\n",
                "function f() public /* visibility */\n returns (word) { return 0; }\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_special_function_and_lambda_header_comments(self) -> None:
        cases = [
            (
                "payable /* mutability */"
                " constructor(/* parameter */ x: Option(word)) {}\n",
                "constructor(/* parameter */ x: Option<word>)"
                " payable /* mutability */ {}\n",
            ),
            (
                "external fallback(/* empty */) payable {}\n",
                "fallback(/* empty */ ) external payable {}\n",
            ),
            (
                "lam (/* parameter */ x: Option(word))"
                " -> /* result */ word { x }\n",
                "lam (/* parameter */ x: Option<word>)"
                " returns (/* result */ word) { x }\n",
            ),
            (
                "constructor() payable // comment\n {}\n",
                "constructor() payable // comment\n {}\n",
            ),
            (
                "fallback() external payable // comment\n {}\n",
                "fallback() external payable // comment\n {}\n",
            ),
            (
                "lam (x: word) // comment\n { x }\n",
                "lam (x: word) // comment\n { x }\n",
            ),
            (
                "payable // mutability\n"
                "constructor(x: Option(word)) {}\n",
                "constructor(x: Option<word>) payable // mutability\n"
                " {}\n",
            ),
            (
                "lam (x: Option(word)) // comment\n { x }\n",
                "lam (x: Option<word>) // comment\n  { x }\n",
            ),
            (
                "let f = lam (x: Option(/* arg */ word))"
                " returns (Result(/* result */ word)) { x };\n",
                "let f = lam (x: Option</* arg */ word>)"
                " returns (Result</* result */ word>) { x };\n",
            ),
            (
                "external /* visibility */ fallback() returns () {}\n",
                "fallback() external /* visibility */ returns () {}\n",
            ),
            (
                "let f = lam (x: Option(word))"
                " returns ((word, bool)) /* body */ { x };\n",
                "let f = lam (x: Option<word>)"
                " returns ((word, bool)) /* body */ { x };\n",
            ),
            (
                "external fallback() returns (()) /* body */ {}\n",
                "fallback() external returns (()) /* body */ {}\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_types_in_partially_canonical_function_headers(self) -> None:
        cases = [
            (
                "function f(x: Option(/* arg */ word)) {}\n",
                "function f(x: Option</* arg */ word>) {}\n",
            ),
            (
                "function f(x: Option(/* arg */ word))"
                " returns (Result(/* result */ word)) {}\n",
                "function f(x: Option</* arg */ word>)"
                " returns (Result</* result */ word>) {}\n",
            ),
            (
                "function f<T>(x: Option(/* arg */ word))"
                " where T: Eq(/* trait arg */ word) {}\n",
                "function f<T>(x: Option</* arg */ word>)"
                " where T: Eq</* trait arg */ word> {}\n",
            ),
            (
                "function f(x: Option(word))"
                " returns ((word, bool)) /* body */ {}\n",
                "function f(x: Option<word>)"
                " returns ((word, bool)) /* body */ {}\n",
            ),
            (
                "function f()"
                " returns (result: Option(/* arg */ word)) {}\n",
                "function f()"
                " returns (result: Option</* arg */ word>) {}\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_local_and_field_type_comments(self) -> None:
        cases = [
            (
                "function f(v: Option<word>) {"
                " let x /* binding */ : /* type */ Option(/* arg */ word) = v;"
                " }\n",
                "function f(v: Option<word>) {"
                " let x /* binding */ : /* type */ Option</* arg */ word> = v;"
                " }\n",
            ),
            (
                "function f(v: Option<word>) {"
                " let x /* binding */ : comptime /* type */"
                " Option(/* arg */ word) = v;"
                " }\n",
                "function f(v: Option<word>) {"
                " let comptime /* type */ x /* binding */ :"
                " Option</* arg */ word> = v;"
                " }\n",
            ),
            (
                "contract C { value: /* type */ Option(/* arg */ word); }\n",
                "contract C { value: /* type */ Option</* arg */ word>; }\n",
            ),
            (
                "contract C { value: /* before */ memory(bytes); }\n",
                "contract C { value: /* before */ bytes memory; }\n",
            ),
            (
                "contract C { value: // before\n memory(bytes); }\n",
                "contract C { value: // before\n  bytes memory; }\n",
            ),
            (
                "function f() { let result: comptime"
                " pair(word, Option(word)) /* trailing */ = value; }\n",
                "function f() { let comptime result:"
                " pair<word, Option<word>> /* trailing */ = value; }\n",
            ),
            (
                "contract C { result:"
                " pair(/* inner */ word, Option(word)) /* trailing */; }\n",
                "contract C { result:"
                " pair</* inner */ word, Option<word>> /* trailing */ ; }\n",
            ),
            (
                "function f(x: Outer<Inner<word> /* inner */ > /* outer */) {}\n",
                "function f(x: Outer<Inner<word> /* inner */ > /* outer */) {}\n",
            ),
            (
                "contract C { value: Outer<Inner<word> // inner\n"
                "> // outer\n; }\n",
                "contract C { value: Outer<Inner<word> // inner\n"
                "> // outer\n; }\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_pragma_import_and_annotation_comments(self) -> None:
        cases = [
            (
                "pragma /* rationale */ no-coverage-condition;\n",
                "pragma /* rationale */ solcore noCoverageCondition;\n",
            ),
            (
                "pragma no-generic-instance-for /* target */ Option;\n",
                "pragma solcore noGenericInstanceFor /* target */ Option;\n",
            ),
            (
                "import foo.{Thing, /* selected */ value as renamed};\n",
                "import {Thing, /* selected */ value as renamed} from foo;\n",
            ),
            (
                "import /* module */ foo.{Thing};\n",
                "import /* module */ {Thing} from foo;\n",
            ),
            (
                "import foo /* path */ .bar as foo /* alias */;\n",
                "import * as foo /* alias */ from foo /* path */ .bar;\n",
            ),
            (
                "import foo /* path */ .{Thing /* selector */};\n",
                "import {Thing /* selector */ } from foo /* path */ ;\n",
            ),
            (
                "import foo.{A /* one */, /* two */ B} hiding {B};\n",
                "import {A /* one */ } /* two */ from foo;\n",
            ),
            (
                "import foo /* one */ .{/* two */ *};\n",
                "import foo /* one */  /* two */ ;\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_comments_in_classic_bare_import_paths(self) -> None:
        classic = (
            "import foo./* segment */bar;"
            " function f() { foo./* use segment */bar.value(); }\n"
        )
        expected = (
            "import * as bar from foo. /* segment */ bar;"
            " function f() { bar /* use segment */ .value(); }\n"
        )

        migrated = MIGRATE.migrate_classic_bare_imports(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(
            MIGRATE.migrate_classic_bare_imports(migrated),
            migrated,
        )

    def test_keeps_generated_punctuation_outside_line_comments(self) -> None:
        cases = [
            (
                "function f(cond: bool) -> word {"
                " return if cond // condition\n then 1 else 2; }\n",
                "function f(cond: bool) returns (word) {"
                " return (cond // condition\n? 1 : 2); }\n",
            ),
            (
                "function f(cond: bool) -> word {"
                " return if cond then 1 // then\n else 2; }\n",
                "function f(cond: bool) returns (word) {"
                " return (cond ? 1 // then\n: 2); }\n",
            ),
            (
                "function f(cond: bool) -> word {"
                " return if cond then 1 else 2 // else\n ; }\n",
                "function f(cond: bool) returns (word) {"
                " return (cond ? 1 : 2 // else\n); }\n",
            ),
            (
                "function f(cond: bool) { if cond // condition\n {} }\n",
                "function f(cond: bool) { if (cond // condition\n) {} }\n",
            ),
            (
                "function f(cond: bool) { while cond // condition\n {} }\n",
                "function f(cond: bool) { while (cond // condition\n) {} }\n",
            ),
            (
                "function f(v: Option<word>) {"
                " match v // scrutinee\n { | _ => 0 } }\n",
                "function f(v: Option<word>) { match (v // scrutinee\n) {\n"
                "default {\n0\n}\n} }\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_match_arm_pattern_comments(self) -> None:
        classic = """\
data Option(a) = None | Some(a);
function f(v: Option(word)) -> word {
  match v { | /* arm */ .Some(/* bind */ x) => x | .None /* none */ => 0 }
}
"""
        expected = """\
enum Option<a> { None, Some(a) }
function f(v: Option<word>) returns (word) {
  match (v) {
case /* arm */ Option.Some(/* bind */ x) {
x
}
case Option.None /* none */  {
0
}
}
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)


class ExpressionHeaderBoundaryMigrationTests(unittest.TestCase):
    def test_migrates_less_than_conditions_and_match_scrutinees(self) -> None:
        classic = """\
function compare(x: word, y: word) {
  if x < y { return; }
  while x < y { break; }
  match x < y {
    | true => return;
    | false => return;
  }
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertIn("if (x < y) {", migrated)
        self.assertIn("while (x < y) {", migrated)
        self.assertIn("match (x < y) {", migrated)
        self.assertIn("case true {", migrated)
        self.assertIn("case false {", migrated)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_less_than_match_in_isolated_rust_literals(self) -> None:
        rust = r'''
const RAW: &str =
    r#"match x < y { | true => 1 | false => 0 }"#;
const ORDINARY: &str =
    "match x < y { | true => 1 | false => 0 }";
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 2)
        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertEqual(migrated.count("match (x < y)"), 2)
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)


class ExpressionAnnotationMigrationTests(unittest.TestCase):
    def test_moves_complete_let_initializer_annotations_to_bindings(
        self,
    ) -> None:
        classic = """\
function annotate(c: bool) {
  let simple = value : word;
  let (left, right) = pairValue : (word, bool);
  let choice = c ? left : right : Option(word);
  let callback = value : word -> bool;
  let table = value : mapping(word, Option(word));
  let buffer = value : memory(bytes);
  let powered = value : comptime Option(word);
}
"""
        expected = """\
function annotate(c: bool) {
  let simple: word = value;
  let (left, right): (word, bool) = pairValue;
  let choice: Option<word> = c ? left : right;
  let callback: function(word) returns (bool) = value;
  let table: mapping(word => Option<word>) = value;
  let buffer: bytes memory = value;
  let comptime powered: Option<word> = value;
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_annotation_comments_when_reordering_a_let(self) -> None:
        classic = (
            "function f() {"
            " let x /* binding */ = value /* operand */"
            " : /* type */ Box(word) /* tail */;"
            " }\n"
        )
        expected = (
            "function f() {"
            " let x /* binding */ : /* type */ Box<word> /* tail */"
            " = value /* operand */ ;"
            " }\n"
        )

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_line_endings_and_line_comments(self) -> None:
        classic = (
            "function f() {\r\n"
            "  let x = value // operand\r\n"
            "    : // type\r\n"
            "    Option(word) // tail\r\n"
            "    ;\r\n"
            "}\r\n"
        )

        migrated = MIGRATE.migrate_source(classic)

        self.assertIn("let x: // type\r\n", migrated)
        self.assertIn("Option<word> // tail\r\n", migrated)
        self.assertIn("= value // operand\r\n", migrated)
        self.assertNotIn("\n", migrated.replace("\r\n", ""))
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_isolated_let_annotations_in_rust_literals(self) -> None:
        rust = r'''
const RAW: &str = r#"let x = value : Option(word);"#;
const ORDINARY: &str =
    "let choice = c ? left : right : word;";
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 2)
        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn('r#"let x: Option<word> = value;"#', migrated)
        self.assertIn(
            '"let choice: word = c ? left : right;"',
            migrated,
        )
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)

    def test_rejects_annotations_that_need_manual_typed_bindings(self) -> None:
        cases = [
            "function f() { return value : word; }\n",
            "function f() { call(value : word); }\n",
            "function f() { value : word; }\n",
            "function f() { target = value : word; }\n",
            "function f() { target : word = value; }\n",
            "function f() { if value : bool {} }\n",
            "function f() { match value : word { | _ => return; } }\n",
            "function f() { let x = call(value : word); }\n",
            "contract C { value: word = source : word; }\n",
            "function f() { let x: word = value : bool; }\n",
            "function f() { let x = value : word : word; }\n",
            (
                "function f() {"
                " let (x, y) = value : comptime (word, word);"
                " }\n"
            ),
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot safely migrate Classic expression annotation",
                ):
                    MIGRATE.migrate_source(classic)

    def test_keeps_canonical_conversions_bindings_and_ternaries(self) -> None:
        canonical = """\
function f(c: bool) {
  let typed: word = value;
  let converted = value as word;
  let choice = c ? left : right;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)


class ComptimeLetMigrationTests(unittest.TestCase):
    def test_rejects_comptime_tuple_destructuring(self) -> None:
        cases = [
            (
                "function f(value: (word, word)) {"
                " let (x, y): comptime (word, word) = value;"
                " }\n"
            ),
            (
                "function f(value: (word, word)) {"
                " let comptime (x, y): (word, word) = value;"
                " }\n"
            ),
        ]

        for source in cases:
            with self.subTest(source=source):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate comptime tuple destructuring",
                ):
                    MIGRATE.migrate_source(source)

    def test_keeps_scalar_comptime_and_runtime_tuple_bindings(self) -> None:
        canonical = """\
function f(pairValue: (word, word), value: word) {
  let comptime scalar: word = value;
  let (left, right): (word, word) = pairValue;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_comptime_tuple_in_rust_source_literals(self) -> None:
        rust = (
            'const SOURCE: &str = r#"let (x, y): '
            'comptime (word, word) = pairValue;"#;\n'
        )

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 1)
        with self.assertRaisesRegex(
            ValueError,
            "cannot migrate comptime tuple destructuring",
        ):
            MIGRATE.migrate_rust_strings(rust)

    def test_preserves_nested_comptime_type_layers_and_comments(self) -> None:
        classic = """\
function f() {
  let x: comptime comptime word = y;
  let z: comptime /* outer */ comptime /* inner */ word = y;
}
"""
        expected = """\
function f() {
  let comptime x: comptime word = y;
  let comptime /* outer */ z: comptime /* inner */ word = y;
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_keeps_canonical_binding_and_type_comptime_layers(self) -> None:
        canonical = (
            "function f() {"
            " let comptime x: comptime /* type */ word = y;"
            " }\n"
        )

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_preserves_nested_comptime_in_rust_source_literals(self) -> None:
        rust = r'''
const RAW: &str =
    r#"function f() { let x: comptime comptime word = y; }"#;
const ORDINARY: &str =
    "function f() { let x: comptime comptime word = y; }";
'''

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertEqual(migrated.count("let comptime x: comptime word"), 2)
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)


class LetInitializerMigrationTests(unittest.TestCase):
    def test_rewrites_classic_let_initializers_in_bodies_and_for_loops(
        self,
    ) -> None:
        classic = """\
function f(v: Option(word)) {
  let x := 1;
  let y: Option(word) := v;
  let z: comptime Option(word) := v;
  for (let i := 0; i < 1; i += 1) {}
  assembly { let untouched := 1 }
}
"""
        expected = """\
function f(v: Option<word>) {
  let x = 1;
  let y: Option<word> = v;
  let comptime z: Option<word> = v;
  for (let i = 0; i < 1; i += 1) {}
  assembly { let untouched := 1 }
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_comments_around_classic_initializer(self) -> None:
        classic = (
            "function f() {"
            " let x /* binding */ : Option(word) /* type */"
            " := /* value */ make();"
            " }\n"
        )
        expected = (
            "function f() {"
            " let x /* binding */ : Option<word> /* type */"
            " = /* value */ make();"
            " }\n"
        )

        self.assertEqual(MIGRATE.migrate_source(classic), expected)

    def test_preserves_standalone_yul_walrus_literals(self) -> None:
        rust = r'''
const YUL: &str = r#"let x := add(1, 2);"#;
const ORDINARY: &str = "let x: Option(word) := value;";
const SOURCE: &str =
    r#"function f() { let x := 1; }"#;
'''

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 2)
        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn('r#"let x := add(1, 2);"#', migrated)
        self.assertIn('"let x: Option<word> = value;"', migrated)
        self.assertIn("function f() { let x = 1; }", migrated)
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)


class CallOptionMigrationTests(unittest.TestCase):
    def test_rejects_solidity_call_options_before_annotation_migration(
        self,
    ) -> None:
        cases = [
            "function f(x: word) { target.call{value: x, gas: x}(x); }\n",
            "function f(x: word) { new C{salt: x}(); }\n",
            (
                "function f(x: word) { obj.call /* call */ {"
                " /* option */ gas /* name */ : gasFor(x ? 1 : 2),"
                " value: wrap({nested: x}) /* value */ }(x); }\n"
            ),
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate Solidity call options",
                ):
                    MIGRATE.migrate_source(classic)

    def test_does_not_confuse_declaration_or_statement_blocks_with_options(
        self,
    ) -> None:
        canonical = """\
contract C {
  value: word;
  function f(x: word) {
    if (true) { let value: word = x; }
    target(x);
  }
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)


class NamedArgumentMigrationTests(unittest.TestCase):
    def test_rejects_named_call_and_struct_arguments_before_annotations(
        self,
    ) -> None:
        cases = [
            "function f(x: word) { return g({arg: x}); }\n",
            (
                "struct S { a: word; b: word; }"
                " function f(x: word) returns (S) {"
                " return S({a: x, b: 0}); }\n"
            ),
            (
                "function f(x: word) { return g({"
                " /* name */ arg /* colon */ :"
                " choose(x ? 1 : 2), nested: h({inner: x}),"
                " }); }\n"
            ),
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate named call or struct arguments",
                ):
                    MIGRATE.migrate_source(classic)

    def test_does_not_confuse_core_blocks_and_fields_with_named_arguments(
        self,
    ) -> None:
        canonical = """\
struct S { value: word; }
function f(x: word) {
  if (true) { let value: word = x; }
  match (x) { default { return; } }
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_named_arguments_in_rust_source_literals(self) -> None:
        rust = r'''
const RAW: &str =
    r#"function f(x: word) { return g({arg: x}); }"#;
const ORDINARY: &str =
    "function f(x: word) { return S({value: x}); }";
'''

        with self.assertRaisesRegex(
            ValueError,
            "cannot migrate named call or struct arguments",
        ):
            MIGRATE.migrate_rust_strings(rust)


class UnsupportedSoliditySugarMigrationTests(unittest.TestCase):
    def test_rejects_deliberately_omitted_solidity_constructs(self) -> None:
        cases = [
            (
                "contract C { receive() external payable {} }\n",
                "Solidity `receive` declaration",
            ),
            (
                "event Transfer(word indexed sender, word value);\n",
                "Solidity event declaration",
            ),
            (
                "event Transfer(word value) anonymous;\n",
                "Solidity event declaration",
            ),
            (
                "error Unauthorized(word caller);\n",
                "Solidity custom-error declaration",
            ),
            (
                "modifier onlyOwner() { _; }\n",
                "Solidity modifier declaration",
            ),
            (
                "function f(x: word) { emit Token.Transfer(x); }\n",
                "Solidity `emit` statement",
            ),
            (
                "function f(x: word) { revert Unauthorized(x); }\n",
                "custom-error revert",
            ),
            (
                "function revert(x: word) { return; }\n",
                "Classic `revert` identifier",
            ),
            (
                "function f(x: word) { return new pkg.C<word>(x); }\n",
                "Solidity `new` creation expression",
            ),
            (
                "function f(n: word) { let xs = new word[](n); }\n",
                "Solidity `new` creation expression",
            ),
        ]

        for classic, message in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(ValueError, message):
                    MIGRATE.migrate_source(classic)

    def test_preserves_ordinary_identifiers_and_bare_revert(self) -> None:
        canonical = """\
function receive() { receive(); }
function event(x: word) { event(x); }
function error(x: word) { error(x); }
function modifier(x: word) { modifier(x); }
function emit(x: word) { emit(x); obj.emit(x); }
function new(x: word) { new(x); obj.new(x); }
function f(flag: bool) {
  if (flag) revert;
  if (flag) { revert; }
  else revert;
  assembly { revert(0, 0) }
  let text = "emit Event(0); revert Error();";
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_isolated_constructs_in_rust_source_literals(self) -> None:
        cases = [
            'const S: &str = r#"event E(word x);"#;\n',
            'const S: &str = r#"error E(word x);"#;\n',
            'const S: &str = r#"modifier only() { _; }"#;\n',
            'const S: &str = r#"receive() external payable {}"#;\n',
            'const S: &str = r#"emit E(0);"#;\n',
            'const S: &str = r#"revert E(0);"#;\n',
            'const S: &str = r#"new C(0);"#;\n',
        ]

        for rust in cases:
            with self.subTest(rust=rust):
                self.assertEqual(
                    len(MIGRATE._rust_solcore_literal_spans(rust)),
                    1,
                )
                with self.assertRaisesRegex(ValueError, "cannot migrate"):
                    MIGRATE.migrate_rust_strings(rust)

    def test_does_not_classify_similar_rust_prose_or_calls(self) -> None:
        rust = r'''
const PROSE: &str = "a new C(value) example";
const CALLS: &str = r#"emit(0); new(0); obj.emit(0);"#;
'''

        self.assertEqual(MIGRATE._rust_solcore_literal_spans(rust), [])
        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)


class GenericFallbackMigrationTests(unittest.TestCase):
    def test_rejects_generic_and_constrained_fallbacks(self) -> None:
        cases = [
            "forall T. fallback(x: T) -> T { return x; }\n",
            (
                "forall T: Eq. external fallback(x: T)"
                " returns (T) { return x; }\n"
            ),
            "(T: Eq) => fallback(x: T) returns (T) { return x; }\n",
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate generic or constrained fallback",
                ):
                    MIGRATE.migrate_source(classic)

    def test_preserves_non_generic_fallbacks_and_named_functions(self) -> None:
        canonical = """\
contract C {
  fallback() external payable { revert; }
  function fallback<T>(x: T) returns (T) { return x; }
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_isolated_generic_fallback_in_rust_literal(self) -> None:
        rust = (
            'const SOURCE: &str = r#"forall T. '
            'fallback(x: T) -> T { return x; }"#;\n'
        )

        self.assertEqual(len(MIGRATE._rust_solcore_literal_spans(rust)), 1)
        with self.assertRaisesRegex(
            ValueError,
            "cannot migrate generic or constrained fallback",
        ):
            MIGRATE.migrate_rust_strings(rust)


class ContractInheritanceMigrationTests(unittest.TestCase):
    def test_rejects_unsupported_contract_like_inheritance(self) -> None:
        cases = [
            (
                "contract Child is Base {}\n",
                "cannot migrate contract inheritance at line 1, column 16",
            ),
            (
                "interface I /* head */ is /* base */ Base, Other(word) {}\n",
                "cannot migrate interface inheritance at line 1, column 24",
            ),
            (
                "library L\n is Base;\n",
                "cannot migrate library inheritance at line 2, column 2",
            ),
            (
                "abstract contract C<T> /* head */ is /* base */"
                ' A<T>("is", f({x: 1})), B {}\n',
                "cannot migrate contract inheritance",
            ),
        ]

        for classic, message in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(ValueError, message):
                    MIGRATE.migrate_source(classic)

    def test_does_not_confuse_value_types_or_contract_bodies_with_inheritance(
        self,
    ) -> None:
        canonical = """\
type Amount is word;
contract C {
  type Inner is word;
  value: word;
  function f() { let text = "is"; }
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_does_not_scan_past_a_malformed_contract_head(self) -> None:
        malformed = """\
contract C
type Amount is word;
contract is {}
"""

        self.assertEqual(MIGRATE.migrate_source(malformed), malformed)

    def test_legacy_negative_marker_bypasses_inheritance_rejection(self) -> None:
        marked = """\
// migrate-syntax: keep-legacy-negative
contract Child is Base {}
"""

        self.assertEqual(MIGRATE.migrate_source(marked), marked)

        nested_marked = """\
/* outer /* inner */ migrate-syntax: keep-legacy-negative */
contract Child is Base {}
"""

        self.assertEqual(
            MIGRATE.migrate_source(nested_marked),
            nested_marked,
        )

    def test_marker_text_outside_comments_does_not_bypass_rejection(
        self,
    ) -> None:
        cases = [
            """\
contract Child is Base {
  function marker() {
    let text = "migrate-syntax: keep-legacy-negative";
  }
}
""",
            """\
contract Child is Base {
  function marker() {
    assembly {
      let text := "migrate-syntax: keep-legacy-negative"
    }
  }
}
""",
        ]

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate contract inheritance",
                ):
                    MIGRATE.migrate_source(classic)

    def test_cli_rejection_leaves_inheritance_source_unchanged(self) -> None:
        source = "contract Child is Base(arg) {}\n"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "inheritance.solc"
            path.write_text(source)

            check = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            unchanged = path.read_text()

        self.assertEqual(check.returncode, 2)
        self.assertEqual(migration.returncode, 2)
        self.assertEqual(unchanged, source)
        self.assertIn("cannot migrate contract inheritance", check.stderr)
        self.assertIn("cannot migrate contract inheritance", migration.stderr)

    def test_rust_string_cli_rejects_embedded_inheritance(self) -> None:
        source = (
            'const SOURCE: &str = r#"contract Child is Base {}"#;\n'
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "embedded.rs"
            path.write_text(source)

            migration = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--rust-strings",
                    str(path),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            unchanged = path.read_text()

        self.assertEqual(migration.returncode, 2)
        self.assertEqual(unchanged, source)
        self.assertIn(
            "cannot migrate contract inheritance",
            migration.stderr,
        )


class ContractTypeParameterMigrationTests(unittest.TestCase):
    def test_migrates_classic_contract_like_type_parameters(self) -> None:
        classic = """\
contract Box(t, u, /* trailing */) { value: pair(t, u); }
interface Reader(/* first */ key, value /* second */) {}
library Helpers() {}
"""
        expected = """\
contract Box<t, u /* trailing */> { value: pair<t, u>; }
interface Reader</* first */ key, value /* second */> {}
library Helpers {}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_comments_in_an_empty_classic_binder(self) -> None:
        classic = "contract C(/* no type parameters */) {}\n"
        expected = "contract C/* no type parameters */ {}\n"

        self.assertEqual(MIGRATE.migrate_source(classic), expected)

    def test_keeps_canonical_contract_type_parameters(self) -> None:
        canonical = "contract Box<t> { value: t; }\n"

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_does_not_repair_malformed_classic_type_binders(self) -> None:
        for malformed in [
            "contract C(,) {}\n",
            "contract C(, T) {}\n",
            "contract C(T,, U) {}\n",
        ]:
            with self.subTest(malformed=malformed):
                self.assertEqual(
                    MIGRATE.migrate_source(malformed),
                    malformed,
                )

    def test_rejects_inheritance_after_a_classic_type_binder(self) -> None:
        with self.assertRaisesRegex(
            ValueError,
            "cannot migrate contract inheritance",
        ):
            MIGRATE.migrate_source("contract Box(t) is Base {}\n")

    def test_migrates_contract_binders_in_rust_literals(self) -> None:
        rust = r'''
const RAW: &str = r#"contract Box(t) { value: t; }"#;
const ORDINARY: &str = "interface Empty() {}";
'''

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn("contract Box<t>", migrated)
        self.assertIn("interface Empty {}", migrated)
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)


class FunctionTypeQualifierMigrationTests(unittest.TestCase):
    def assert_function_type_rejected(
        self,
        source: str,
        offending: str,
    ) -> None:
        with self.assertRaises(ValueError) as rejection:
            MIGRATE.migrate_source(source)
        message = str(rejection.exception)
        self.assertIn(
            "cannot migrate noncanonical function type qualifier",
            message,
        )
        self.assertIn(f"`{offending}`", message)

    def test_rejects_noncanonical_qualifiers_in_every_type_context(
        self,
    ) -> None:
        cases = [
            (
                "alias Callback = function(word) public returns (bool);\n",
                "public",
            ),
            (
                "type Callback = function(word) private returns (bool);\n",
                "private",
            ),
            (
                "contract C { callback: function(word) private"
                " returns (bool); }\n",
                "private",
            ),
            (
                "function f() { let callback: function(word) memory"
                " returns (bool); }\n",
                "memory",
            ),
            (
                "function f(callback: function(word) view external"
                " returns (bool)) {}\n",
                "external",
            ),
            (
                "function f() returns (function(word) internal external"
                " returns (bool)) {}\n",
                "external",
            ),
            (
                "function f(value: word) { return value as function(word)"
                " pure view returns (bool); }\n",
                "view",
            ),
            (
                "alias Boxed = Box<function(word) internal internal"
                " returns (bool)>;\n",
                "internal",
            ),
            (
                "alias Nested = function(function(word) payable internal"
                " returns (bool)) returns (word);\n",
                "internal",
            ),
            (
                "type Guarded = function(word) onlyOwner;\n",
                "onlyOwner",
            ),
            (
                "function f() { let callback: function(word) virtual; }\n",
                "virtual",
            ),
        ]

        for source, offending in cases:
            with self.subTest(source=source):
                self.assert_function_type_rejected(source, offending)

    def test_rejects_the_complete_noncanonical_qualifier_matrix(
        self,
    ) -> None:
        cases = [
            ("function(word) public", "public"),
            ("function(word) private", "private"),
            ("function(word) internal external", "external"),
            ("function(word) external internal", "internal"),
            ("function(word) internal internal", "internal"),
            ("function(word) pure view", "view"),
            ("function(word) view view", "view"),
            ("function(word) payable payable", "payable"),
            ("function(word) view external", "external"),
            ("function(word) payable internal", "internal"),
            ("function(word) memory returns (bool)", "memory"),
            ("function(word) storage returns (bool)", "storage"),
            ("function(word) calldata returns (bool)", "calldata"),
            ("function(word) returns (bool) internal", "internal"),
            ("function(word) returns (bool) view", "view"),
            ("function(word) returns (bool) public", "public"),
            ("function(word)[] returns (bool)", "returns"),
            ("function(word) memory[]", "memory"),
            ("function(word) returns (bool) storage[]", "storage"),
            ("function(word) memory storage", "storage"),
            ("function(word) returns bool", "returns"),
            ("function(word) comptime returns (bool)", "comptime"),
            ("function(word) internal comptime returns (bool)", "comptime"),
            ("function(word) returns (bool) comptime", "comptime"),
            ("function(word) onlyOwner", "onlyOwner"),
            ("function(word) virtual", "virtual"),
            ("function(word) override", "override"),
            ("function(word) immutable", "immutable"),
            ("function(word) constant", "constant"),
            ("function(word)[foo]", "["),
            ("function(word)[1, 2]", "["),
            ("function(word)[1 + 2]", "["),
            ("function(word)[0]", "0"),
            ("function(word)[0X1]", "0X1"),
            (
                "function(word)[18446744073709551616]",
                "18446744073709551616",
            ),
        ]

        for ty, offending in cases:
            with self.subTest(ty=ty):
                self.assert_function_type_rejected(
                    f"alias Callback = {ty};\n",
                    offending,
                )

    def test_preserves_valid_outer_arrays_and_locations_in_all_contexts(
        self,
    ) -> None:
        canonical = """\
alias Bare = function(word) memory;
alias Qualified = function(word) internal view memory;
alias CompileTime = comptime function(word) internal view returns (bool) memory;
alias HexArray = function(word)[0x1];
alias LargestArray = function(word)[18446744073709551615];
contract C {
  handler: function(word) internal view memory;
  function inspect(callback: function(word) external payable returns (bool)[][2] storage) returns (function(bool) internal pure returns (word) calldata) {
    let local: function(address) view returns (bool)[4] memory = callback;
    return callback as function(word) external payable returns (bool)[][2] storage;
  }
}
"""

        migrated = MIGRATE.migrate_source(canonical)

        self.assertEqual(migrated, canonical)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_nested_function_type_boundaries(self) -> None:
        canonical = (
            "alias Nested = "
            "function(function(word) internal returns (bool) memory) "
            "external view returns "
            "(function(bool) internal payable returns (word) calldata) "
            "storage;\n"
        )

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

        invalid = canonical.replace(
            "function(word) internal",
            "function(word) view external",
        )
        self.assert_function_type_rejected(invalid, "external")

    def test_preserves_chained_conversions_after_function_types(self) -> None:
        canonical = """\
function convert(value: word) {
  return value as function(word) external view returns (bool) as Callback;
  return value as comptime @function(word) memory as Wrapped;
  return value as function(word) + 1;
  return value as function(word) == other;
  return value as function(word) ? left : right;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_allows_classic_function_return_types_before_where_clauses(
        self,
    ) -> None:
        cases = [
            (
                "function f<T>() -> function(T) where T: Eq"
                " { return g; }\n",
                "function f<T>() returns (function(T)) where T: Eq"
                " { return g; }\n",
            ),
            (
                "function f<T>() -> comptime @function(T)[] memory"
                " where T: Eq { return g; }\n",
                "function f<T>() returns (comptime @function(T)[] memory)"
                " where T: Eq { return g; }\n",
            ),
            (
                "function f<T>() -> function(T) returns (bool)"
                " where T: Eq { return g; }\n",
                "function f<T>() returns (function(T) returns (bool))"
                " where T: Eq { return g; }\n",
            ),
            (
                "function f<T>() -> function(function(T) returns (T))"
                " external view returns (bool)[][2] memory"
                " where T: Eq { return g; }\n",
                "function f<T>() returns"
                " (function(function(T) returns (T)) external view"
                " returns (bool)[][2] memory)"
                " where T: Eq { return g; }\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

        self.assert_function_type_rejected(
            "alias Callback = function(word) where;\n",
            "where",
        )

    def test_allows_function_types_at_type_operator_boundaries(self) -> None:
        classic = """\
type Higher = function(word) -> bool;
alias Direct = function(word) -> bool;
alias Generic = Box<function(word) -> bool>;
alias Nested = function(function(word) -> bool) returns (word);
contract C {
  callback: function(word) -> bool;
  function use(callback: function(word) -> bool) returns (function(word) -> bool) {
    let local: function(word) -> bool = callback;
  }
}
"""
        expected = """\
alias Higher = function(function(word)) returns (bool);
alias Direct = function(function(word)) returns (bool);
alias Generic = Box<function(function(word)) returns (bool)>;
alias Nested = function(function(function(word)) returns (bool)) returns (word);
contract C {
  callback: function(function(word)) returns (bool);
  function use(callback: function(function(word)) returns (bool)) returns (function(function(word)) returns (bool)) {
    let local: function(function(word)) returns (bool) = callback;
  }
}
"""
        migrated = MIGRATE.migrate_source(classic)
        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

        canonical = """\
alias FunctionKey = mapping(function(word) => bool);
contract C {
  callbacks: mapping(function(word) => bool);
}
"""
        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

        for invalid in [
            "alias Bad = function(word) => bool;\n",
            "alias Bad = Box<function(word) => bool>;\n",
            "alias Bad = foo.mapping(function(word) => bool);\n",
        ]:
            with self.subTest(invalid=invalid):
                self.assert_function_type_rejected(invalid, "=>")

    def test_migrates_classic_arrows_in_expression_and_enum_types(
        self,
    ) -> None:
        classic = """\
function use(x: word) {
  return x as function(word) -> bool;
  return x as Box<function(word) -> bool>;
  return @function(word) -> bool;
  return @Box<function(word) -> bool>.member;
  return @function(function(word) -> bool);
}
enum Callback {
  Direct(function(word) -> bool),
  Boxed(Box<function(word) -> bool>, word)
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertNotIn("->", migrated)
        self.assertIn(
            "return x as function(function(word)) returns (bool);",
            migrated,
        )
        self.assertIn(
            "return @function(function(word)) returns (bool);",
            migrated,
        )
        self.assertIn(
            "return @Box<function(function(word)) returns (bool)>.member;",
            migrated,
        )
        self.assertIn(
            "return @function(function(function(word)) returns (bool));",
            migrated,
        )
        self.assertIn(
            "Direct(function(function(word)) returns (bool))",
            migrated,
        )
        self.assertIn(
            "Boxed(Box<function(function(word)) returns (bool)>, word)",
            migrated,
        )
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_classic_arrow_domain_and_prefix_precedence(
        self,
    ) -> None:
        classic = """\
alias EmptyDomain = () -> C;
alias UnaryTupleDomain = (A) -> C;
alias TupleDomain = (A, B) -> C;
alias Chain = A -> B -> C;
alias CompileTime = comptime A -> B;
alias CompileTimeChain = comptime A -> B -> C;
alias ProxyDomain = @A -> B;
function proxy() {
  return @A -> B;
  return @A -> ();
  return @A -> (B);
  return @A -> (B, C);
  return @A -> (B -> C);
}
"""
        expected = """\
alias EmptyDomain = function(()) returns (C);
alias UnaryTupleDomain = function((A)) returns (C);
alias TupleDomain = function((A, B)) returns (C);
alias Chain = function(A) returns (function(B) returns (C));
alias CompileTime = comptime function(A) returns (B);
alias CompileTimeChain = comptime function(A) returns (function(B) returns (C));
alias ProxyDomain = function(@A) returns (B);
function proxy() {
  return @function(A) returns (B);
  return @function(A) returns (());
  return @function(A) returns ((B));
  return @function(A) returns ((B, C));
  return @function(A) returns ((function(B) returns (C)));
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_rejects_statement_annotations_inside_executable_bodies(
        self,
    ) -> None:
        for classic in [
            "function annotate() { x : function(); }\n",
            "function annotate() { y : A -> B; }\n",
            "function annotate() { z : function() + y; }\n",
        ]:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot safely migrate Classic expression annotation",
                ):
                    MIGRATE.migrate_source(classic)

    def test_rejects_arrows_in_unterminated_parameter_lists(self) -> None:
        for source in [
            "function f(x: A -> B {}\n",
            "function f(x: A, y: C -> D;\n",
        ]:
            with self.subTest(source=source):
                with self.assertRaisesRegex(
                    ValueError,
                    "cannot migrate Classic type arrow",
                ):
                    MIGRATE.migrate_source(source)

    def test_rejects_noncanonical_proxy_comptime_in_type_positions(
        self,
    ) -> None:
        invalid = [
            "alias F = @comptime function(word);\n",
            "contract C { f: @comptime function(word); }\n",
            "function f(x: @comptime function(word)) {}\n",
            "function f(x: word) { return x as @comptime function(word); }\n",
            "function f() { return @function(@comptime function(word)); }\n",
            "function f() { return @(@comptime function(word)); }\n",
            "function f() { return @comptime function(@comptime T); }\n",
        ]
        for source in invalid:
            with self.subTest(source=source):
                with self.assertRaisesRegex(
                    ValueError,
                    "noncanonical proxy type",
                ):
                    MIGRATE.migrate_source(source)

        canonical = """\
alias F = comptime @function(word);
function f() { return @comptime function(word); }
function g() { return @mapping(@comptime T, U); }
"""
        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_named_or_empty_function_type_entries(self) -> None:
        invalid = [
            "alias F = function(x: word);\n",
            "alias F = function(word) returns (r: bool);\n",
            "function f() { return x as function(x: word); }\n",
            "alias F = function(A,, B);\n",
            "alias F = function(,);\n",
            "alias F = function(A) returns (B,, C);\n",
        ]
        for source in invalid:
            with self.subTest(source=source):
                self.assert_function_type_rejected(
                    source,
                    ":" if ":" in source else ",",
                )

        trailing = "alias F = function(A,) returns (B,);\n"
        self.assertEqual(MIGRATE.migrate_source(trailing), trailing)

    def test_migrates_wrapped_arrow_arrays_and_separates_operators(
        self,
    ) -> None:
        classic = """\
alias Wrapped = (A -> B)[2] memory;
enum E { Wrapped((A -> B)[2] memory) }
function compare() { return @Box(A -> B)>1; }
"""
        expected = """\
alias Wrapped = (function(A) returns (B))[2] memory;
enum E { Wrapped((function(A) returns (B))[2] memory) }
function compare() { return @Box<function(A) returns (B)> >1; }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_separates_generic_closers_from_adjacent_operators(
        self,
    ) -> None:
        classic = """\
function compare(x: word) {
  return @Box<A -> B>>x;
  return @Box<A -> B>>=x;
  return @Box<A -> B>=x;
  return @Box<A -> B>==x;
  return x as Outer<A -> B>>x;
  return x as Outer<Inner<A -> B>>>=x;
}
"""
        expected = """\
function compare(x: word) {
  return @Box<function(A) returns (B)> >x;
  return @Box<function(A) returns (B)> >=x;
  return @Box<function(A) returns (B)> =x;
  return @Box<function(A) returns (B)> ==x;
  return x as Outer<function(A) returns (B)> >x;
  return x as Outer<Inner<function(A) returns (B)>> >=x;
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

        canonical = """\
function compare(x: word) {
  return x as Box<function()>>= y;
  return x as Outer<Inner<function()>>>= y;
}
"""
        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_separates_generic_closers_from_let_initializers(self) -> None:
        compact = """\
function f(value: word) {
  let single: Box<word>=value;
  let nested: Box<Box<word>>=value;
}
"""
        expected = """\
function f(value: word) {
  let single: Box<word> = value;
  let nested: Box<Box<word>> = value;
}
"""

        migrated = MIGRATE.migrate_source(compact)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_validates_nested_array_mapping_and_generic_types(self) -> None:
        invalid = [
            "alias F = function(word[0]);\n",
            "alias F = function(word[18446744073709551616]);\n",
            "alias F = function(word memory[]);\n",
            "alias F = function(mapping(word => bool,));\n",
            "function f() { return x as Box<,function()>; }\n",
        ]

        for source in invalid:
            with self.subTest(source=source):
                with self.assertRaises(ValueError):
                    MIGRATE.migrate_source(source)

        malformed_mappings = [
            "alias M = mapping(=> bool);\n",
            "contract C { m: mapping(word =>); }\n",
            "function f(x: mapping(=>)) {}\n",
            "function f() { let x: mapping(word => bool,); }\n",
            "enum E { Value(mapping(word => bool,)) }\n",
            "function f() { return x : mapping(word => bool,); }\n",
            "function f() { return @mapping(word => bool,); }\n",
        ]
        for source in malformed_mappings:
            with self.subTest(source=source):
                with self.assertRaisesRegex(
                    ValueError,
                    "malformed mapping type",
                ):
                    MIGRATE.migrate_source(source)

    def test_moves_whole_initializer_annotations_to_let_bindings(
        self,
    ) -> None:
        classic = """\
function annotate(c: bool) {
  let z = a + b : word;
  let result = c ? x : y : word -> bool;
}
"""
        expected = """\
function annotate(c: bool) {
  let z: word = a + b;
  let result: function(word) returns (bool) = c ? x : y;
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_classic_type_application_in_complete_type_spans(
        self,
    ) -> None:
        classic = """\
alias Wrapped = Box(word);
alias GenericMemory = memory<word>;
alias GenericMapping = mapping<word, bool>;
function use(x: word) {
  return x as Box(word);
  return x as memory<word>;
  return @Box(function(word) -> bool);
  return @memory<word>;
  return @Box(function(word) -> bool)(x);
  return @comptime Box(function(word) -> bool);
  return @mapping(function(word) -> bool, bool);
}
enum WrappedValue { Value(Box(word)) }
"""
        expected = """\
alias Wrapped = Box<word>;
alias GenericMemory = memory<word>;
alias GenericMapping = mapping<word, bool>;
function use(x: word) {
  return x as Box<word>;
  return x as memory<word>;
  return @Box<function(function(word)) returns (bool)>;
  return @memory<word>;
  return @Box<function(function(word)) returns (bool)>(x);
  return @comptime Box<function(function(word)) returns (bool)>;
  return @mapping(function(function(word)) returns (bool) => bool);
}
enum WrappedValue { Value(Box<word>) }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_remaining_complete_declaration_type_spans(
        self,
    ) -> None:
        classic = """\
type Wad is Box(word);
type Table is mapping(word, bool,);
type Deferred is memory(comptime word);
type Generic<T, U> is mapping(T, Box(U));
trait Comparable<T> where T: Eq<Box(word)> {}
impl Eq<Box(word)> {}
impl<T> Eq<Box(word)> where T: Show<Box(word)> {}
"""
        expected = """\
type Wad is Box<word>;
type Table is mapping(word => bool);
type Deferred is memory<comptime word>;
type Generic<T, U> is mapping(T => Box<U>);
trait Comparable<T> where T: Eq<Box<word>> {}
impl Eq<Box<word>> {}
impl<T> Eq<Box<word>> where T: Show<Box<word>> {}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_proxy_mapping_and_parenthesized_arrow_types(
        self,
    ) -> None:
        classic = """\
function use() {
  return @mapping(A -> B => C);
  return @mapping(A => B -> C);
  return @mapping(function(A), B);
  return @Box(A,);
  return @mapping(A, B,);
  return @memory(A,);
  return @Box(() -> B, C);
  return @Box((A, B) -> C, D);
  return @A -> memory(B);
  return @Box(B) -> C;
  return @memory(B) -> C;
  return @pkg.Box(B) -> C;
}
"""
        expected = """\
function use() {
  return @mapping(function(A) returns (B) => C);
  return @mapping(A => function(B) returns (C));
  return @mapping(function(A) => B);
  return @Box<A,>;
  return @mapping(A => B);
  return @A memory;
  return @Box<function(()) returns (B), C>;
  return @Box<function((A, B)) returns (C), D>;
  return @function(A) returns (B memory);
  return @function(Box<B>) returns (C);
  return @function(B memory) returns (C);
  return @function(pkg.Box<B>) returns (C);
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

        for invalid in [
            "function f() { return @mapping(@comptime T => U); }\n",
            "function f() { return @mapping(T => @comptime U); }\n",
        ]:
            with self.subTest(invalid=invalid):
                with self.assertRaisesRegex(
                    ValueError,
                    "noncanonical proxy type",
                ):
                    MIGRATE.migrate_source(invalid)

    def test_preserves_clear_proxy_calls_and_rewrites_only_type_syntax(
        self,
    ) -> None:
        source = """\
function use(flag: bool) {
  return @T(42);
  return @T("hi");
  return @T(!flag);
  return @@T(42);
  return @comptime T(42);
  return @T(@comptime A);
  return @mapping(42);
  return @mapping(@comptime A, B);
  return @word memory[1];
}
"""
        expected = source.replace(
            "@word memory[1]", "(@word memory)[1]"
        )

        migrated = MIGRATE.migrate_source(source)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_comptime_function_layers_in_mapping_keys(
        self,
    ) -> None:
        canonical = """\
alias First = mapping(comptime function() memory[] storage => word);
alias Second = mapping(comptime function() memory[2] storage => word);
alias Third = mapping(comptime function() view returns (word)[] memory storage => word);
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_preserves_comment_order_across_nested_location_wrappers(
        self,
    ) -> None:
        classic = (
            "alias X = memory(/*a*/ storage(/*b*/ word /*c*/) /*d*/);\n"
        )
        expected = (
            "alias X = memory</*a*/ /*b*/ word /*c*/ storage /*d*/ >;\n"
        )

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_comparisons_without_treating_ternary_as_annotation(
        self,
    ) -> None:
        classic = """\
function annotate(c: bool) {
  return c ? a >= b : d;
  return @pkg.T -> bool;
}
"""
        expected = """\
function annotate(c: bool) {
  return c ? a >= b : d;
  return @function(pkg.T) returns (bool);
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_migrates_nested_proxy_type_arrows_once(self) -> None:
        classic = """\
function use() {
  return @Box<@Inner<function(word) -> bool>>;
  return @(@Box<function(word) -> bool>);
  return @Box<@Inner<function(word) -> bool>>.member;
  return @Box<@Inner<function(word) -> bool>>(value);
  return @Box<@Inner<function(word) -> bool>>[value];
}
"""
        expected = """\
function use() {
  return @Box<@Inner<function(function(word)) returns (bool)>>;
  return @(@Box<function(function(word)) returns (bool)>);
  return @Box<@Inner<function(function(word)) returns (bool)>>.member;
  return @Box<@Inner<function(function(word)) returns (bool)>>(value);
  return (@Box<@Inner<function(function(word)) returns (bool)>>)[value];
}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_proxy_expression_boundaries(self) -> None:
        classic = """\
function useProxy(x: word) {
  return @function(word) + x;
  return @function(word) == x;
  return @function(word) as Callback;
  return @function(word).member;
  return @function(word)(x);
  return @function(word)[x];
  return @function(word)[0];
  return @function(word) ? left : right;
  return condition ? left : @function(word).member;
  @function(word) += x;
  x as function(word) = replacement;
}
"""
        expected = """\
function useProxy(x: word) {
  return @function(word) + x;
  return @function(word) == x;
  return @function(word) as Callback;
  return @function(word).member;
  return @function(word)(x);
  return (@function(word))[x];
  return (@function(word))[0];
  return @function(word) ? left : right;
  return condition ? left : @function(word).member;
  @function(word) += x;
  x as function(word) = replacement;
}
"""

        migrated_proxy = MIGRATE.migrate_source(classic)
        self.assertEqual(migrated_proxy, expected)
        self.assertEqual(
            MIGRATE.migrate_source(migrated_proxy),
            migrated_proxy,
        )

        for ambiguous in [
            "function f() { return @A[1]; }\n",
            "function f() { return @function()[2]; }\n",
            "function f() { return @Box<T>[3]; }\n",
        ]:
            with self.subTest(ambiguous=ambiguous):
                with self.assertRaisesRegex(
                    ValueError,
                    "ambiguous proxy array/index syntax",
                ):
                    MIGRATE.migrate_source(ambiguous)

        for ambiguous_call in [
            "function f() { return @Box(word); }\n",
            "function f() { return @Box(word)(x); }\n",
            "function f() { return @Box(word)[1]; }\n",
            "function f() { return @T(x); }\n",
            "function f() { return @T(); }\n",
            "function f() { return @mapping(A, B); }\n",
        ]:
            with self.subTest(ambiguous_call=ambiguous_call):
                with self.assertRaisesRegex(
                    ValueError,
                    "ambiguous proxy call/type-application syntax",
                ):
                    MIGRATE.migrate_source(ambiguous_call)

        canonical_arrays = (
            "function f() { return @word[]; return @function()[]; "
            "return @Box<T>[][2] calldata; }\n"
        )
        self.assertEqual(
            MIGRATE.migrate_source(canonical_arrays),
            canonical_arrays,
        )

        classic_dynamic_arrays = """\
function f() {
  return @Outer<Inner(A)>[];
  return @Box<A -> B>[];
  return @Box(A,)[];
}
"""
        expected_dynamic_arrays = """\
function f() {
  return @Outer<Inner<A>>[];
  return @Box<function(A) returns (B)>[];
  return @Box<A,>[];
}
"""
        migrated_dynamic_arrays = MIGRATE.migrate_source(
            classic_dynamic_arrays
        )
        self.assertEqual(
            migrated_dynamic_arrays,
            expected_dynamic_arrays,
        )
        self.assertEqual(
            MIGRATE.migrate_source(migrated_dynamic_arrays),
            migrated_dynamic_arrays,
        )

        self.assert_function_type_rejected(
            "function f() { return comptime @function(word) + x; }\n",
            "+",
        )

        classic = """\
function control(c: bool) {
  if @function() {}
  while @function() {}
  match @function() { | _ => 0 }
  return if @function() then left else right;
  return if c then @function() else right;
}
"""
        migrated = MIGRATE.migrate_source(classic)
        self.assertIn("if (@function()) {}", migrated)
        self.assertIn("while (@function()) {}", migrated)
        self.assertIn("match (@function()) {", migrated)
        self.assertIn("return (@function() ? left : right);", migrated)
        self.assertIn("return (c ? @function() : right);", migrated)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

        self.assert_function_type_rejected(
            "function f() { return @function() {}; }\n",
            "{",
        )

        for invalid_arm in [
            "function f() { return if x then @function() {} else y; }\n",
            "function f() { return if x then y as function() {} else z; }\n",
            "function f() { return if x then y : function() {} else z; }\n",
        ]:
            with self.subTest(invalid_arm=invalid_arm):
                self.assert_function_type_rejected(invalid_arm, "{")

        for invalid_for in [
            "function f() { for @function() {} }\n",
            "function f() { for x as function() {} }\n",
            "function f() { for x : function() {} }\n",
        ]:
            with self.subTest(invalid_for=invalid_for):
                self.assert_function_type_rejected(invalid_for, "{")

        canonical_for = (
            "function f() { for (; @function(); ) {} }\n"
        )
        self.assertEqual(
            MIGRATE.migrate_source(canonical_for),
            canonical_for,
        )

    def test_rejects_malformed_classic_type_arrows_and_delimiters(
        self,
    ) -> None:
        malformed_arrows = [
            "alias Bad = -> A;\n",
            "alias Bad = A ->;\n",
            "alias Bad = A -> -> B;\n",
            "function f() { return x as Box<A -> >; }\n",
            "function f() { return @-> bool; }\n",
            "enum E { Bad(function(word) ->) }\n",
        ]
        for source in malformed_arrows:
            with self.subTest(source=source):
                with self.assertRaisesRegex(
                    ValueError,
                    "malformed Classic type arrow",
                ):
                    MIGRATE.migrate_source(source)

        malformed_delimiters = [
            "alias Bad = Box<word -> bool>>;\n",
            "function f() { return x as Box<word -> bool>>; }\n",
            "enum E { Bad(Box<word -> bool>>) }\n",
            "alias Bad = Box<(word] -> bool>;\n",
            "function f() { let x: Box<(word] = value; }\n",
            "function f() { let x: Box<(word] }\n",
            "let x: Box<word\n",
        ]
        for source in malformed_delimiters:
            with self.subTest(source=source):
                with self.assertRaisesRegex(
                    ValueError,
                    "malformed type delimiters|cannot migrate Classic type arrow",
                ):
                    MIGRATE.migrate_source(source)

        with self.assertRaisesRegex(ValueError, "noncanonical type suffix"):
            MIGRATE.render_type(MIGRATE.significant("T[8 >> 1]"))

    def test_malformed_arrow_rejection_is_atomic_for_cli_and_rust(
        self,
    ) -> None:
        source = "alias Bad = A ->;\n"
        for rust in [
            'const SOURCE: &str = r#"alias Bad = A ->;"#;\n',
            'const SOURCE: &str = "alias Bad = A ->;";\n',
        ]:
            with self.subTest(rust=rust):
                with self.assertRaisesRegex(
                    ValueError,
                    "malformed Classic type arrow",
                ):
                    MIGRATE.migrate_rust_strings(rust)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.solc"
            path.write_text(source)
            check = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            unchanged = path.read_text()

        self.assertEqual(check.returncode, 2)
        self.assertEqual(migration.returncode, 2)
        self.assertEqual(unchanged, source)
        self.assertIn("malformed Classic type arrow", check.stderr)
        self.assertIn(
            "malformed Classic type arrow",
            migration.stderr,
        )

    def test_renderer_refuses_unknown_function_type_tails(self) -> None:
        for ty in [
            "function(word) onlyOwner",
            "function(word) returns (bool) virtual",
            "function(word) memory override",
        ]:
            with self.subTest(ty=ty):
                with self.assertRaisesRegex(
                    ValueError,
                    "noncanonical function type qualifier sequence",
                ):
                    MIGRATE.render_type(MIGRATE.significant(ty))

        for ty in [
            "function(word) @Other",
            "function(word) (bool)",
            "function(word) ?",
            "function(word) { value }",
        ]:
            with self.subTest(ty=ty):
                with self.assertRaisesRegex(
                    ValueError,
                    "noncanonical function type suffix",
                ):
                    MIGRATE.render_type(MIGRATE.significant(ty))

        for tail in ["@Other", "(bool)", "?", "{ value }"]:
            with self.subTest(tail=tail):
                with self.assertRaisesRegex(
                    ValueError,
                    "noncanonical function type",
                ):
                    MIGRATE.migrate_source(
                        f"type Callback = function(word) {tail};\n"
                    )

    def test_validator_refuses_symbol_tails_in_canonical_contexts(
        self,
    ) -> None:
        cases = [
            (
                "alias Callback = function(word) @Other;\n",
                "@",
            ),
            (
                "alias Callback = function(word) (bool);\n",
                "(",
            ),
            (
                "alias Callback = function(word) ?;\n",
                "?",
            ),
            (
                "alias Callback = function(word) { value };\n",
                "{",
            ),
            (
                "alias Callback = function(word) .member;\n",
                ".",
            ),
            (
                "alias Boxed = Box<function(word) @Other>;\n",
                "@",
            ),
            (
                "alias Nested = function(function(word) @Other)"
                " returns (word);\n",
                "@",
            ),
            (
                "function f(value: word) {"
                " return value as function(word) @Other; }\n",
                "@",
            ),
            (
                "alias Proxy = @function(word) + word;\n",
                "+",
            ),
            (
                "alias Proxy = @function(word).member;\n",
                ".",
            ),
            (
                "alias Proxy = @function(word)(word);\n",
                "(",
            ),
            (
                "alias Proxy = @function(word)[word];\n",
                "[",
            ),
            (
                "alias Proxy = Box<@function(word) + word>;\n",
                "+",
            ),
        ]

        for source, offending in cases:
            with self.subTest(source=source):
                self.assert_function_type_rejected(source, offending)

        rust = (
            'const SOURCE: &str = r#"alias Callback = '
            'function(word) @Other;"#;\n'
        )
        with self.assertRaisesRegex(
            ValueError,
            "noncanonical function type qualifier",
        ):
            MIGRATE.migrate_rust_strings(rust)

    def test_migrates_classic_location_wrappers_around_function_types(
        self,
    ) -> None:
        cases = [
            (
                "type Callback = memory(function(word) internal view"
                " returns(bool));\n",
                "alias Callback = function(word) internal view"
                " returns (bool) memory;\n",
            ),
            (
                "function f(callback: calldata(function(word) external"
                " payable returns(bool))) {}\n",
                "function f(callback: function(word) external payable"
                " returns (bool) calldata) {}\n",
            ),
            (
                "alias Wrapped = @memory(word,);\n",
                "alias Wrapped = @memory<word>;\n",
            ),
            (
                "alias Wrapped = @storage(A, B);\n",
                "alias Wrapped = @storage<A, B>;\n",
            ),
            (
                "alias Wrapped = comptime @calldata(Box(word),);\n",
                "alias Wrapped = comptime @calldata<Box<word>>;\n",
            ),
        ]

        for classic, expected in cases:
            with self.subTest(classic=classic):
                migrated = MIGRATE.migrate_source(classic)
                self.assertEqual(migrated, expected)
                self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_comments_do_not_change_function_type_recognition(self) -> None:
        canonical = """\
contract C {
  callback: function /* type */ (/* param */ word)
    /* visibility */ external /* mutability */ view
    returns (/* result */ bool) /* array */ [4]
    /* location */ storage;
  function f() public view returns (word) { return 0; }
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

        self.assert_function_type_rejected(
            "alias Callback = function /* type */ (word)"
            " /* misplaced */ memory returns (bool);\n",
            "memory",
        )

    def test_legacy_negative_marker_is_the_only_source_bypass(self) -> None:
        marked = """\
// migrate-syntax: keep-legacy-negative
alias Callback = function(word) public returns (bool);
"""
        self.assertEqual(MIGRATE.migrate_source(marked), marked)

        for unmarked in [
            'let marker = "migrate-syntax: keep-legacy-negative";\n'
            "alias Callback = function(word) public returns (bool);\n",
            "assembly { let marker := "
            '"migrate-syntax: keep-legacy-negative" }\n'
            "alias Callback = function(word) public returns (bool);\n",
        ]:
            with self.subTest(unmarked=unmarked):
                self.assert_function_type_rejected(unmarked, "public")

    def test_cli_rejection_leaves_source_unchanged_in_both_modes(
        self,
    ) -> None:
        source = (
            "alias Callback = function(word) memory returns (bool);\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "callback.solc"
            path.write_text(source)

            check = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            unchanged = path.read_text()

        self.assertEqual(check.returncode, 2)
        self.assertEqual(migration.returncode, 2)
        self.assertEqual(unchanged, source)
        self.assertIn("line 1, column 33", check.stderr)
        self.assertIn(
            "cannot migrate noncanonical function type qualifier",
            migration.stderr,
        )

    def test_rust_string_migration_rejects_embedded_invalid_types(self) -> None:
        cases = [
            (
                'const SOURCE: &str = r#"alias Callback = '
                'function(word) public returns (bool);"#;\n'
            ),
            (
                'const SOURCE: &str = "alias Callback = '
                'function(word) view external returns (bool);";\n'
            ),
            (
                "// migrate-syntax: keep-legacy-negative\n"
                'const SOURCE: &str = r#"alias Callback = '
                'function(word) private returns (bool);"#;\n'
            ),
        ]

        for rust in cases:
            with self.subTest(rust=rust):
                with self.assertRaisesRegex(
                    ValueError,
                    "noncanonical function type qualifier",
                ):
                    MIGRATE.migrate_rust_strings(rust)

    def test_rust_string_migration_rewrites_expression_type_arrows(
        self,
    ) -> None:
        rust = """\
const RAW: &str = r#"function f() { return @Box<@Inner<function(word) -> bool>>; }"#;
const NORMAL: &str = "function f(x: word) { return x as Box<function(word) -> bool>; }";
"""

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertNotIn("->", migrated)
        self.assertEqual(
            migrated.count(
                "function(function(word)) returns (bool)"
            ),
            2,
        )
        self.assertEqual(
            MIGRATE.migrate_rust_strings(migrated),
            migrated,
        )


class FunctionMigrationTests(unittest.TestCase):
    def test_rejects_duplicate_classic_generic_binders(self) -> None:
        cases = (
            "forall T, T. function f(x: T) -> T { return x; }\n",
            "forall T. function f<T>(x: T) -> T { return x; }\n",
        )

        for classic in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(
                    ValueError,
                    r"duplicate Classic generic binder `T`",
                ):
                    MIGRATE.migrate_source(classic)

    def test_preserves_canonical_duplicate_generic_binders(self) -> None:
        canonical = (
            "function f<T, T>(x: T) returns (T) { return x; }\n"
        )

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_duplicate_generic_binders_in_rust_literals(
        self,
    ) -> None:
        rust = (
            'const SOURCE: &str = r#"forall T, T. '
            'function f(x: T) -> T { return x; }"#;\n'
        )

        with self.assertRaisesRegex(
            ValueError,
            r"duplicate Classic generic binder `T`",
        ):
            MIGRATE.migrate_rust_strings(rust)

    def test_duplicate_generic_binder_cli_failure_is_atomic(self) -> None:
        good_source = "alias Good = A -> B;\n"
        bad_source = (
            "forall T, T. "
            "function f(x: T) -> T { return x; }\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "good.solc"
            bad = root / "bad.solc"
            good.write_text(good_source)
            bad.write_text(bad_source)

            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(root)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            good_after = good.read_text()
            bad_after = bad.read_text()

        self.assertEqual(migration.returncode, 2)
        self.assertEqual(good_after, good_source)
        self.assertEqual(bad_after, bad_source)
        self.assertIn("0 file(s) migrated", migration.stdout)
        self.assertIn(
            "duplicate Classic generic binder `T`",
            migration.stderr,
        )

    def test_preserves_duplicate_function_modifiers(self) -> None:
        classic = """\
public public function first(x: word) -> word { return x; }
public function second(x: word) public -> word { return x; }
"""
        expected = """\
function first(x: word) public public returns (word) { return x; }
function second(x: word) public public returns (word) { return x; }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_preserves_duplicate_special_function_modifiers(self) -> None:
        classic = """\
public public constructor() {}
fallback() view view {}
"""
        expected = """\
constructor() public public {}
fallback() view view {}
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_moves_classic_prefix_before_canonical_return_clause(self) -> None:
        classic = """\
public function f(x: word) returns (word) { return x; }
external function read(key: word) returns (word);
"""
        expected = """\
function f(x: word) public returns (word) { return x; }
function read(key: word) external returns (word);
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_moves_classic_prefix_around_existing_generics_and_where(self) -> None:
        classic = """\
public function existing<T>(x: T) returns (T) where T: Eq { return x; }
forall T. function introduced(x: T) returns (T) { return x; }
"""
        expected = """\
function existing<T>(x: T) public returns (T) where T: Eq { return x; }
function introduced<T>(x: T) returns (T) { return x; }
"""

        migrated = MIGRATE.migrate_source(classic)

        self.assertEqual(migrated, expected)
        self.assertEqual(MIGRATE.migrate_source(migrated), migrated)

    def test_mixed_prefix_cli_check_reaches_a_clean_fixed_point(self) -> None:
        source = "public function f(x: word) returns (word) { return x; }\n"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mixed.solc"
            path.write_text(source)

            needs_migration = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            migration = subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            clean = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(path)],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(needs_migration.returncode, 1, needs_migration.stderr)
        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(clean.returncode, 0, clean.stderr)
        self.assertIn("1 file(s) need migration", needs_migration.stdout)
        self.assertIn("0 file(s) need migration", clean.stdout)

    def test_moves_mixed_prefix_in_rust_source_literal(self) -> None:
        rust = (
            'const SOURCE: &str = r#"public function f(x: word) '
            'returns (word) { return x; }"#;\n'
        )

        migrated = MIGRATE.migrate_rust_strings(rust)

        self.assertIn(
            "function f(x: word) public returns (word)",
            migrated,
        )
        self.assertEqual(MIGRATE.migrate_rust_strings(migrated), migrated)

    def test_preserves_canonical_no_result_prototype(self) -> None:
        canonical = """\
trait Hook<t> {
  function run(value: t);
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

    def test_rejects_unsupported_callable_header_tokens(self) -> None:
        cases = [
            (
                "constructor(x: Option(word)) Base(x) {}\n",
                "cannot migrate constructor header",
            ),
            (
                "constructor(x: word) onlyOwner {}\n",
                "cannot migrate constructor header",
            ),
            (
                "fallback() external onlyOwner {}\n",
                "cannot migrate fallback header",
            ),
            (
                "lam (x: Option(word)) view -> Result(word) { x }\n",
                "cannot migrate lambda header",
            ),
            (
                "function f(x: Option(word)) onlyOwner {}\n",
                "cannot migrate function header",
            ),
        ]

        for classic, message in cases:
            with self.subTest(classic=classic):
                with self.assertRaisesRegex(ValueError, message):
                    MIGRATE.migrate_source(classic)


if __name__ == "__main__":
    unittest.main()
