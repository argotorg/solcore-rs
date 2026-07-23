//! Proves the anchor-relative span design keeps a def's HIR byte-shift
//! invariant: editing *above* a definition must not change its relative span
//! (the property that lets Salsa backdate the def's downstream queries), while
//! absolute resolution still tracks the edit.

use std::sync::{Arc, Mutex};

use hir::{
    ast::{
        function::{ExprKind, FuncBody},
        item::{AdtDef, ClassDef, ContractDef, FunctionDef, Item},
    },
    input::SourceFile,
    span::Spanned,
};
use salsa::Setter;
use solcore_parser::parse_file_to_hir;

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    executed: Arc<Mutex<Vec<String>>>,
}

impl Default for TestDb {
    fn default() -> Self {
        let executed = Arc::new(Mutex::new(Vec::new()));
        Self {
            storage: salsa::Storage::new(Some(Box::new({
                let executed = executed.clone();
                move |event| {
                    if let salsa::EventKind::WillExecute { database_key } = event.kind {
                        executed
                            .lock()
                            .expect("execution log lock")
                            .push(format!("{database_key:?}"));
                    }
                }
            }))),
            executed,
        }
    }
}

impl TestDb {
    fn take_executed(&self) -> Vec<String> {
        std::mem::take(&mut *self.executed.lock().expect("execution log lock"))
    }
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

#[salsa::tracked]
fn function_relative_span<'db>(db: &'db dyn hir::Db, function: FunctionDef<'db>) -> (u32, u32) {
    let span = function.span(db);
    (span.begin().as_u32(), span.end().as_u32())
}

#[salsa::tracked]
fn function_leading_comment_text<'db>(
    db: &'db dyn hir::Db,
    function: FunctionDef<'db>,
) -> Vec<String> {
    function
        .leading_comments(db)
        .iter()
        .map(|comment| comment.text.clone())
        .collect()
}

#[salsa::tracked]
fn nested_item_semantic_names<'db>(
    db: &'db dyn hir::Db,
    adt: AdtDef<'db>,
    class: ClassDef<'db>,
    contract: ContractDef<'db>,
) -> (String, String, String) {
    let ctor = adt
        .ctors(db)
        .first()
        .expect("ADT constructor")
        .name
        .atom()
        .text(db)
        .to_owned();
    let method = class
        .methods(db)
        .first()
        .expect("class method")
        .name
        .atom()
        .text(db)
        .to_owned();
    let field = contract
        .fields(db)
        .first()
        .expect("contract field")
        .name()
        .atom()
        .text(db)
        .to_owned();
    (ctor, method, field)
}

#[salsa::tracked]
fn nested_item_comment_texts<'db>(
    db: &'db dyn hir::Db,
    adt: AdtDef<'db>,
    class: ClassDef<'db>,
    contract: ContractDef<'db>,
) -> (String, String, String) {
    let ctor = adt.ctor_comments(db)[0][0].text.clone();
    let method = class.method_comments(db)[0][0].text.clone();
    let field = contract.field_comments(db)[0][0].text.clone();
    (ctor, method, field)
}

#[salsa::tracked]
fn lambda_first_stmt_relative_span<'db>(db: &'db dyn hir::Db, body: FuncBody<'db>) -> (u32, u32) {
    let stmt_id = body
        .top_level_stmts(db)
        .first()
        .copied()
        .expect("lambda body statement");
    let span = body.stmts(db).get(stmt_id).span(db);
    (span.begin().as_u32(), span.end().as_u32())
}

fn first_function<'db>(db: &'db TestDb, file: SourceFile) -> FunctionDef<'db> {
    parse_file_to_hir(db, file)
        .module(db)
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::FunctionDef(def) => Some(*def),
            _ => None,
        })
        .expect("a top-level function")
}

fn first_lambda_body<'db>(db: &'db TestDb, file: SourceFile) -> FuncBody<'db> {
    let function_body = first_function(db, file).body(db).expect("function body");
    function_body
        .exprs(db)
        .iter()
        .find_map(|(_, expr)| match &expr.kind {
            ExprKind::Lambda { body, .. } => Some(*body),
            _ => None,
        })
        .expect("lambda expression")
}

fn nested_item_defs<'db>(
    db: &'db TestDb,
    file: SourceFile,
) -> (AdtDef<'db>, ClassDef<'db>, ContractDef<'db>) {
    let module = parse_file_to_hir(db, file).module(db);
    let adt = module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(def) => Some(*def),
            _ => None,
        })
        .expect("a top-level ADT");
    let class = module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(def) => Some(*def),
            _ => None,
        })
        .expect("a top-level class");
    let contract = module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(def) => Some(*def),
            _ => None,
        })
        .expect("a top-level contract");
    (adt, class, contract)
}

#[test]
fn top_level_error_item_has_recovery_span() {
    let db = TestDb::default();
    let url = "memory:///recovery.solc".parse().expect("valid url");
    let src = "function first() {}\nunknown nonsense tokens\nfunction second() {}\n";
    let file = SourceFile::new(&db, url, Some(src.to_owned()));

    let module = parse_file_to_hir(&db, file).module(&db);
    let error_item = module
        .items(&db)
        .iter()
        .find(|item| matches!(item, Item::Error { .. }))
        .expect("a recovered top-level error item");
    let absolute = error_item.span(&db).resolve_to_absolute(&db);

    assert_eq!(absolute.file(), file);
    assert_eq!(
        absolute.start().as_u32(),
        src.find("unknown").expect("error text") as u32
    );
}

#[test]
fn relative_span_query_backdates_after_edit_above_def() {
    let mut db = TestDb::default();
    let url = "memory:///incr.solc".parse().expect("valid url");
    let src = "function id(x: word) returns (word) {\n  return x;\n}\n";
    let file = SourceFile::new(&db, url, Some(src.to_owned()));

    // Baseline: execute the semantic-style query once, then drop all `'db`
    // borrows so the input can be mutated.
    let (before_fact, abs_start) = {
        let func = first_function(&db, file);
        let _ = db.take_executed();
        let fact = function_relative_span(&db, func);
        let executed = db.take_executed();
        assert_eq!(relative_span_query_executions(&executed), 1);

        let rel = func.span(&db);
        let abs = rel.resolve_to_absolute(&db);
        // The function anchors on itself, so its relative span starts at 0, and
        // with no leading text its absolute start is 0 too.
        assert_eq!(rel.begin().as_u32(), 0);
        assert_eq!(abs.start().as_u32(), 0);
        (fact, abs.start().as_u32())
    };

    // Insert a comment line *above* the function.
    let prefix = "// a comment above\n";
    file.set_content(&mut db).to(Some(format!("{prefix}{src}")));

    let (after_fact, abs) = {
        let func = first_function(&db, file);
        let _ = db.take_executed();
        let fact = function_relative_span(&db, func);
        let executed = db.take_executed();
        assert_eq!(relative_span_query_executions(&executed), 0);

        let rel = func.span(&db);
        (fact, rel.resolve_to_absolute(&db))
    };

    // Relative fact is byte-identical and the tracked query did not re-execute.
    assert_eq!(after_fact, before_fact);
    // Absolute span shifted by exactly the inserted prefix length.
    assert_eq!(abs.start().as_u32(), abs_start + prefix.len() as u32);
}

#[test]
fn editing_leading_comment_invalidates_only_comment_consumers() {
    let mut db = TestDb::default();
    let url = "memory:///comment-incr.solc".parse().expect("valid url");
    let file = SourceFile::new(
        &db,
        url,
        Some("// one\nfunction id(x: word) returns (word) { return x; }\n".to_owned()),
    );

    let (before_identity, before_span) = {
        let function = first_function(&db, file);
        let def = function.def_id_value(&db);
        let identity = (
            def.kind(&db),
            def.name(&db),
            def.disambiguator(&db).as_u32(),
        );
        let span = function_relative_span(&db, function);
        assert_eq!(
            function_leading_comment_text(&db, function),
            vec![" one".to_owned()]
        );
        (identity, span)
    };

    file.set_content(&mut db).to(Some(
        "// two\nfunction id(x: word) returns (word) { return x; }\n".to_owned(),
    ));

    let function = first_function(&db, file);
    let def = function.def_id_value(&db);
    let after_identity = (
        def.kind(&db),
        def.name(&db),
        def.disambiguator(&db).as_u32(),
    );
    let _ = db.take_executed();
    let after_span = function_relative_span(&db, function);
    let after_comments = function_leading_comment_text(&db, function);
    let executed = db.take_executed();

    assert_eq!(after_identity, before_identity);
    assert_eq!(after_span, before_span);
    assert_eq!(after_comments, vec![" two".to_owned()]);
    assert_eq!(relative_span_query_executions(&executed), 0);
    assert_eq!(comment_query_executions(&executed), 1);
}

#[test]
fn editing_nested_item_comments_preserves_semantic_fields() {
    let mut db = TestDb::default();
    let url = "memory:///nested-comment-incr.solc"
        .parse()
        .expect("valid url");
    let before_src = "enum Choice {
  // alpha
  First
}
trait Documented<a> {
  // alpha
  function describe(x: a) returns (word);
}
contract C {
  // alpha
  value: word;
}
";
    let file = SourceFile::new(&db, url, Some(before_src.to_owned()));

    let (before_semantics, before_comments) = {
        let (adt, class, contract) = nested_item_defs(&db, file);
        let _ = db.take_executed();
        let semantics = nested_item_semantic_names(&db, adt, class, contract);
        let comments = nested_item_comment_texts(&db, adt, class, contract);
        let executed = db.take_executed();
        assert_eq!(nested_semantic_query_executions(&executed), 1);
        assert_eq!(nested_comment_query_executions(&executed), 1);
        (semantics, comments)
    };
    assert_eq!(
        before_semantics,
        (
            "First".to_owned(),
            "describe".to_owned(),
            "value".to_owned()
        )
    );
    assert_eq!(
        before_comments,
        (
            " alpha".to_owned(),
            " alpha".to_owned(),
            " alpha".to_owned()
        )
    );

    // Keep the payload byte length unchanged so every nested declaration keeps
    // the same owner-relative span. Only the parallel comment fields change.
    file.set_content(&mut db).to(Some(
        "enum Choice {
  // bravo
  First
}
trait Documented<a> {
  // bravo
  function describe(x: a) returns (word);
}
contract C {
  // bravo
  value: word;
}
"
        .to_owned(),
    ));

    let (adt, class, contract) = nested_item_defs(&db, file);
    let _ = db.take_executed();
    let after_semantics = nested_item_semantic_names(&db, adt, class, contract);
    let after_comments = nested_item_comment_texts(&db, adt, class, contract);
    let executed = db.take_executed();

    assert_eq!(after_semantics, before_semantics);
    assert_eq!(
        after_comments,
        (
            " bravo".to_owned(),
            " bravo".to_owned(),
            " bravo".to_owned()
        )
    );
    assert_eq!(nested_semantic_query_executions(&executed), 0);
    assert_eq!(nested_comment_query_executions(&executed), 1);
}

#[test]
fn lambda_body_relative_span_backdates_after_cosmetic_signature_edit() {
    let mut db = TestDb::default();
    let url = "memory:///lambda-incr.solc".parse().expect("valid url");
    let before_src = "function make(z: word) returns (word) {
  let n = lam (x: word) returns (word) {
    return x;
  };
  return n(z);
}
";
    let file = SourceFile::new(&db, url, Some(before_src.to_owned()));

    let before_fact = {
        let body = first_lambda_body(&db, file);
        let _ = db.take_executed();
        let fact = lambda_first_stmt_relative_span(&db, body);
        let executed = db.take_executed();
        assert_eq!(lambda_span_query_executions(&executed), 1);
        fact
    };

    file.set_content(&mut db).to(Some(
        "function make(z: word) returns (word) {
  let n = lam (
    x /* same binder */: /* same parameter type */ word
  ) returns (/* same return type */ word) {
    return x;
  };
  return n(z);
}
"
        .to_owned(),
    ));

    let after_cosmetic_fact = {
        let body = first_lambda_body(&db, file);
        let _ = db.take_executed();
        let fact = lambda_first_stmt_relative_span(&db, body);
        let executed = db.take_executed();
        assert_eq!(lambda_span_query_executions(&executed), 0);
        fact
    };

    assert_eq!(after_cosmetic_fact, before_fact);

    file.set_content(&mut db).to(Some(
        "function make(z: word) returns (word) {
  let n = lam (x: uint) returns (word) {
    return x;
  };
  return n(z);
}
"
        .to_owned(),
    ));

    let after_structural_fact = {
        let body = first_lambda_body(&db, file);
        let _ = db.take_executed();
        let fact = lambda_first_stmt_relative_span(&db, body);
        let executed = db.take_executed();
        assert_eq!(lambda_span_query_executions(&executed), 1);
        fact
    };

    assert_eq!(after_structural_fact, before_fact);
}

fn relative_span_query_executions(events: &[String]) -> usize {
    events
        .iter()
        .filter(|event| event.contains("function_relative_span"))
        .count()
}

fn lambda_span_query_executions(events: &[String]) -> usize {
    events
        .iter()
        .filter(|event| event.contains("lambda_first_stmt_relative_span"))
        .count()
}

fn comment_query_executions(events: &[String]) -> usize {
    events
        .iter()
        .filter(|event| event.contains("function_leading_comment_text"))
        .count()
}

fn nested_semantic_query_executions(events: &[String]) -> usize {
    events
        .iter()
        .filter(|event| event.contains("nested_item_semantic_names"))
        .count()
}

fn nested_comment_query_executions(events: &[String]) -> usize {
    events
        .iter()
        .filter(|event| event.contains("nested_item_comment_texts"))
        .count()
}
