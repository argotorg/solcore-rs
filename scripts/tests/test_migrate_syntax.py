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
    def test_preserves_prose_format_strings_and_sql(self) -> None:
        rust = r'''
const HELP: &str = r#"prefer function f(x: T) -> T syntax"#;
const DIAGNOSTIC: &str = "expected function f(x: T) -> T";
const FORMAT: &str = r#"function f(x: T) -> T {{ return {value}; }}"#;
const TYPE_FORMAT: &str = r#"type {name} = {ty};"#;
const IMPORT_FORMAT: &str = r#"import {{name}} from {path};"#;
const IMPORT_PROSE: &str = r#"run import foo.bar; to continue"#;
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


class FunctionMigrationTests(unittest.TestCase):
    def test_preserves_canonical_no_result_prototype(self) -> None:
        canonical = """\
trait Hook<t> {
  function run(value: t);
}
"""

        self.assertEqual(MIGRATE.migrate_source(canonical), canonical)


if __name__ == "__main__":
    unittest.main()
