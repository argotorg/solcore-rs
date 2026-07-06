use hir::{
    ast::{
        function::{ExprKind, FuncBody},
        item::{ContractItem, FunctionDef, Item, Module},
    },
    diag::Diagnostic,
    input::SourceFile,
    nameres::{Resolution, resolve_module},
};
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

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.solc").parse().expect("valid url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
    let file = source_file(db, "nameres", src);
    parse_file_to_hir(db, file).module(db)
}

fn function_name<'db>(db: &'db TestDb, function: FunctionDef<'db>) -> &'db str {
    (*function.sig(db).name.atom()).text(db)
}

fn top_function<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> FunctionDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::FunctionDef(function) if function_name(db, *function) == name => Some(*function),
            _ => None,
        })
        .expect("top-level function")
}

fn contract_function<'db>(
    db: &'db TestDb,
    module: Module<'db>,
    contract_name: &str,
    function_name_: &str,
) -> FunctionDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract)
                if (*contract.name_elem(db).atom()).text(db) == contract_name =>
            {
                contract.items(db).iter().find_map(|item| match item {
                    ContractItem::FunctionDef(function)
                        if function_name(db, *function) == function_name_ =>
                    {
                        Some(*function)
                    }
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("contract function")
}

fn diagnostics<'db>(db: &'db TestDb, module: Module<'db>) -> Vec<Diagnostic> {
    resolve_module(db, module)
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect()
}

fn diagnostic_codes(db: &TestDb, module: Module<'_>) -> Vec<String> {
    diagnostics(db, module)
        .iter()
        .filter_map(|diagnostic| diagnostic.code.clone())
        .collect()
}

fn body_map<'db>(
    db: &'db TestDb,
    module: Module<'db>,
    body: FuncBody<'db>,
) -> hir::nameres::BodyResolutionMap<'db> {
    resolve_module(db, module)
        .bodies
        .into_iter()
        .find(|map| {
            map.exprs.iter().any(|entry| entry.body == body)
                || map.stmt_bindings.iter().any(|entry| entry.body == body)
                || map.pats.iter().any(|entry| entry.body == body)
        })
        .expect("body map")
}

fn ident_resolutions<'db>(
    db: &'db TestDb,
    body: FuncBody<'db>,
    map: &hir::nameres::BodyResolutionMap<'db>,
) -> Vec<(&'db str, Resolution<'db>)> {
    map.exprs
        .iter()
        .filter(|entry| entry.body == body)
        .filter_map(|entry| match &body.exprs(db).get(entry.expr).kind {
            ExprKind::Ident(name) => Some(((*name.atom()).text(db), entry.resolution.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn let_initializer_resolves_before_binder_and_then_shadows() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "function f(x: word) -> word {
           let x = x;
           return x;
         }",
    );
    assert!(diagnostic_codes(&db, module).is_empty());

    let function = top_function(&db, module, "f");
    let body = function.body(&db).expect("body");
    let map = body_map(&db, module, body);
    let events = ident_resolutions(&db, body, &map);

    assert_eq!(
        events.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["x", "x"]
    );
    assert!(matches!(events[0].1, Resolution::Param(_)));
    assert!(matches!(events[1].1, Resolution::Local(_)));
}

#[test]
fn explicit_blocks_scope_locals_but_for_body_lets_leak() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "function f(x: word) -> word {
           {
             let x = x;
           }
           for (let i = x; i; i = i) {
             let j = i;
           }
           return j;
         }",
    );
    assert!(diagnostic_codes(&db, module).is_empty());

    let function = top_function(&db, module, "f");
    let body = function.body(&db).expect("body");
    let map = body_map(&db, module, body);
    let events = ident_resolutions(&db, body, &map);

    let return_j = events
        .iter()
        .rev()
        .find(|(name, _)| *name == "j")
        .expect("return j");
    assert!(matches!(return_j.1, Resolution::Local(_)));
}

#[test]
fn contract_fields_beat_top_level_functions_and_params_shadow_fields() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "function balance() -> word { return 0; }
         contract C {
           balance: word;
           function f() -> word { return balance; }
           function g(balance: word) -> word { return balance; }
         }",
    );
    assert!(diagnostic_codes(&db, module).is_empty());

    let field_function = contract_function(&db, module, "C", "f");
    let field_body = field_function.body(&db).expect("body");
    let field_map = body_map(&db, module, field_body);
    let field_events = ident_resolutions(&db, field_body, &field_map);
    assert!(matches!(field_events[0].1, Resolution::Field(_)));

    let param_function = contract_function(&db, module, "C", "g");
    let param_body = param_function.body(&db).expect("body");
    let param_map = body_map(&db, module, param_body);
    let param_events = ident_resolutions(&db, param_body, &param_map);
    assert!(matches!(param_events[0].1, Resolution::Param(_)));
}

#[test]
fn qualified_ctor_class_method_and_dot_ctor_resolve_as_expected() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "data Option = None | Some(word);
         data Foo = Foo(word);
         forall self . class self:Show { function show(x: self) -> word; }
         function good(x: word) -> Option { return Option.Some(x); }
         function classCall(x: word) -> word { return Show.show(x); }
         function dot(x: word) -> Option { return .Some(x); }
         function bad(x: word) -> Option { return Some(x); }
         function badSameName(x: word) -> Foo { return Foo(x); }",
    );
    let codes = diagnostic_codes(&db, module);
    assert_eq!(codes, ["SC0106", "SC0106"]);

    let good = top_function(&db, module, "good");
    let good_body = good.body(&db).expect("body");
    let good_map = body_map(&db, module, good_body);
    assert!(good_map.exprs.iter().any(
        |entry| entry.body == good_body && matches!(entry.resolution, Resolution::Ctor { .. })
    ));

    let class_call = top_function(&db, module, "classCall");
    let class_body = class_call.body(&db).expect("body");
    let class_map = body_map(&db, module, class_body);
    assert!(class_map.exprs.iter().any(|entry| entry.body == class_body
        && matches!(entry.resolution, Resolution::ClassMethod { .. })));

    let dot = top_function(&db, module, "dot");
    let dot_body = dot.body(&db).expect("body");
    let dot_map = body_map(&db, module, dot_body);
    assert!(
        dot_map.exprs.iter().any(|entry| entry.body == dot_body
            && matches!(entry.resolution, Resolution::DotCtorDeferred))
    );
}

#[test]
fn duplicate_declarations_report_two_namespace_errors_with_two_labels() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "data Foo = Foo;
         type Foo = word;
         function dup() {}
         function dup() {}",
    );
    let diagnostics = diagnostics(&db, module);
    let duplicate_diagnostics = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("SC0108"))
        .collect::<Vec<_>>();

    assert_eq!(duplicate_diagnostics.len(), 2);
    assert!(
        duplicate_diagnostics
            .iter()
            .all(|diagnostic| diagnostic.labels.len() >= 2)
    );
}

#[test]
fn undefined_name_type_and_class_have_distinct_diagnostics() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "forall a . a:MissingClass => function f(x: MissingTy) -> word {
           return missingName;
         }",
    );
    let mut codes = diagnostic_codes(&db, module);
    codes.sort();

    assert_eq!(codes, ["SC0101", "SC0103", "SC0105"]);
}
