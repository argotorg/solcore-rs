use hir::{
    ast::{
        function::{ExprKind, FuncParam, StmtKind},
        item::{ContractItem, FunctionDef, Item, Module},
        ty::TypeRefKind,
    },
    diag::{AnyDiagnostic, Diagnostic},
    input::SourceFile,
    span::Spanned,
};
use solcore_parser::{parse_diagnostics, parse_file_to_hir};

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

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.solc").parse().expect("valid url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn parse_module<'db>(db: &'db TestDb, name: &str, src: &str) -> (SourceFile, Module<'db>) {
    let file = source_file(db, name, src);
    (file, parse_file_to_hir(db, file).module(db))
}

fn diagnostics(db: &TestDb, file: SourceFile) -> Vec<Diagnostic> {
    parse_diagnostics(db, file)
        .iter()
        .map(|diagnostic: &AnyDiagnostic| diagnostic.lower(db))
        .collect()
}

fn top_function<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> FunctionDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::FunctionDef(function) if (*function.sig(db).name.atom()).text(db) == name => {
                Some(*function)
            }
            _ => None,
        })
        .expect("top-level function")
}

#[test]
fn block_comments_do_not_swallow_following_items_and_unterminated_comments_diagnose() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "block-comment-ok",
        "/* **/ /* outer /* inner */ done */ function f() {}",
    );
    assert_eq!(
        (*top_function(&db, module, "f").sig(&db).name.atom()).text(&db),
        "f"
    );

    let file = source_file(&db, "block-comment-bad", "/* unterminated\nfunction f() {}");
    let messages = diagnostics(&db, file)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message == "unterminated block comment")
    );
}

#[test]
fn equivalent_type_and_predicate_refs_share_semantic_shapes_without_sharing_occurrences() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "type-ref-shapes",
        "class self:C {}
         function a(x: word) {}
         function b(y: word) {}
         forall t . t:C => function c(x: t) {}
         forall t . t:C => function d(x: t) {}",
    );

    let a = top_function(&db, module, "a");
    let b = top_function(&db, module, "b");
    let a_ty = match &a.sig(&db).params.atom()[0] {
        FuncParam::Typed { ty, .. } => *ty,
        other => panic!("unexpected param: {other:?}"),
    };
    let b_ty = match &b.sig(&db).params.atom()[0] {
        FuncParam::Typed { ty, .. } => *ty,
        other => panic!("unexpected param: {other:?}"),
    };
    assert_ne!(a_ty, b_ty);
    assert_eq!(a_ty.semantic_shape(), b_ty.semantic_shape());

    let c = top_function(&db, module, "c");
    let d = top_function(&db, module, "d");
    let c_pred = c.sig(&db).preds[0];
    let d_pred = d.sig(&db).preds[0];
    assert_ne!(c_pred, d_pred);
    assert_eq!(c_pred.semantic_shape(), d_pred.semantic_shape());
}

#[test]
fn implicit_return_applies_to_function_definitions_but_not_lambdas() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "implicit-return",
        "function id(x: word) -> word { x }
         function make() { return lam (x: word) { x }; }",
    );

    let id = top_function(&db, module, "id");
    let id_body = id.body(&db).expect("body");
    let id_stmt = id_body.stmts(&db).get(id_body.top_level_stmts(&db)[0]);
    assert!(matches!(&id_stmt.kind, StmtKind::Return(_)));

    let make = top_function(&db, module, "make");
    let make_body = make.body(&db).expect("body");
    let lambda_body = make_body
        .exprs(&db)
        .iter()
        .find_map(|(_, expr)| match &expr.kind {
            ExprKind::Lambda { body, .. } => Some(*body),
            _ => None,
        })
        .expect("lambda expression");
    let lambda_stmt = lambda_body
        .stmts(&db)
        .get(lambda_body.top_level_stmts(&db)[0]);
    assert!(matches!(&lambda_stmt.kind, StmtKind::Expr(_)));
}

#[test]
fn contract_fields_can_be_interleaved_and_have_initializers() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "contract-fields",
        "contract C {
           function f() {}
           x: word = 1;
         }",
    );

    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    assert_eq!(contract.fields(&db).len(), 1);
    assert!(contract.fields(&db)[0].init().is_some());
    assert_eq!(
        contract
            .items(&db)
            .iter()
            .filter(|item| matches!(item, ContractItem::FunctionDef(_)))
            .count(),
        1
    );
}

#[test]
fn top_level_recovery_resumes_at_next_item_and_preserves_body_errors() {
    let db = TestDb::default();
    let src = "import core.math
function bad() {
    let x = ;
    return 1;
}
function good() {}";
    let (file, module) = parse_module(&db, "top-level-resync", src);

    assert!(top_function(&db, module, "bad").body(&db).is_some());
    assert!(top_function(&db, module, "good").body(&db).is_some());

    let messages = diagnostics(&db, file)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| { message.contains("import declaration requires trailing `;`") })
    );
    assert!(messages.iter().any(|message| {
        message.contains("while parsing expression")
            || message.contains("while parsing statement")
            || message.contains("unexpected `let`")
            || message.contains("unexpected `;`")
    }));
}

#[test]
fn arrow_types_are_right_associative_and_tuple_domains_are_unary() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "arrow-types",
        "type F = word -> word -> bool;
         type G = (word, bool) -> uint;",
    );
    let aliases = module
        .items(&db)
        .iter()
        .filter_map(|item| match item {
            Item::TypeAlias(alias) => Some(*alias),
            _ => None,
        })
        .collect::<Vec<_>>();

    let f = aliases[0].ty(&db);
    let TypeRefKind::Fn { params, ret } = f.kind(&db) else {
        panic!("F should be an arrow type");
    };
    assert_eq!(params.atom().len(), 1);
    assert!(matches!(ret.kind(&db), TypeRefKind::Fn { .. }));

    let g = aliases[1].ty(&db);
    let TypeRefKind::Fn { params, .. } = g.kind(&db) else {
        panic!("G should be an arrow type");
    };
    assert_eq!(params.atom().len(), 1);
    assert!(matches!(
        params.atom()[0].kind(&db),
        TypeRefKind::Tuple { .. }
    ));
}

#[test]
fn type_and_predicate_argument_list_spans_are_precise() {
    let db = TestDb::default();
    let src = "class self:C(arg) {}
type T = Map(word, bool);
forall t . t:C(word) => function f(x: t) {}";
    let (_, module) = parse_module(&db, "precise-type-spans", src);

    let alias = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::TypeAlias(alias) => Some(*alias),
            _ => None,
        })
        .expect("type alias");
    let TypeRefKind::Named { args, .. } = alias.ty(&db).kind(&db) else {
        panic!("alias target should be named");
    };
    let args_abs = args.span(&db).resolve_to_absolute(&db);
    let expected_args_start = src.find("(word, bool)").expect("type args") as u32;
    assert_eq!(args_abs.start().as_u32(), expected_args_start);
    assert_eq!(
        args_abs.end().as_u32(),
        expected_args_start + "(word, bool)".len() as u32
    );

    let function = top_function(&db, module, "f");
    let pred = function.sig(&db).preds[0].kind(&db);
    let pred_args_abs = pred.args.span(&db).resolve_to_absolute(&db);
    let expected_pred_start = src.find("(word) =>").expect("predicate args") as u32;
    assert_eq!(pred_args_abs.start().as_u32(), expected_pred_start);
    assert_eq!(
        pred_args_abs.end().as_u32(),
        expected_pred_start + "(word)".len() as u32
    );
}

#[test]
fn ternary_expression_lowers_to_conditional_expression() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "ternary",
        "function f(x: bool) -> word { return x ? 1 : 0; }",
    );
    let function = top_function(&db, module, "f");
    let body = function.body(&db).expect("body");
    let stmt = body.stmts(&db).get(body.top_level_stmts(&db)[0]);
    let StmtKind::Return(Some(expr_id)) = &stmt.kind else {
        panic!("expected return with expression");
    };
    assert!(matches!(
        &body.exprs(&db).get(*expr_id).kind,
        ExprKind::If { .. }
    ));
}
