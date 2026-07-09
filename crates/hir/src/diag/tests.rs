use std::collections::{BTreeMap, BTreeSet};

use annotate_snippets::Renderer;

use super::{span::LabelAnchor, *};
use crate::{
    anchor::{DefId, DefKind, DefLocationTable, Disambiguator},
    input::SourceFile,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::tracked(returns(ref))]
fn empty_def_location_table<'db>(
    db: &'db dyn crate::Db,
    file: SourceFile,
) -> DefLocationTable<'db> {
    let _ = (db, file);
    DefLocationTable::default()
}

#[salsa::db]
impl crate::Db for TestDb {
    fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
        empty_def_location_table(self, file)
    }
}

fn source_file(db: &TestDb, name: &str, content: Option<&str>) -> SourceFile {
    let url = format!("memory:///{name}.solc").parse().expect("valid url");
    SourceFile::new(db, url, content.map(ToOwned::to_owned))
}

fn root_span(file: SourceFile, start: u32, end: u32) -> LabelSpan {
    LabelSpan::new(
        LabelAnchor::Root(file),
        Offset::new(start),
        Offset::new(end),
    )
}

#[test]
fn diagnostic_id_includes_level_and_suggestions() {
    let db = TestDb::default();
    let file = source_file(&db, "ids", Some("let x = 1;\n"));
    let primary = root_span(file, 0, 3);
    let edit = root_span(file, 4, 5);

    let error = Diagnostic::error("same headline")
        .with_code("SC9999")
        .with_primary_label_span(primary.clone(), Some("same label"));
    let warning = Diagnostic::warning("same headline")
        .with_code("SC9999")
        .with_primary_label_span(primary.clone(), Some("same label"));

    assert_ne!(error.diagnostic_id(&db), warning.diagnostic_id(&db));

    let with_machine_fix = error.clone().with_suggestion(Suggestion {
        title: "rename".to_owned(),
        applicability: Applicability::MachineApplicable,
        edits: vec![AnchoredTextEdit {
            span: edit.clone(),
            replacement: "y".to_owned(),
        }],
    });
    let with_review_fix = error.with_suggestion(Suggestion {
        title: "rename".to_owned(),
        applicability: Applicability::MaybeIncorrect,
        edits: vec![AnchoredTextEdit {
            span: edit,
            replacement: "z".to_owned(),
        }],
    });

    assert_ne!(
        with_machine_fix.diagnostic_id(&db),
        with_review_fix.diagnostic_id(&db)
    );
}

#[test]
fn diagnostic_sort_key_uses_diagnostic_id_tiebreaker() {
    let db = TestDb::default();
    let file = source_file(&db, "sort", Some("alpha beta gamma\n"));
    let primary = root_span(file, 0, 5);

    let first = Diagnostic::error("same headline")
        .with_code("SC9999")
        .with_primary_label_span(primary.clone(), None::<String>)
        .with_secondary_label_span(root_span(file, 6, 10), Some("first secondary"));
    let second = Diagnostic::error("same headline")
        .with_code("SC9999")
        .with_primary_label_span(primary, None::<String>)
        .with_secondary_label_span(root_span(file, 11, 16), Some("second secondary"));

    let first_key = first.sort_key(&db);
    let second_key = second.sort_key(&db);
    assert_eq!(first_key.file, second_key.file);
    assert_eq!(first_key.primary_start, second_key.primary_start);
    assert_eq!(first_key.code, second_key.code);
    assert_eq!(first_key.message, second_key.message);
    assert_ne!(first_key.id, second_key.id);
    assert_ne!(first_key, second_key);

    let mut original_order = [first.clone(), second.clone()];
    original_order.sort_by_key(|diagnostic| diagnostic.sort_key(&db));
    let mut reversed_order = [second, first];
    reversed_order.sort_by_key(|diagnostic| diagnostic.sort_key(&db));

    let original_ids = original_order
        .iter()
        .map(|diagnostic| diagnostic.diagnostic_id(&db))
        .collect::<Vec<_>>();
    let reversed_ids = reversed_order
        .iter()
        .map(|diagnostic| diagnostic.diagnostic_id(&db))
        .collect::<Vec<_>>();
    assert_eq!(original_ids, reversed_ids);
}

#[test]
fn diagnostic_code_registry_has_only_documented_aliases() {
    let mut by_code = BTreeMap::<&str, Vec<&str>>::new();
    for entry in DiagnosticCode::ALL {
        by_code.entry(entry.code()).or_default().push(entry.name());
    }

    let mut allowed = BTreeMap::<&str, &str>::new();
    for alias in DiagnosticCode::INTENTIONAL_DUPLICATES {
        assert!(
            !alias.reason().trim().is_empty(),
            "intentional duplicate {} needs a reason",
            alias.code()
        );
        assert!(
            allowed.insert(alias.code(), alias.reason()).is_none(),
            "duplicate allow-list entry for {}",
            alias.code()
        );
    }

    let mut undocumented = Vec::new();
    for (code, names) in &by_code {
        if names.len() > 1 && !allowed.contains_key(code) {
            undocumented.push(format!("{code}: {}", names.join(", ")));
        }
    }
    assert!(
        undocumented.is_empty(),
        "duplicate diagnostic codes need explicit allow-list entries: {}",
        undocumented.join("; ")
    );

    let duplicate_codes = by_code
        .iter()
        .filter_map(|(code, names)| (names.len() > 1).then_some(*code))
        .collect::<BTreeSet<_>>();
    for code in allowed.keys() {
        assert!(
            duplicate_codes.contains(*code),
            "allow-list entry {code} does not correspond to duplicate registry values"
        );
    }
}

#[test]
fn render_skips_contentless_def_labels_before_absolute_resolution() {
    let db = TestDb::default();
    let file = source_file(&db, "missing", None);
    let def = DefId::new(
        &db,
        file,
        None,
        DefKind::Function,
        Some("f".to_owned()),
        None,
        Disambiguator::ZERO,
    );
    let stale_def_span = LabelSpan::new(
        LabelAnchor::Def(def.key(&db)),
        Offset::new(0),
        Offset::new(1),
    );
    let diagnostic = Diagnostic::error("stale diagnostic")
        .with_code("SC9998")
        .with_primary_label_span(stale_def_span, Some("stale label"))
        .with_note("note still renders");

    let rendered = diagnostic.render_with(&db, &Renderer::plain());
    assert!(rendered.contains("stale diagnostic"));
    assert!(rendered.contains("note still renders"));
    assert!(!rendered.contains("stale label"));
}
