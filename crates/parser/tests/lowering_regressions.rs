use hir::{
    ast::{
        SourceComment, SourceCommentKind,
        function::{AssignOp, BinOp, ExprKind, FuncParam, StmtKind},
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

fn contract_function<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> FunctionDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match item {
                ContractItem::FunctionDef(function)
                    if (*function.sig(db).name.atom()).text(db) == name =>
                {
                    Some(*function)
                }
                _ => None,
            }),
            _ => None,
        })
        .expect("contract function")
}

fn assert_comment_texts(comments: &[SourceComment], expected: &[&str]) {
    assert_eq!(
        comments
            .iter()
            .map(|comment| comment.text.as_str())
            .collect::<Vec<_>>(),
        expected
    );
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
fn function_hir_retains_only_directly_leading_source_comments() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "function-comments",
        r#"
contract C {
  // ordinary documentation
  // #[(0, 1) -> 1]
  /* block /* nested */ documentation */
  function add(x: word, y: word) public returns (word) { return x; }

  function body_comment() {
    // this belongs to the body
  }
  function after_body() {}

  // separated from the declaration

  function after_blank_line() {}

  function trailing_owner() {} // trailing on the prior declaration
  function after_trailing() {}
}
"#,
    );

    assert_eq!(
        contract_function(&db, module, "add").leading_comments(&db),
        &[
            SourceComment {
                kind: SourceCommentKind::Line,
                text: " ordinary documentation".to_owned(),
            },
            SourceComment {
                kind: SourceCommentKind::Line,
                text: " #[(0, 1) -> 1]".to_owned(),
            },
            SourceComment {
                kind: SourceCommentKind::Block,
                text: " block /* nested */ documentation ".to_owned(),
            },
        ]
    );
    for name in [
        "body_comment",
        "after_body",
        "after_blank_line",
        "trailing_owner",
        "after_trailing",
    ] {
        assert!(
            contract_function(&db, module, name)
                .leading_comments(&db)
                .is_empty(),
            "{name} unexpectedly received leading comments"
        );
    }
}

#[test]
fn hir_retains_comments_for_every_item_like_declaration() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "all-item-comments",
        r#"
// top import
import * as dependency from dependency;
// top export
export dependency;
// top pragma
pragma solidity ^0.8.23;
// top alias
alias Alias = word;
// top enum
enum TopData {
  // first constructor
  First,
  // second constructor
  Second
}
// top trait
trait Documented<a> {
  // trait method
  function describe(x: a) returns (word);
}
// top impl
impl Documented<word> {
  // impl method
  function describe(x: word) returns (word) { return x; }
}
// top contract
contract C {
  // contract field
  value: word;
  // contract alias
  alias LocalAlias = word;
  // contract enum
  enum LocalData {
    // local first constructor
    LocalFirst,
    // local second constructor
    LocalSecond
  }
  // contract constructor
  constructor() {}
  // contract fallback
  fallback() external {}
  // contract function
  function get() returns (word) { return value; }
}
// top function
function top() {}
"#,
    );
    let diagnostics = diagnostics(&db, file);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:#?}"
    );

    let expected_top_comments = [
        " top import",
        " top export",
        " top pragma",
        " top alias",
        " top enum",
        " top trait",
        " top impl",
        " top contract",
        " top function",
    ];
    assert_eq!(module.items(&db).len(), expected_top_comments.len());
    for (item, expected) in module.items(&db).iter().zip(expected_top_comments) {
        assert_comment_texts(item.leading_comments(&db), &[expected]);
    }

    let top_adt = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("top-level ADT");
    assert_eq!(top_adt.ctors_with_comments(&db).len(), 2);
    assert_comment_texts(
        top_adt.ctor_leading_comments(&db, 0).expect("first ctor"),
        &[" first constructor"],
    );
    assert_comment_texts(
        top_adt.ctor_leading_comments(&db, 1).expect("second ctor"),
        &[" second constructor"],
    );

    let class = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(*class),
            _ => None,
        })
        .expect("class");
    assert_eq!(class.methods_with_comments(&db).len(), 1);
    assert_comment_texts(
        class.method_leading_comments(&db, 0).expect("class method"),
        &[" trait method"],
    );

    let instance = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::InstanceDef(instance) => Some(*instance),
            _ => None,
        })
        .expect("instance");
    assert_comment_texts(
        instance.methods(&db)[0].leading_comments(&db),
        &[" impl method"],
    );

    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    assert_eq!(contract.fields_with_comments(&db).len(), 1);
    assert_comment_texts(
        contract
            .field_leading_comments(&db, 0)
            .expect("contract field"),
        &[" contract field"],
    );

    let expected_contract_item_comments = [
        " contract alias",
        " contract enum",
        " contract constructor",
        " contract fallback",
        " contract function",
    ];
    assert_eq!(
        contract.items(&db).len(),
        expected_contract_item_comments.len()
    );
    for (item, expected) in contract
        .items(&db)
        .iter()
        .zip(expected_contract_item_comments)
    {
        assert_comment_texts(item.leading_comments(&db), &[expected]);
    }

    let local_adt = contract
        .items(&db)
        .iter()
        .find_map(|item| match item {
            ContractItem::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("contract-local ADT");
    assert_eq!(local_adt.ctors_with_comments(&db).len(), 2);
    assert_comment_texts(
        local_adt
            .ctor_leading_comments(&db, 0)
            .expect("local first ctor"),
        &[" local first constructor"],
    );
    assert_comment_texts(
        local_adt
            .ctor_leading_comments(&db, 1)
            .expect("local second ctor"),
        &[" local second constructor"],
    );
}

#[test]
fn item_comments_do_not_cross_blank_lines_trailing_code_or_bodies() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "item-comment-boundaries",
        r#"
alias Owner = word; // trailing top-level comment
enum AfterTrailing {}
// separated top-level comment

trait Boundary<a> {
  // separated method comment

  function method(x: a) returns (word);
}
contract C {
  first: word; // trailing field comment
  alias AfterTrailingField = word;
  // separated field comment

  second: word;
  enum Nested {
    First, // trailing constructor comment
    Second,
    // separated from the constructor name by a blank line

    Third
  }
  function body_owner() {
    // body-only comment
  }
  alias AfterBody = word;
}
"#,
    );
    let diagnostics = diagnostics(&db, file);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:#?}"
    );
    assert!(
        module
            .items(&db)
            .iter()
            .all(|item| item.leading_comments(&db).is_empty())
    );

    let class = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(*class),
            _ => None,
        })
        .expect("class");
    assert!(
        class
            .method_leading_comments(&db, 0)
            .expect("class method")
            .is_empty()
    );

    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    assert!(
        contract
            .fields_with_comments(&db)
            .all(|(_, comments)| comments.is_empty())
    );
    assert!(
        contract
            .items(&db)
            .iter()
            .all(|item| item.leading_comments(&db).is_empty())
    );
    let adt = contract
        .items(&db)
        .iter()
        .find_map(|item| match item {
            ContractItem::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("nested ADT");
    assert!(
        adt.ctors_with_comments(&db)
            .all(|(_, comments)| comments.is_empty())
    );
}

#[test]
fn recovery_items_retain_comments_without_leaking_to_following_items() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "recovery-item-comments",
        r#"
// invalid top-level item
unknown top;
function valid_top() {}
contract C {
  // invalid contract item
  unknown nested;
  function valid_nested() {}
}
"#,
    );

    let top_error = module.items(&db)[0];
    assert!(matches!(top_error, Item::Error { .. }));
    assert_comment_texts(
        top_error.leading_comments(&db),
        &[" invalid top-level item"],
    );
    assert!(
        top_function(&db, module, "valid_top")
            .leading_comments(&db)
            .is_empty()
    );

    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    let nested_error = contract.items(&db)[0];
    assert!(matches!(nested_error, ContractItem::Error { .. }));
    assert_comment_texts(
        nested_error.leading_comments(&db),
        &[" invalid contract item"],
    );
    assert!(
        contract_function(&db, module, "valid_nested")
            .leading_comments(&db)
            .is_empty()
    );
}

#[test]
fn equivalent_type_and_predicate_refs_share_semantic_shapes_without_sharing_occurrences() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "type-ref-shapes",
        "trait C<self> {}
         function a(x: word) {}
         function b(y: word) {}
         function c<t>(x: t) where t: C {}
         function d<t>(x: t) where t: C {}",
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
fn expression_statements_stay_expressions_and_explicit_returns_stay_returns() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "explicit-return",
        "function expression() returns (word) { 1; }
         function explicit() returns (word) { return 1; }",
    );
    assert!(
        diagnostics(&db, file).is_empty(),
        "unexpected parse diagnostics"
    );

    let expression = top_function(&db, module, "expression");
    let expression_body = expression.body(&db).expect("body");
    let expression_stmt = expression_body
        .stmts(&db)
        .get(expression_body.top_level_stmts(&db)[0]);
    assert!(matches!(&expression_stmt.kind, StmtKind::Expr(_)));

    let explicit = top_function(&db, module, "explicit");
    let explicit_body = explicit.body(&db).expect("body");
    let explicit_stmt = explicit_body
        .stmts(&db)
        .get(explicit_body.top_level_stmts(&db)[0]);
    assert!(matches!(&explicit_stmt.kind, StmtKind::Return(Some(_))));
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
fn function_types_preserve_source_arity_and_explicit_tuple_domains() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "function-types",
        "alias F = function(word) returns (function(word) returns (bool));
         alias G = function(word, bool) returns (uint);
         alias H = function((word, bool)) returns (uint);
         alias I = function() returns (uint);",
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
        panic!("F should be a function type");
    };
    assert_eq!(params.atom().len(), 1);
    assert!(matches!(ret.kind(&db), TypeRefKind::Fn { .. }));

    let g = aliases[1].ty(&db);
    let TypeRefKind::Fn { params, .. } = g.kind(&db) else {
        panic!("G should be a function type");
    };
    assert_eq!(params.atom().len(), 2);
    assert!(
        params
            .atom()
            .iter()
            .all(|param| !matches!(param.kind(&db), TypeRefKind::Tuple { .. }))
    );

    let h = aliases[2].ty(&db);
    let TypeRefKind::Fn { params, .. } = h.kind(&db) else {
        panic!("H should be a function type");
    };
    assert_eq!(params.atom().len(), 1);
    assert!(matches!(
        params.atom()[0].kind(&db),
        TypeRefKind::Tuple { .. }
    ));

    let i = aliases[3].ty(&db);
    let TypeRefKind::Fn { params, .. } = i.kind(&db) else {
        panic!("I should be a function type");
    };
    assert!(params.atom().is_empty());
}

#[test]
fn type_and_predicate_argument_list_spans_are_precise() {
    let db = TestDb::default();
    let src = "trait C<self, arg> {}
alias T = Map<word, bool>;
function f<t>(x: t) where t: C<word> {}";
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
    let expected_args_start = src.find("<word, bool>").expect("type args") as u32;
    assert_eq!(args_abs.start().as_u32(), expected_args_start);
    assert_eq!(
        args_abs.end().as_u32(),
        expected_args_start + "<word, bool>".len() as u32
    );

    let function = top_function(&db, module, "f");
    let pred = function.sig(&db).preds[0].kind(&db);
    let pred_args_abs = pred.args.span(&db).resolve_to_absolute(&db);
    let expected_pred_start = src.find("<word>").expect("predicate args") as u32;
    assert_eq!(pred_args_abs.start().as_u32(), expected_pred_start);
    assert_eq!(
        pred_args_abs.end().as_u32(),
        expected_pred_start + "<word>".len() as u32
    );
}

#[test]
fn ternary_expression_lowers_to_conditional_expression() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "ternary",
        "function f(x: bool) returns (word) { return x ? 1 : 0; }",
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

#[test]
fn compound_assignments_lower_through_binary_operator_calls() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "compound-assignments",
        "function f(x: word, y: word) {\n\
           x += y;\n\
           x -= y;\n\
           x ^= y;\n\
           x &= y;\n\
           x |= y;\n\
           x %= y;\n\
         }",
    );
    let function = top_function(&db, module, "f");
    let body = function.body(&db).expect("body");
    let expected = [
        BinOp::Add,
        BinOp::Sub,
        BinOp::BitXor,
        BinOp::BitAnd,
        BinOp::BitOr,
        BinOp::Mod,
    ];

    for (stmt_id, expected_op) in body.top_level_stmts(&db).iter().zip(expected) {
        let stmt = body.stmts(&db).get(*stmt_id);
        let StmtKind::Assign {
            op: AssignOp::Plain,
            rhs,
            ..
        } = &stmt.kind
        else {
            panic!("compound assignment should lower to plain assignment");
        };
        assert!(matches!(
            &body.exprs(&db).get(*rhs).kind,
            ExprKind::BinOp { op, .. } if *op.atom() == expected_op
        ));
    }
}

#[test]
fn boolean_and_fallback_keywords_are_rejected_as_declaration_names() {
    let db = TestDb::default();
    let contexts = [
        "function {keyword}() {}",
        "function f({keyword}: word) {}",
        "function f() returns ({keyword}: word) {}",
        "function f() { let {keyword}: word = 0; }",
        "struct {keyword} { value: word; }",
        "struct S { {keyword}: word; }",
        "enum E { {keyword} }",
        "type {keyword} is word;",
        "contract {keyword} {}",
        "contract C { {keyword}: word; }",
        "alias A<{keyword}> = word;",
        "import * as {keyword} from source;",
    ];

    for keyword in ["true", "false", "fallback"] {
        let expected = format!("`{keyword}` is reserved and cannot be used as an identifier");
        for (index, context) in contexts.iter().enumerate() {
            let source = context.replace("{keyword}", keyword);
            let file = source_file(&db, &format!("reserved-{keyword}-{index}"), &source);
            let messages = diagnostics(&db, file)
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>();
            assert!(
                messages.iter().any(|message| message == &expected),
                "missing reserved-name diagnostic for `{source}`: {messages:#?}"
            );
        }
    }
}

#[test]
fn boolean_values_patterns_and_special_fallback_remain_valid() {
    let db = TestDb::default();
    let source = r#"
contract Keywords {
  fallback() external {}
}

function negate(value: bool) returns (bool) {
  match (value) {
    case true { return false; }
    case false { return true; }
  }
}

function fallbackHandler() {}
"#;
    let file = source_file(&db, "reserved-keyword-positive", source);
    let diagnostics = diagnostics(&db, file);
    assert!(
        diagnostics.is_empty(),
        "keyword literals and the special fallback declaration should parse: {diagnostics:#?}"
    );
}
