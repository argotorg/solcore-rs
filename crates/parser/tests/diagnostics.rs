use std::path::Path;

use annotate_snippets::Renderer;
use dir_test::{Fixture, dir_test};
use hir::{diag::Diagnostic, input::SourceFile};
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
    let db = TestDb::default();
    let file = fixture_source_file(&db, &fixture);
    let _ = parse_file_to_hir(&db, file);
    let diagnostics = parse_file_to_hir::accumulated::<Diagnostic>(&db, file);
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostics for fail fixture `{}`",
        fixture.path()
    );
    if fixture.path().ends_with("multiple_emitted_errors.solc") {
        assert!(
            diagnostics.len() > 1,
            "expected more than one diagnostic for `{}`",
            fixture.path()
        );
    }
    let rendered = render_diagnostics(&db, &diagnostics);

    assert_snapshot_for_fixture(fixture.path(), &rendered);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/ok",
    glob: "**/*.solc"
)]
fn parser_ok_no_diagnostics(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let file = fixture_source_file(&db, &fixture);

    let _ = parse_file_to_hir(&db, file).module(&db);
    let diagnostics = parse_file_to_hir::accumulated::<Diagnostic>(&db, file);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics for ok fixture `{}`\n{}",
        fixture.path(),
        render_diagnostics(&db, &diagnostics)
    );
}

fn fixture_source_file(db: &TestDb, fixture: &Fixture<&str>) -> SourceFile {
    let fixture_path = Path::new(fixture.path());
    let file_name = fixture_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("fixture.solc");
    let url = format!("memory:///{file_name}")
        .parse()
        .expect("valid fixture URL");
    SourceFile::new(db, url, Some(fixture.content().to_string()))
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
