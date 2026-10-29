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
    let url = format!("memory:///{name}.sol").parse().expect("valid url");
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
fn empty_loop_post_clauses_have_valid_scope_spans() {
    for separator in ["", " ", "\n    ", " /* empty post */ "] {
        let source = format!(
            "pragma solcore; function f() {{ for (let i = 0; false;{separator}) {{}} while (false) {{}} }}"
        );
        let db = TestDb::default();
        let (file, module) = parse_module(&db, "empty_loop_post", &source);
        assert!(diagnostics(&db, file).is_empty(), "{source}");
        let body = top_function(&db, module, "f").body(&db).expect("body");
        for (_, stmt) in body.stmts(&db).iter() {
            let span = stmt.span.resolve_to_absolute(&db);
            assert!(span.start() <= span.end(), "{source}: {stmt:?}");
            assert!(span.end().as_usize() <= source.len());
        }
    }
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
        "/* **/ /* outer /* inner */ done */ pragma solcore; function f() {}",
    );
    assert_eq!(
        (*top_function(&db, module, "f").sig(&db).name.atom()).text(&db),
        "f"
    );

    let file = source_file(
        &db,
        "block-comment-bad",
        "pragma solcore;\n/* unterminated\nfunction f() {}",
    );
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
    // The arrow-like text below is comment payload under test, not a legacy
    // function result annotation.
    let (_, module) = parse_module(
        &db,
        "function-comments",
        r#"
pragma solcore;
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
pragma solcore;
// top import
import dependency;
// top export
export dependency;
// top pragma
pragma feature Example;
// top alias
type Alias = word;
// top enum
enum TopData { // first constructor after opening brace
  First
  // second constructor before separator
  , Second
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
  type LocalAlias = word;
  // contract enum
  enum LocalData {
    // local first constructor
    LocalFirst
    , // local second constructor after separator
      LocalSecond
  }
  // contract constructor
  constructor() {}
  // contract fallback
  fallback() {}
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
    // The leading `pragma solcore;` has no comment of its own.
    let [Item::Pragma(_), items @ ..] = module.items(&db).as_slice() else {
        panic!("expected the leading pragma solcore");
    };
    assert_eq!(items.len(), expected_top_comments.len());
    for (item, expected) in items.iter().zip(expected_top_comments) {
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
        &[" first constructor after opening brace"],
    );
    assert_comment_texts(
        top_adt.ctor_leading_comments(&db, 1).expect("second ctor"),
        &[" second constructor before separator"],
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
        &[" local second constructor after separator"],
    );
}

#[test]
fn item_comments_do_not_cross_blank_lines_trailing_code_or_bodies() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "item-comment-boundaries",
        r#"
pragma solcore;
type Owner = word; // trailing top-level comment
enum AfterTrailing {}
// separated top-level comment

trait Boundary<a> {
  // separated method comment

  function method(x: a) returns (word);
}
contract C {
  first: word; // trailing field comment
  type AfterTrailingField = word;
  // separated field comment

  second: word;
  enum Nested { First // trailing constructor comment
    , Second
    // separated from the constructor name by a blank line after `,`
    ,

    Third
  }
  function body_owner() {
    // body-only comment
  }
  type AfterBody = word;
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
pragma solcore;
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

    let top_error = module.items(&db)[1];
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
        "pragma solcore;\ntrait C<self> {}
         function a(x: word) {}
         function b(y: word) {}
         function c<t>(x: t) where t:C {}
         function d<t>(x: t) where t:C {}",
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
fn implicit_return_applies_only_to_named_function_tail_expressions() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "implicit-return",
        "pragma solcore;\nfunction id(x: word) returns (word) { x }
         function sequence(x: word) returns (word) { let copy = x; copy }
         function discarded(x: word) { x; }
         function make() returns (function(word)) { return lam (x: word) { return x; }; }",
    );

    let id = top_function(&db, module, "id");
    let id_body = id.body(&db).expect("body");
    let id_stmt = id_body.stmts(&db).get(id_body.top_level_stmts(&db)[0]);
    assert!(matches!(&id_stmt.kind, StmtKind::Return(_)));

    let sequence = top_function(&db, module, "sequence");
    let sequence_body = sequence.body(&db).expect("body");
    let sequence_stmts = sequence_body.top_level_stmts(&db);
    assert_eq!(sequence_stmts.len(), 2);
    assert!(matches!(
        &sequence_body.stmts(&db).get(sequence_stmts[1]).kind,
        StmtKind::Return(_)
    ));

    let discarded = top_function(&db, module, "discarded");
    let discarded_body = discarded.body(&db).expect("body");
    let discarded_stmt = discarded_body
        .stmts(&db)
        .get(discarded_body.top_level_stmts(&db)[0]);
    assert!(matches!(&discarded_stmt.kind, StmtKind::Expr(_)));

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
    assert!(matches!(&lambda_stmt.kind, StmtKind::Return(_)));

    let (file, _) = parse_module(
        &db,
        "lambda-tail-expression",
        "pragma solcore; function invalid() returns (function(word)) { return lam (x: word) { x }; }",
    );
    let diagnostics = diagnostics(&db, file);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("expression statement requires trailing `;`")),
        "missing lambda tail-expression diagnostic: {diagnostics:#?}"
    );
}

#[test]
fn constructor_and_fallback_tail_expressions_require_semicolons() {
    let db = TestDb::default();
    let (file, _) = parse_module(
        &db,
        "entry-tail-expression",
        "pragma solcore;\ncontract C {
           constructor() { (); }
           fallback() { () }
         }",
    );
    let diagnostics = diagnostics(&db, file);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("expression statement requires trailing `;`")),
        "missing fallback tail-expression diagnostic: {diagnostics:#?}"
    );
}

#[test]
fn named_parameters_are_typed_while_lambda_parameters_may_be_inferred() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "parameter-annotations",
        "pragma solcore;\nfunction apply(value: word) returns (word) {
           let identity = lam (inferred) { return inferred; };
           return identity(value);
         }",
    );
    assert!(diagnostics(&db, file).is_empty());

    let apply = top_function(&db, module, "apply");
    assert!(matches!(
        apply.sig(&db).params.atom().as_slice(),
        [FuncParam::Typed { .. }]
    ));
    let body = apply.body(&db).expect("body");
    let lambda_params = body
        .exprs(&db)
        .iter()
        .find_map(|(_, expr)| match &expr.kind {
            ExprKind::Lambda { params, .. } => Some(params.atom()),
            _ => None,
        })
        .expect("lambda expression");
    assert!(matches!(
        lambda_params.as_slice(),
        [FuncParam::Untyped { comptime: None, .. }]
    ));

    // These two sources intentionally omit the annotation to assert the
    // canonical named-parameter rejection rule.
    for (name, source) in [
        (
            "untyped-named-parameter",
            "pragma solcore; function invalid(value) {}",
        ),
        (
            "untyped-comptime-parameter",
            "pragma solcore; function invalid(comptime value) {}",
        ),
    ] {
        let file = source_file(&db, name, source);
        assert!(diagnostics(&db, file).iter().any(|diagnostic| {
            diagnostic.message == "named function parameter requires an explicit type"
        }));
    }
}

#[test]
fn omitted_named_return_is_explicit_unit_even_when_the_body_returns_a_value() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "omitted-return-is-unit",
        "pragma solcore;\nfunction noValue() {}
         function valueInBody() { return 1; }
         trait UnitMethod<T> { function unit(value: T); }",
    );
    assert!(diagnostics(&db, file).is_empty());

    for name in ["noValue", "valueInBody"] {
        let ret = top_function(&db, module, name)
            .sig(&db)
            .ret
            .expect("omitted `returns` lowers to an explicit unit type");
        assert!(matches!(ret.kind(&db), TypeRefKind::Tuple { elems } if elems.atom().is_empty()));
    }

    let trait_method = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => class.methods(&db).first().cloned(),
            _ => None,
        })
        .expect("trait method");
    let ret = trait_method
        .ret
        .expect("trait method omission lowers to unit");
    assert!(matches!(ret.kind(&db), TypeRefKind::Tuple { elems } if elems.atom().is_empty()));
}

#[test]
fn core_bindings_and_assignments_reject_yul_colon_equals() {
    let db = TestDb::default();
    // These are intentional legacy-rejection probes. `:=` remains valid only
    // within an `assembly` block; canonical Core uses `=`.
    // syntax-migration: preserve-literals-begin
    for (name, source) in [
        (
            "colon-equals-binding",
            "pragma solcore; function invalid() { let value := 1; }",
        ),
        (
            "colon-equals-assignment",
            "pragma solcore; function invalid() { value := 1; }",
        ),
    ] {
        let file = source_file(&db, name, source);
        assert!(
            !diagnostics(&db, file).is_empty(),
            "Core `:=` unexpectedly accepted in {name}"
        );
    }
    // syntax-migration: preserve-literals-end
}

#[test]
fn contract_fields_can_be_interleaved_and_have_initializers() {
    let db = TestDb::default();
    let (_, module) = parse_module(
        &db,
        "contract-fields",
        "pragma solcore;\ncontract C {
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
    let src = "pragma solcore;\nimport core.math
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
        "pragma solcore;\ntype F = function(word) returns (function(word) returns (bool));
         type G = function(word, bool) returns (uint);
         type H = function((word, bool)) returns (uint);
         type I = function() returns (uint);",
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
    let src = "pragma solcore;\ntrait C<self, arg> {}
type T = Map<word, bool>;
function f<t>(x: t) where t:C<word> {}";
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
        "pragma solcore; function f(x: bool) returns (word) { return x ? 1 : 0; }",
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
fn ternary_expression_is_right_associative_and_allows_a_nested_then_arm() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "nested-ternary",
        "pragma solcore;\nfunction right(x: bool, y: bool) returns (word) {
           return x ? 1 : y ? 2 : 3;
         }
         function nestedThen(x: bool, y: bool) returns (word) {
           return x ? y ? 1 : 2 : 3;
         }",
    );
    assert!(diagnostics(&db, file).is_empty());

    let conditional_parts = |name| {
        let function = top_function(&db, module, name);
        let body = function.body(&db).expect("body");
        let stmt = body.stmts(&db).get(body.top_level_stmts(&db)[0]);
        let StmtKind::Return(Some(expr_id)) = &stmt.kind else {
            panic!("expected return with expression");
        };
        let ExprKind::If {
            then_expr,
            else_expr,
            ..
        } = &body.exprs(&db).get(*expr_id).kind
        else {
            panic!("expected outer conditional expression");
        };
        (
            matches!(&body.exprs(&db).get(*then_expr).kind, ExprKind::If { .. }),
            matches!(&body.exprs(&db).get(*else_expr).kind, ExprKind::If { .. }),
        )
    };

    assert_eq!(conditional_parts("right"), (false, true));
    assert_eq!(conditional_parts("nestedThen"), (true, false));
}

#[test]
fn array_literals_lower_with_empty_nested_and_postfix_index_forms() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "array-literals",
        r#"
pragma solcore;
function f(a: word, b: word) returns (word) {
  let empty = [];
  let nested = [[a], [b]];
  return [a, b][0];
}
"#,
    );
    assert!(diagnostics(&db, file).is_empty());

    let body = top_function(&db, module, "f").body(&db).expect("body");
    let mut lengths = body
        .exprs(&db)
        .iter()
        .filter_map(|(_, expr)| match &expr.kind {
            ExprKind::Array(elems) => Some(elems.len()),
            _ => None,
        })
        .collect::<Vec<_>>();
    lengths.sort_unstable();
    assert_eq!(lengths, [0, 1, 1, 2, 2]);

    let indexed_array = body
        .exprs(&db)
        .iter()
        .find_map(|(_, expr)| match &expr.kind {
            ExprKind::Index { base, .. } => Some(body.exprs(&db).get(*base)),
            _ => None,
        });
    assert!(matches!(
        indexed_array.map(|expr| &expr.kind),
        Some(ExprKind::Array(elems)) if elems.len() == 2
    ));
}

#[test]
fn unary_minus_lowers_literals_before_conversion_and_values_through_subtraction() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "unary-minus",
        "pragma solcore; function f(x: word) { let decimal = -128; let hex = -(0x80); return -x * 2; }",
    );
    assert!(diagnostics(&db, file).is_empty());
    let body = top_function(&db, module, "f").body(&db).expect("body");
    let exprs = body.exprs(&db);
    assert!(exprs.iter().any(|(_, expr)| matches!(
        &expr.kind,
        ExprKind::Lit(hir::ast::function::LitKind::Number(value)) if value == "-128"
    )));
    assert!(exprs.iter().any(|(_, expr)| matches!(
        &expr.kind,
        ExprKind::Lit(hir::ast::function::LitKind::Hex(value)) if value == "-0x80"
    )));
    let stmt = body
        .stmts(&db)
        .get(*body.top_level_stmts(&db).last().unwrap());
    let StmtKind::Return(Some(result)) = &stmt.kind else {
        panic!("expected return");
    };
    let ExprKind::BinOp { lhs, op, .. } = &exprs.get(*result).kind else {
        panic!("expected multiplication");
    };
    assert_eq!(*op.atom(), BinOp::Mul);
    let ExprKind::BinOp { lhs: zero, op, rhs } = &exprs.get(*lhs).kind else {
        panic!("expected negation lowered to subtraction");
    };
    assert_eq!(*op.atom(), BinOp::Sub);
    assert!(matches!(
        &exprs.get(*zero).kind,
        ExprKind::Lit(hir::ast::function::LitKind::Number(value)) if value == "0"
    ));
    assert!(matches!(&exprs.get(*rhs).kind, ExprKind::Ident(_)));
}

#[test]
fn compound_assignments_lower_through_binary_operator_calls() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "compound-assignments",
        "pragma solcore;\nfunction f(x: word, y: word) {\n\
           x += y;\n\
           x -= y;\n\
           x *= y;\n\
           x /= y;\n\
           x ^= y;\n\
           x &= y;\n\
           x |= y;\n\
           x %= y;\n\
           x <<= y;\n\
           x >>= y;\n\
           x ~=;\n\
         }",
    );
    assert!(
        diagnostics(&db, file).is_empty(),
        "{:?}",
        diagnostics(&db, file)
    );
    let function = top_function(&db, module, "f");
    let body = function.body(&db).expect("body");
    let expected = [
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::BitXor,
        BinOp::BitAnd,
        BinOp::BitOr,
        BinOp::Mod,
        BinOp::Shl,
        BinOp::Shr,
    ];

    let stmts = body.top_level_stmts(&db);
    for (stmt_id, expected_op) in stmts.iter().zip(expected) {
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

    let stmt = body
        .stmts(&db)
        .get(*stmts.last().expect("bit-not assignment"));
    let StmtKind::Assign {
        op: AssignOp::Plain,
        rhs,
        ..
    } = &stmt.kind
    else {
        panic!("bit-not assignment should lower to plain assignment");
    };
    assert!(matches!(
        &body.exprs(&db).get(*rhs).kind,
        ExprKind::UnaryOp { op, .. } if *op.atom() == hir::ast::function::UnOp::BitNot
    ));
}

fn bin_ops(db: &TestDb, body: hir::ast::function::FuncBody<'_>) -> Vec<BinOp> {
    let mut ops = body
        .exprs(db)
        .iter()
        .filter_map(|(_, expr)| match &expr.kind {
            ExprKind::BinOp { op, .. } => Some(*op.atom()),
            _ => None,
        })
        .collect::<Vec<_>>();
    ops.sort_by_key(|op| format!("{op:?}"));
    ops
}

#[test]
fn unchecked_blocks_lower_arithmetic_to_unchecked_operators() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "unchecked-block",
        "pragma solcore;\nfunction f(x: word, y: word) returns (word) {\n\
           let a: word = x + y;\n\
           unchecked {\n\
             let b: word = x + y - x * y / y % y;\n\
             x += y;\n\
             let c: bool = x < y;\n\
             let d: word = x & y;\n\
           }\n\
           return a - y;\n\
         }",
    );
    assert!(diagnostics(&db, file).is_empty());
    let body = top_function(&db, module, "f").body(&db).expect("body");
    let mut expected = vec![
        // Outside the block: checked.
        BinOp::Add,
        BinOp::Sub,
        // Inside the block: arithmetic only, including the `+=` desugaring.
        BinOp::UncheckedAdd,
        BinOp::UncheckedSub,
        BinOp::UncheckedMul,
        BinOp::UncheckedDiv,
        BinOp::UncheckedMod,
        BinOp::UncheckedAdd,
        BinOp::Lt,
        BinOp::BitAnd,
    ];
    expected.sort_by_key(|op| format!("{op:?}"));
    assert_eq!(bin_ops(&db, body), expected);
}

#[test]
fn lambdas_inside_unchecked_blocks_keep_checked_arithmetic() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "unchecked-lambda",
        "pragma solcore;\nfunction f(x: word, y: word) returns (word) {\n\
           unchecked {\n\
             let g = lam (v: word) -> word { return v + y; };\n\
             return g(x) + y;\n\
           }\n\
         }",
    );
    assert!(diagnostics(&db, file).is_empty());
    let body = top_function(&db, module, "f").body(&db).expect("body");
    assert_eq!(bin_ops(&db, body), [BinOp::UncheckedAdd]);
    let lambda_body = body
        .exprs(&db)
        .iter()
        .find_map(|(_, expr)| match &expr.kind {
            ExprKind::Lambda { body, .. } => Some(*body),
            _ => None,
        })
        .expect("lambda");
    assert_eq!(bin_ops(&db, lambda_body), [BinOp::Add]);
}

#[test]
fn nested_unchecked_blocks_are_rejected() {
    let db = TestDb::default();
    let (file, _) = parse_module(
        &db,
        "nested-unchecked",
        "pragma solcore;\nfunction f(x: word) returns (word) {\n\
           unchecked {\n\
             unchecked { return x + x; }\n\
           }\n\
         }",
    );
    let messages = diagnostics(&db, file)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    assert_eq!(messages, ["`unchecked` blocks cannot be nested"]);
}

#[test]
fn unchecked_remains_a_valid_identifier() {
    let db = TestDb::default();
    let (file, module) = parse_module(
        &db,
        "unchecked-identifier",
        "pragma solcore;\nfunction unchecked(x: word) returns (word) { return x; }\n\
         function f(unchecked: word) returns (word) {\n\
           let r: word = unchecked;\n\
           return r;\n\
         }",
    );
    assert!(diagnostics(&db, file).is_empty());
    let body = top_function(&db, module, "f").body(&db).expect("body");
    assert!(bin_ops(&db, body).is_empty());
}

#[test]
fn derive_attributes_lower_qualified_targets_and_precise_spans() {
    let db = TestDb::default();
    let src = "pragma solcore;\n#[derive(Eq, core.Show)] enum Top<a> { Top(a) }\n\
contract C { #[derive(pkg.codec.Encode)] enum Local {} }";
    let (file, module) = parse_module(&db, "derive-attributes", src);
    let diagnostics = diagnostics(&db, file);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:#?}"
    );

    let top = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("top-level derived ADT");
    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    let local = contract
        .items(&db)
        .iter()
        .find_map(|item| match item {
            ContractItem::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("contract-local derived ADT");

    let top_targets = top
        .derives(&db)
        .iter()
        .map(|target| (*target.atom()).text(&db))
        .collect::<Vec<_>>();
    assert_eq!(top_targets, ["Eq", "core.Show"]);
    assert_eq!(
        local
            .derives(&db)
            .iter()
            .map(|target| (*target.atom()).text(&db))
            .collect::<Vec<_>>(),
        ["pkg.codec.Encode"]
    );

    let assert_span = |span: hir::span::Span<'_>, expected: &str| {
        let absolute = span.resolve_to_absolute(&db);
        let start = src.find(expected).expect("expected source fragment") as u32;
        assert_eq!(absolute.start().as_u32(), start);
        assert_eq!(absolute.end().as_u32(), start + expected.len() as u32);
    };
    assert_span(
        top.derive_attr_span(&db)
            .expect("top derive attribute span"),
        "#[derive(Eq, core.Show)]",
    );
    assert_span(top.derives(&db)[0].span(&db), "Eq");
    assert_span(top.derives(&db)[1].span(&db), "core.Show");
    assert_span(
        local
            .derive_attr_span(&db)
            .expect("local derive attribute span"),
        "#[derive(pkg.codec.Encode)]",
    );
    assert_span(local.derives(&db)[0].span(&db), "pkg.codec.Encode");
}

#[test]
fn invalid_derive_attributes_diagnose_and_keep_following_declarations() {
    let db = TestDb::default();
    // syntax-migration: preserve-next-literal
    let src = r#"
pragma solcore;
#[derive()] enum Empty {}
#[derive(Eq,)] enum Malformed {}
#[derive(Eq)] function kept() {}
enum After {}
contract C {
  #[derive(Eq)] field: word;
  #[derive(Eq)] function nested() {}
  #[derive()] enum EmptyLocal {}
  enum AfterLocal {}
}
"#;
    let (file, module) = parse_module(&db, "invalid-derive-attributes", src);
    let messages = diagnostics(&db, file)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 6, "unexpected diagnostics: {messages:#?}");
    assert_eq!(
        messages
            .iter()
            .filter(|message| {
                message.as_str() == "derive attribute requires at least one trait path"
            })
            .count(),
        2
    );
    assert!(messages.iter().any(|message| {
        message == "malformed derive attribute; expected `#[derive(Trait, ...)]`"
    }));
    assert_eq!(
        messages
            .iter()
            .filter(|message| {
                message.as_str() == "derive attribute is only allowed on enum declarations"
            })
            .count(),
        3
    );

    assert!(top_function(&db, module, "kept").body(&db).is_some());
    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    assert_eq!(contract.fields(&db).len(), 1);
    assert!(contract_function(&db, module, "nested").body(&db).is_some());
    assert_eq!(
        module
            .items(&db)
            .iter()
            .filter(|item| matches!(item, Item::AdtDef(_)))
            .count(),
        3
    );
    assert_eq!(
        contract
            .items(&db)
            .iter()
            .filter(|item| matches!(item, ContractItem::AdtDef(_)))
            .count(),
        2
    );
}

#[test]
fn unclosed_derive_attribute_recovers_at_the_next_declaration() {
    let db = TestDb::default();
    let src = "pragma solcore;\n#[derive(Eq)\nenum Recovered {}\nfunction after() {}";
    let (file, module) = parse_module(&db, "unclosed-derive-attribute", src);
    assert!(!diagnostics(&db, file).is_empty());
    assert_eq!(
        module
            .items(&db)
            .iter()
            .filter(|item| matches!(item, Item::AdtDef(_)))
            .count(),
        1
    );
    assert!(top_function(&db, module, "after").body(&db).is_some());
}

#[test]
fn recovery_before_derive_preserves_top_level_and_contract_local_attributes() {
    let db = TestDb::default();
    let src = r#"
pragma solcore;
@ stray
#[derive(Eq)] enum Top {}
contract C {
  @ stray
  #[derive(Ord)] enum Local {}
}
"#;
    let (file, module) = parse_module(&db, "recovery-before-derive", src);
    assert!(!diagnostics(&db, file).is_empty());

    let top = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("top-level derived ADT after recovery");
    assert_eq!(
        top.derives(&db)
            .iter()
            .map(|target| (*target.atom()).text(&db))
            .collect::<Vec<_>>(),
        ["Eq"]
    );

    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    let local = contract
        .items(&db)
        .iter()
        .find_map(|item| match item {
            ContractItem::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("contract-local derived ADT after recovery");
    assert_eq!(
        local
            .derives(&db)
            .iter()
            .map(|target| (*target.atom()).text(&db))
            .collect::<Vec<_>>(),
        ["Ord"]
    );
}

#[test]
fn derive_remains_an_ordinary_identifier_outside_attributes() {
    let db = TestDb::default();
    let src =
        "pragma solcore; enum derive {} function derive() returns (derive) { return derive; }";
    let (file, module) = parse_module(&db, "derive-soft-keyword", src);
    assert!(diagnostics(&db, file).is_empty());
    assert!(module.items(&db).iter().any(|item| {
        matches!(item, Item::AdtDef(adt) if (*adt.name(&db).atom()).text(&db) == "derive")
    }));
    assert!(top_function(&db, module, "derive").body(&db).is_some());
}

#[test]
fn unclosed_derive_does_not_consume_later_declarations_or_contract_fields() {
    let db = TestDb::default();
    // syntax-migration: preserve-next-literal
    let src = r#"
pragma solcore;
#[derive(Eq)
function kept() {}
]
enum After {}
contract C {
  #[derive(Eq)
  slot: word;
  function nested() {}
}
"#;
    let (file, module) = parse_module(&db, "derive-unclosed-boundaries", src);
    assert!(!diagnostics(&db, file).is_empty());
    assert!(top_function(&db, module, "kept").body(&db).is_some());
    assert!(module.items(&db).iter().any(|item| {
        matches!(item, Item::AdtDef(adt) if (*adt.name(&db).atom()).text(&db) == "After")
    }));

    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    assert_eq!(contract.fields(&db).len(), 1);
    assert!(contract_function(&db, module, "nested").body(&db).is_some());
}

#[test]
fn derive_targets_reject_reserved_identifiers() {
    let db = TestDb::default();
    let src = "pragma solcore; #[derive(fallback)] enum Kept {}";
    let (file, module) = parse_module(&db, "derive-reserved-target", src);
    assert!(!diagnostics(&db, file).is_empty());
    module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(adt) => Some(*adt),
            _ => None,
        })
        .expect("enum declaration survives malformed attribute");
}

#[path = "lowering_regressions/fixtures.rs"]
mod fixture_tests;
