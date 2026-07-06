use std::{panic, path::Path, thread};

use annotate_snippets::Renderer;
use dir_test::{Fixture, dir_test};
use hir::{diag::Diagnostic, input::SourceFile, visit::ErrorNode};
use solcore_parser::parse_file_to_hir;

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl solcore_parser::Db for TestDb {}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/fail",
    glob: "*.solc"
)]
fn parser_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_assertion(fixture, assert_fail_fixture);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/corpus/fail",
    glob: "**/*.solc"
)]
fn parser_corpus_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_assertion(fixture, assert_fail_fixture);
}

fn assert_fail_fixture(path: &str, content: &str) {
    let db = TestDb::default();
    let file = fixture_source_file(&db, path, content);
    let _ = parse_file_to_hir(&db, file);
    let diagnostics = parse_file_to_hir::accumulated::<Diagnostic>(&db, file);
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostics for fail fixture `{}`",
        path
    );
    if path.ends_with("multiple_emitted_errors.solc") {
        assert!(
            diagnostics.len() > 1,
            "expected more than one diagnostic for `{}`",
            path
        );
    }
    let rendered = render_diagnostics(&db, &diagnostics);

    assert_snapshot_for_fixture(path, &rendered);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/ok",
    glob: "**/*.solc"
)]
fn parser_ok_no_diagnostics(fixture: Fixture<&str>) {
    run_fixture_assertion(fixture, assert_ok_fixture);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/corpus/ok",
    glob: "**/*.solc"
)]
fn parser_corpus_ok_no_diagnostics(fixture: Fixture<&str>) {
    run_fixture_assertion(fixture, assert_ok_fixture);
}

fn assert_ok_fixture(path: &str, content: &str) {
    let db = TestDb::default();
    let file = fixture_source_file(&db, path, content);

    let module = parse_file_to_hir(&db, file).module(&db);
    let diagnostics = parse_file_to_hir::accumulated::<Diagnostic>(&db, file);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics for ok fixture `{}`\n{}",
        path,
        render_diagnostics(&db, &diagnostics)
    );
    let error_nodes = hir::visit::collect_error_nodes(&db, module);
    assert!(
        error_nodes.is_empty(),
        "expected no HIR Error nodes for ok fixture `{}`\n{}",
        path,
        render_error_nodes(&db, &error_nodes)
    );
}

fn run_fixture_assertion(fixture: Fixture<&str>, assertion: fn(&str, &str)) {
    let path = fixture.path().to_owned();
    let content = fixture.content().to_string();
    let result = thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || assertion(&path, &content))
        .expect("spawn fixture assertion")
        .join();
    if let Err(payload) = result {
        panic::resume_unwind(payload);
    }
}

fn fixture_source_file(db: &TestDb, path: &str, content: &str) -> SourceFile {
    let fixture_path = Path::new(path);
    let file_name = fixture_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("fixture.solc");
    let url = format!("memory:///{file_name}")
        .parse()
        .expect("valid fixture URL");
    SourceFile::new(db, url, Some(content.to_string()))
}

fn render_diagnostics(db: &dyn hir::Db, diagnostics: &[&Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "no diagnostics\n".to_owned();
    }

    let renderer = Renderer::plain();
    let mut output = String::new();
    for (idx, diagnostic) in diagnostics.iter().enumerate() {
        if idx > 0 {
            output.push_str("\n---\n\n");
        }
        output.push_str(&diagnostic.render_with(db, &renderer));
    }
    output
}

fn render_error_nodes(db: &dyn hir::Db, errors: &[ErrorNode<'_>]) -> String {
    if errors.is_empty() {
        return "no HIR Error nodes\n".to_owned();
    }

    let mut output = String::new();
    for error in errors {
        let span = error.span.resolve_to_absolute(db);
        output.push_str(&format!(
            "{} @ {}..{}\n",
            error.kind,
            span.start().as_u32(),
            span.end().as_u32()
        ));
    }
    output
}

fn assert_snapshot_for_fixture(fixture_path: &str, value: &str) {
    let fixture_path = Path::new(fixture_path);
    let fixture_dir = fixture_path.parent().expect("fixture parent");
    let fixture_name = fixture_path
        .file_stem()
        .and_then(|name| name.to_str())
        .expect("fixture file stem");

    let mut settings = insta::Settings::new();
    settings.set_snapshot_path(fixture_dir);
    settings.set_input_file(fixture_path);
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!(fixture_name, value);
    });
}
