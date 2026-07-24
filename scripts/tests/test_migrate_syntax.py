from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


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

    def test_does_not_rewrite_member_access(self) -> None:
        canonical = """\
enum Option<T> { None, Some(T) }
function project(value: Option<word>) returns (word) {
  return value.Some;
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)

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


class ClassicBareImportMigrationTests(unittest.TestCase):
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
const SQL: &str =
    r#"select type, alias, comptime from function where match = ?;"#;
const SQL_IMPORT: &str = r#"import records from audit;"#;
'''

        self.assertEqual(MIGRATE.migrate_rust_strings(rust), rust)
        self.assertEqual(MIGRATE._rust_solcore_literal_spans(rust), [])

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

    def test_cli_owner_scan_decodes_rust_specific_escapes(self) -> None:
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
            check = subprocess.run(
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

        self.assertEqual(migration.returncode, 0, migration.stderr)
        self.assertEqual(check.returncode, 0, check.stderr)
        self.assertIn('"enum Option<a> { None, Some(a) }"', migrated)
        self.assertIn("return Option.Some(x);", migrated)
        self.assertIn("0 file(s) need migration", check.stdout)
        self.assert_rust_syntax(migrated)

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
            (
                "function f(value: word) -> word {"
                " return value : /* type */ Option(/* arg */ word);"
                " }\n",
                "function f(value: word) returns (word) {"
                " return value  as /* type */ Option</* arg */ word>;"
                " }\n",
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


class FunctionMigrationTests(unittest.TestCase):
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
