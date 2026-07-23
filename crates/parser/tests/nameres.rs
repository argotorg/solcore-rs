use hir::{
    ast::{
        function::{ExprKind, FuncBody},
        item::{ContractItem, FunctionDef, Item, Module},
    },
    diag::Diagnostic,
    input::SourceFile,
    nameres::{
        DefResolutionKind, EmptyImportedNames, ImportedNames, ModuleRef, NameresDiagnostic,
        NameresDiagnosticPolicy, Namespace, Resolution, UndefinedNameKind, item_scope,
        resolve_module, resolve_module_with_imports_and_policy,
    },
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

fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
    let file = source_file(db, "nameres", src);
    parse_file_to_hir(db, file).module(db)
}

fn parse_and_module<'db>(db: &'db TestDb, name: &str, src: &str) -> (SourceFile, Module<'db>) {
    let file = source_file(db, name, src);
    let module = parse_file_to_hir(db, file).module(db);
    (file, module)
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

struct ModuleOnlyImports<'db> {
    owner: Module<'db>,
}

struct UnknownWildcardImports;

impl<'db> ImportedNames<'db> for UnknownWildcardImports {
    fn imported(
        &self,
        _db: &'db dyn hir::Db,
        _namespace: Namespace,
        _name: &str,
    ) -> Option<Resolution<'db>> {
        None
    }

    fn may_contain_unknown_unqualified(
        &self,
        _db: &'db dyn hir::Db,
        _namespace: Namespace,
        _name: &str,
    ) -> bool {
        true
    }
}

impl<'db> ImportedNames<'db> for ModuleOnlyImports<'db> {
    fn imported(
        &self,
        db: &'db dyn hir::Db,
        namespace: Namespace,
        name: &str,
    ) -> Option<Resolution<'db>> {
        (namespace == Namespace::Module && name == "math").then(|| {
            Resolution::Module(ModuleRef {
                owner: self.owner.def_id_value(db),
                name: name.to_owned(),
            })
        })
    }
}

#[test]
fn parse_recovery_suppression_policy_silences_name_lookup_cascades() {
    let cases = [
        (
            "body_expr_error",
            "function f() returns (word) {
               let x = ;
               return missing;
             }",
        ),
        (
            "lost_function_signature",
            "lost(x: word) returns (word) { return 0; }
             function caller() returns (word) { return lost(0); }",
        ),
        (
            "broken_import",
            "impoort util;
             function caller() returns (word) { return missing; }",
        ),
        (
            "broken_type_annotation",
            "typeish Alias = word;
             function caller(x: Alias) returns (word) { return 0; }",
        ),
        (
            "top_level_item_error",
            "function first() {}
             unknown nonsense tokens
             function second() {}
             function caller() returns (word) { return missing; }",
        ),
        (
            "broken_contract_member",
            "contract C {
               broken nonsense;
               function get() returns (word) { return broken; }
             }",
        ),
    ];

    for (name, src) in cases {
        let db = TestDb::default();
        let (file, module) = parse_and_module(&db, name, src);
        let parse_count = parse_diagnostics(&db, file).len();
        assert!(parse_count > 0, "probe `{name}` should have parse errors");
        let scope = item_scope(&db, module);
        let imports = EmptyImportedNames;
        let resolution = resolve_module_with_imports_and_policy(
            &db,
            module,
            scope,
            &imports,
            NameresDiagnosticPolicy::SuppressForParseErrors,
        );
        assert!(
            resolution.diagnostics.is_empty(),
            "parse-broken probe `{name}` should not publish nameres diagnostics"
        );
        if name == "body_expr_error" {
            assert!(
                resolution
                    .bodies
                    .iter()
                    .flat_map(|map| &map.exprs)
                    .any(|entry| {
                        matches!(&entry.body.exprs(&db).get(entry.expr).kind, ExprKind::Error)
                            && matches!(entry.resolution, Resolution::Err)
                    }),
                "recovered expression errors should resolve to Resolution::Err"
            );
        }
    }
}

#[test]
fn undefined_name_kind_distinguishes_bare_terms_from_path_lookups() {
    let db = TestDb::default();
    let (file, module) = parse_and_module(
        &db,
        "undefined_name_kinds",
        "enum Local { Present }
         function bare() returns (word) { return missing; }
         function qualified() returns (word) { return math.value(); }
         function ctorExpr() returns (word) { return Option.Some(0); }
         function ctorPat(x: word) returns (word) {
           match (x) { case Option.Some(y) { return y; } default { return 0; } }
         }
         function valueMember(x: word) returns (word) { return x.absent; }
         function member() returns (word) { return Local.absent; }",
    );
    assert!(parse_diagnostics(&db, file).is_empty());

    let resolution = resolve_module(&db, module);
    let diagnostics = resolution
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match diagnostic {
            NameresDiagnostic::UndefinedName { name, kind, .. } => {
                Some((name.as_str(), kind.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        diagnostics,
        [
            ("missing", UndefinedNameKind::Term),
            (
                "math",
                UndefinedNameKind::ModuleQualifier {
                    access_path: "math.value".to_owned(),
                },
            ),
            (
                "Option",
                UndefinedNameKind::ModuleQualifier {
                    access_path: "Option.Some".to_owned(),
                },
            ),
            (
                "Option.Some",
                UndefinedNameKind::QualifiedConstructor {
                    access_path: "Option.Some".to_owned(),
                },
            ),
            ("absent", UndefinedNameKind::Field),
        ]
    );
}

#[test]
fn missing_resolved_module_member_has_qualified_lookup_context() {
    let db = TestDb::default();
    let (file, module) = parse_and_module(
        &db,
        "missing_module_member",
        "enum Local { Present }
         function missing() returns (word) {
           let fromModule = math.value();
           return Local.absent;
         }",
    );
    assert!(parse_diagnostics(&db, file).is_empty());

    let scope = item_scope(&db, module);
    let imports = ModuleOnlyImports { owner: module };
    let resolution = resolve_module_with_imports_and_policy(
        &db,
        module,
        scope,
        &imports,
        NameresDiagnosticPolicy::Emit,
    );
    let diagnostics = resolution
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match diagnostic {
            NameresDiagnostic::UndefinedName { name, kind, .. } => {
                Some((name.as_str(), kind.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        diagnostics,
        [
            (
                "value",
                UndefinedNameKind::ModuleMember {
                    access_path: "math.value".to_owned(),
                },
            ),
            ("absent", UndefinedNameKind::Field),
        ]
    );
}

#[test]
fn missing_constructor_on_resolved_type_is_not_an_import_context() {
    let db = TestDb::default();
    let (file, module) = parse_and_module(
        &db,
        "missing_local_constructor",
        "enum Option { None }
         function missing(value: Option) returns (word) {
           match (value) { case Option.Some { return 1; } default { return 0; } }
         }",
    );
    assert!(parse_diagnostics(&db, file).is_empty());

    let resolution = resolve_module(&db, module);
    let diagnostics = resolution
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match diagnostic {
            NameresDiagnostic::UndefinedName { name, kind, .. } => {
                Some((name.as_str(), kind.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(diagnostics, [("Option.Some", UndefinedNameKind::Field)]);
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
        "function f(x: word) returns (word) {
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
        "function f(x: word) returns (word) {
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
        "function balance() returns (word) { return 0; }
         contract C {
           balance: word;
           function f() returns (word) { return balance; }
           function g(balance: word) returns (word) { return balance; }
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
fn unqualified_call_callee_prefers_contract_function_over_same_name_field() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "contract C {
           balance: word;
           function balance() returns (word) { return 7; }
           function call() returns (word) { return balance(); }
           function bare() returns (word) { return balance; }
         }",
    );
    assert!(diagnostic_codes(&db, module).is_empty());

    let call_function = contract_function(&db, module, "C", "call");
    let call_body = call_function.body(&db).expect("body");
    let call_map = body_map(&db, module, call_body);
    let call_events = ident_resolutions(&db, call_body, &call_map);
    let callee = call_events
        .iter()
        .find(|(name, _)| *name == "balance")
        .expect("call callee");
    assert!(matches!(
        callee.1,
        Resolution::Def {
            kind: DefResolutionKind::Function,
            ..
        }
    ));

    let bare_function = contract_function(&db, module, "C", "bare");
    let bare_body = bare_function.body(&db).expect("body");
    let bare_map = body_map(&db, module, bare_body);
    let bare_events = ident_resolutions(&db, bare_body, &bare_map);
    let bare = bare_events
        .iter()
        .find(|(name, _)| *name == "balance")
        .expect("bare reference");
    assert!(matches!(bare.1, Resolution::Field(_)));
}

#[test]
fn qualified_ctor_and_class_method_resolve_as_expected() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "enum Option { None, Some(word) }
         enum Foo { Foo(word) }
         trait Show<self> { function show(x: self) returns (word); }
         function good(x: word) returns (Option) { return Option.Some(x); }
         function classCall(x: word) returns (word) { return Show.show(x); }
         function qualified(x: word) returns (Option) { return Option.Some(x); }
         function sameName(x: word) returns (Foo) { return Foo.Foo(x); }",
    );
    let codes = diagnostic_codes(&db, module);
    assert!(codes.is_empty());

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

    let qualified = top_function(&db, module, "qualified");
    let qualified_body = qualified.body(&db).expect("body");
    let qualified_map = body_map(&db, module, qualified_body);
    assert!(
        qualified_map
            .exprs
            .iter()
            .any(|entry| entry.body == qualified_body
                && matches!(entry.resolution, Resolution::Ctor { .. }))
    );
}

#[test]
fn self_qualified_contract_methods_do_not_shadow_same_named_local_adt_constructors() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Option {
  enum Option<a> { None, Some(a) }

  function some(x: word) returns (Option<word>) {
    return Option.Some(x);
  }

  function none() returns (Option<word>) {
    return Option.None;
  }

  function read(o: Option<word>) returns (word) {
    match (o) { case Option.Some(x) { return x; } case Option.None { return 0; } }
  }
}
"#,
    );
    assert!(diagnostic_codes(&db, module).is_empty());

    for name in ["some", "none"] {
        let function = contract_function(&db, module, "Option", name);
        let body = function.body(&db).expect("body");
        let map = body_map(&db, module, body);
        assert!(
            map.exprs.iter().any(|entry| {
                entry.body == body && matches!(entry.resolution, Resolution::Ctor { .. })
            }),
            "Option.{name} should resolve its qualified constructor expression"
        );
    }

    let read = contract_function(&db, module, "Option", "read");
    let read_body = read.body(&db).expect("body");
    let read_map = body_map(&db, module, read_body);
    assert_eq!(
        read_map
            .pats
            .iter()
            .filter(|entry| {
                entry.body == read_body && matches!(entry.resolution, Resolution::Ctor { .. })
            })
            .count(),
        2
    );
}

#[test]
fn unqualified_same_name_constructor_is_rejected_with_unknown_wildcard_import() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "// migrate-syntax: keep-unqualified-constructor
         enum Unit { Unit }
         function make() returns (Unit) { return Unit; }",
    );
    let function = top_function(&db, module, "make");
    let body = function.body(&db).expect("body");
    let scope = item_scope(&db, module);
    let resolution = resolve_module_with_imports_and_policy(
        &db,
        module,
        scope,
        &UnknownWildcardImports,
        NameresDiagnosticPolicy::Emit,
    );
    let body_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == body))
        .expect("body map");
    let events = ident_resolutions(&db, body, body_map);

    assert!(
        events
            .iter()
            .any(|(name, resolution)| *name == "Unit" && matches!(resolution, Resolution::Err)),
        "unqualified same-name constructor should be rejected: {events:#?}"
    );
    assert!(
        resolution.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            NameresDiagnostic::UnqualifiedConstructor {
                name,
                qualification: Some(qualification),
                ..
            } if name == "Unit" && qualification == "Unit.Unit"
        )),
        "expected an actionable qualification diagnostic: {:#?}",
        resolution.diagnostics
    );
}
