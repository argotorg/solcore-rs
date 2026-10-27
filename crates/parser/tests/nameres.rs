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
        resolve_module, resolve_module_with_imports, resolve_module_with_imports_and_policy,
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
    let url = format!("memory:///{name}.sol").parse().expect("valid url");
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

fn top_class_id<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> hir::anchor::DefId<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) if (*class.head(db).kind(db).class.atom()).text(db) == name => {
                Some(class.def_id_value(db))
            }
            _ => None,
        })
        .expect("top-level class")
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

struct QualifiedClassImports<'db> {
    class: hir::anchor::DefId<'db>,
}

struct ClassMethodImports<'db> {
    methods: Vec<(String, Resolution<'db>)>,
}

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

impl<'db> ImportedNames<'db> for QualifiedClassImports<'db> {
    fn imported(
        &self,
        _db: &'db dyn hir::Db,
        namespace: Namespace,
        name: &str,
    ) -> Option<Resolution<'db>> {
        (namespace == Namespace::Type && name == "pkg.Eq").then_some(Resolution::Def {
            def: self.class,
            kind: DefResolutionKind::Class,
        })
    }
}

impl<'db> ImportedNames<'db> for ClassMethodImports<'db> {
    fn imported(
        &self,
        _db: &'db dyn hir::Db,
        namespace: Namespace,
        name: &str,
    ) -> Option<Resolution<'db>> {
        (namespace == Namespace::Term)
            .then(|| {
                self.methods
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, resolution)| resolution.clone())
            })
            .flatten()
    }

    fn candidate_names(&self, _db: &'db dyn hir::Db, namespace: Namespace) -> Vec<String> {
        if namespace == Namespace::Term {
            self.methods.iter().map(|(name, _)| name.clone()).collect()
        } else {
            Vec::new()
        }
    }
}

#[test]
fn derive_targets_resolve_in_source_order_for_top_level_and_contract_adts() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\ntrait Eq<a> {}\n\
         #[derive(Eq, Eq)] enum Top {}\n\
         contract C { #[derive(Eq)] enum Local {} }",
    );
    let resolution = resolve_module(&db, module);
    assert!(resolution.diagnostics.is_empty());
    let derives = &resolution.item_resolutions.derives;
    assert_eq!(derives.len(), 3);
    assert_eq!(
        derives
            .iter()
            .map(|derive| derive.index)
            .collect::<Vec<_>>(),
        [0, 1, 0]
    );
    assert_ne!(derives[0].adt, derives[2].adt);
    let class = derives[0].resolution.clone();
    assert!(matches!(
        class,
        Resolution::Def {
            kind: DefResolutionKind::Class,
            ..
        }
    ));
    assert_eq!(derives[1].resolution, class);
    assert_eq!(derives[2].resolution, class);
}

#[test]
fn derive_targets_report_unknown_and_wrong_kind_names() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore; enum NotAClass {} #[derive(Missing, NotAClass)] enum Target {}",
    );
    let resolution = resolve_module(&db, module);
    let undefined = resolution
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match diagnostic {
            NameresDiagnostic::UndefinedClass { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(undefined, ["Missing", "NotAClass"]);
    assert!(
        resolution
            .item_resolutions
            .derives
            .iter()
            .all(|derive| matches!(derive.resolution, Resolution::Err))
    );
}

#[test]
fn qualified_derive_target_uses_the_exact_imported_class_path() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore; trait Eq<a> {} #[derive(pkg.Eq)] enum Target {}",
    );
    let class = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(class.def_id_value(&db)),
            _ => None,
        })
        .expect("class");
    let scope = item_scope(&db, module);
    let imports = QualifiedClassImports { class };
    let resolution = resolve_module_with_imports(&db, module, scope, &imports);
    assert!(resolution.diagnostics.is_empty());
    assert_eq!(resolution.item_resolutions.derives.len(), 1);
    assert!(matches!(
        resolution.item_resolutions.derives[0].resolution,
        Resolution::Def {
            def,
            kind: DefResolutionKind::Class,
        } if def == class
    ));
}

#[test]
fn undefined_name_kind_distinguishes_bare_terms_from_path_lookups() {
    let db = TestDb::default();
    let (file, module) = parse_and_module(
        &db,
        "undefined_name_kinds",
        "pragma solcore;\nenum Local { Present }
         function bare() returns (word) { return missing; }
         function qualified() returns (word) { return math.value(); }
         function ctorExpr() returns (word) { return Option.Some(0); }
         function ctorPat(x: word) returns (word) {
           match (x) {
           case Option.Some(y) { return y; }
           case _ { return 0; }
           }
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
        "pragma solcore;\nenum Local { Present }
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
        "pragma solcore;\nenum Option { None }
         function missing(value: Option) returns (word) {
           match (value) {
           case Option.Some { return 1; }
           case _ { return 0; }
           }
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

fn field_resolutions<'db>(
    db: &'db TestDb,
    body: FuncBody<'db>,
    map: &hir::nameres::BodyResolutionMap<'db>,
) -> Vec<(&'db str, Resolution<'db>)> {
    map.exprs
        .iter()
        .filter(|entry| entry.body == body)
        .filter_map(|entry| match &body.exprs(db).get(entry.expr).kind {
            ExprKind::Field { field, .. } => {
                Some(((*field.atom()).text(db), entry.resolution.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn field_ufcs_resolves_a_unique_local_class_method() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\ntrait Combiner<self> {
           function combine(x: self, y: word) returns (word);
         }
         contract C {
           value: word;
           function viaUfcs(y: word) returns (word) { return value.combine(y); }
         }",
    );
    let resolution = resolve_module(&db, module);
    assert!(resolution.diagnostics.is_empty());

    let function = contract_function(&db, module, "C", "viaUfcs");
    let body = function.body(&db).expect("body");
    let map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == body))
        .expect("body map");
    let receiver = ident_resolutions(&db, body, map)
        .into_iter()
        .find(|(name, _)| *name == "value")
        .expect("UFCS receiver");
    assert!(matches!(receiver.1, Resolution::Field(_)));
    assert!(
        field_resolutions(&db, body, map)
            .into_iter()
            .any(|(name, resolution)| matches!(
                resolution,
                Resolution::ClassMethod { name: method, .. }
                    if name == "combine" && method == "combine"
            ))
    );
}

#[test]
fn field_ufcs_resolves_a_unique_imported_class_method() {
    let db = TestDb::default();
    let provider = parse_module(
        &db,
        "pragma solcore;\ntrait RemoteOps<self> {
           function touch(x: self) returns (word);
         }",
    );
    let class = top_class_id(&db, provider, "RemoteOps");
    let module = parse_module(
        &db,
        "pragma solcore;\ncontract C {
           value: word;
           function viaImport() returns (word) { return value.touch(); }
         }",
    );
    let imports = ClassMethodImports {
        methods: vec![(
            "pkg.RemoteOps.touch".to_owned(),
            Resolution::ClassMethod {
                class,
                name: "touch".to_owned(),
            },
        )],
    };
    let resolution = resolve_module_with_imports(&db, module, item_scope(&db, module), &imports);
    assert!(resolution.diagnostics.is_empty());

    let function = contract_function(&db, module, "C", "viaImport");
    let body = function.body(&db).expect("body");
    let map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == body))
        .expect("body map");
    let receiver = ident_resolutions(&db, body, map)
        .into_iter()
        .find(|(name, _)| *name == "value")
        .expect("UFCS receiver");
    assert!(matches!(receiver.1, Resolution::Field(_)));
    assert!(
        field_resolutions(&db, body, map)
            .into_iter()
            .any(|(name, resolution)| matches!(
                resolution,
                Resolution::ClassMethod { class: resolved, name: method }
                    if name == "touch" && resolved == class && method == "touch"
            ))
    );
}

#[test]
fn ufcs_reports_undefined_name_when_visible_methods_conflict() {
    let db = TestDb::default();
    let provider = parse_module(
        &db,
        "pragma solcore;\ntrait RemoteOps<self> {
           function collide(x: self) returns (word);
         }",
    );
    let remote_class = top_class_id(&db, provider, "RemoteOps");
    let module = parse_module(
        &db,
        "pragma solcore;\ntrait LocalOps<self> {
           function collide(x: self) returns (word);
         }
         contract C {
           value: word;
           function ambiguous() returns (word) { return value.collide(); }
           function ambiguousParameter(value: word) returns (word) { return value.collide(); }
           function missing() returns (word) { return value.absent(); }
         }",
    );
    let imports = ClassMethodImports {
        methods: vec![(
            "RemoteOps.collide".to_owned(),
            Resolution::ClassMethod {
                class: remote_class,
                name: "collide".to_owned(),
            },
        )],
    };
    let resolution = resolve_module_with_imports(&db, module, item_scope(&db, module), &imports);
    assert!(resolution.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        NameresDiagnostic::UndefinedName {
            name,
            kind: UndefinedNameKind::Field,
            ..
        } if name == "collide"
    )));
    assert!(resolution.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        NameresDiagnostic::UndefinedName {
            name,
            kind: UndefinedNameKind::Field,
            ..
        } if name == "absent"
    )));

    let function = contract_function(&db, module, "C", "ambiguous");
    let body = function.body(&db).expect("body");
    let map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == body))
        .expect("body map");
    assert!(
        field_resolutions(&db, body, map)
            .into_iter()
            .any(|(name, resolution)| name == "collide" && resolution == Resolution::Err)
    );

    let parameter = contract_function(&db, module, "C", "ambiguousParameter");
    let parameter_body = parameter.body(&db).expect("body");
    let parameter_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == parameter_body))
        .expect("parameter body map");
    assert!(ident_resolutions(&db, parameter_body, parameter_map)
        .into_iter()
        .any(|(name, resolution)| name == "value" && matches!(resolution, Resolution::Param(_))));
    assert!(
        field_resolutions(&db, parameter_body, parameter_map)
            .into_iter()
            .any(|(name, resolution)| name == "collide" && resolution == Resolution::Err)
    );

    let missing = contract_function(&db, module, "C", "missing");
    let missing_body = missing.body(&db).expect("body");
    let missing_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == missing_body))
        .expect("body map");
    assert!(
        field_resolutions(&db, missing_body, missing_map)
            .into_iter()
            .any(|(name, resolution)| name == "absent" && resolution == Resolution::Err)
    );
}

#[test]
fn value_ufcs_resolves_any_value_receiver_while_preserving_qualified_calls() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\ntrait Combiner<self> {
           function combine(x: self, y: word) returns (word);
         }
         contract C {
           value: word;
           Combiner: word;
           function qualified(y: word) returns (word) {
             return Combiner.combine(value, y);
           }
           function sameNameQualifier(y: word) returns (word) {
             return Combiner.combine(Combiner, y);
           }
           function parameter(value: word, y: word) returns (word) {
             return value.combine(y);
           }
           function local(value: word, y: word) returns (word) {
             let receiver = value;
             return receiver.combine(y);
           }
           function arbitrary(y: word) returns (word) {
             return (value + y).combine(y);
           }
         }",
    );
    let resolution = resolve_module(&db, module);
    assert!(resolution.diagnostics.is_empty());

    let qualified = contract_function(&db, module, "C", "qualified");
    let qualified_body = qualified.body(&db).expect("body");
    let qualified_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == qualified_body))
        .expect("qualified body map");
    assert!(
        field_resolutions(&db, qualified_body, qualified_map)
            .into_iter()
            .any(|(name, resolution)| name == "combine"
                && matches!(resolution, Resolution::ClassMethod { .. }))
    );
    assert!(
        ident_resolutions(&db, qualified_body, qualified_map)
            .into_iter()
            .any(|(name, resolution)| name == "Combiner"
                && matches!(
                    resolution,
                    Resolution::Def {
                        kind: DefResolutionKind::Class,
                        ..
                    }
                ))
    );

    let same_name = contract_function(&db, module, "C", "sameNameQualifier");
    let same_name_body = same_name.body(&db).expect("same-name body");
    let same_name_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == same_name_body))
        .expect("same-name body map");
    let same_name_resolutions = ident_resolutions(&db, same_name_body, same_name_map)
        .into_iter()
        .filter(|(name, _)| *name == "Combiner")
        .map(|(_, resolution)| resolution)
        .collect::<Vec<_>>();
    assert_eq!(
        same_name_resolutions
            .iter()
            .filter(|resolution| matches!(
                resolution,
                Resolution::Def {
                    kind: DefResolutionKind::Class,
                    ..
                }
            ))
            .count(),
        1,
        "callee qualifier must resolve as the class: {same_name_resolutions:?}"
    );
    assert_eq!(
        same_name_resolutions
            .iter()
            .filter(|resolution| matches!(resolution, Resolution::Field(_)))
            .count(),
        1,
        "the explicit argument must still resolve as the field: {same_name_resolutions:?}"
    );

    let parameter = contract_function(&db, module, "C", "parameter");
    let parameter_body = parameter.body(&db).expect("body");
    let parameter_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == parameter_body))
        .expect("parameter body map");
    assert!(ident_resolutions(&db, parameter_body, parameter_map)
        .into_iter()
        .any(|(name, resolution)| name == "value" && matches!(resolution, Resolution::Param(_))));
    assert!(
        field_resolutions(&db, parameter_body, parameter_map)
            .into_iter()
            .any(|(name, resolution)| name == "combine"
                && matches!(resolution, Resolution::ClassMethod { .. }))
    );

    let local = contract_function(&db, module, "C", "local");
    let local_body = local.body(&db).expect("body");
    let local_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == local_body))
        .expect("local body map");
    assert!(
        ident_resolutions(&db, local_body, local_map)
            .into_iter()
            .any(|(name, resolution)| name == "receiver"
                && matches!(resolution, Resolution::Local(_)))
    );
    assert!(
        field_resolutions(&db, local_body, local_map)
            .into_iter()
            .any(|(name, resolution)| name == "combine"
                && matches!(resolution, Resolution::ClassMethod { .. }))
    );

    let arbitrary = contract_function(&db, module, "C", "arbitrary");
    let arbitrary_body = arbitrary.body(&db).expect("body");
    let arbitrary_map = resolution
        .bodies
        .iter()
        .find(|map| map.exprs.iter().any(|entry| entry.body == arbitrary_body))
        .expect("arbitrary body map");
    // As in Solidity, any value receiver works, so `xs[i].push(v)` and
    // `f().length` need no temporary.
    assert!(
        field_resolutions(&db, arbitrary_body, arbitrary_map)
            .into_iter()
            .any(|(name, resolution)| name == "combine"
                && matches!(resolution, Resolution::ClassMethod { .. }))
    );
}

#[test]
fn let_initializer_resolves_before_binder_and_then_shadows() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\nfunction f(x: word) returns (word) {
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
fn control_flow_scopes_restore_outer_bindings() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\nfunction f(x: word) returns (word) {
           {
             let x = x;
           }
           if (true) {
             let x = x;
             x = x;
           } else {
             x = x;
           }
           for (let x = x; true; x = x) {
             let x = x;
             x = x;
           }
           while (true) {
             let x = x;
             break;
           }
           return x;
         }",
    );
    assert!(diagnostic_codes(&db, module).is_empty());

    let function = top_function(&db, module, "f");
    let body = function.body(&db).expect("body");
    let map = body_map(&db, module, body);
    let events = ident_resolutions(&db, body, &map);

    let xs = events
        .iter()
        .filter(|(name, _)| *name == "x")
        .map(|(_, resolution)| resolution)
        .collect::<Vec<_>>();
    assert!(matches!(xs[0], Resolution::Param(_)));
    assert!(matches!(xs[1], Resolution::Param(_)));
    assert_eq!(xs[2], xs[3]);
    assert!(matches!(xs[2], Resolution::Local(_)));
    assert!(matches!(xs[4], Resolution::Param(_)));
    assert!(matches!(xs[5], Resolution::Param(_)));
    assert!(matches!(xs[6], Resolution::Param(_)));
    assert_eq!(xs[7], xs[8]);
    assert_eq!(xs[7], xs[9]);
    assert_ne!(xs[9], xs[10]);
    assert_eq!(xs[10], xs[11]);
    assert!(matches!(xs[12], Resolution::Param(_)));
    assert!(matches!(xs[13], Resolution::Param(_)));
}

#[test]
fn control_flow_locals_do_not_escape_or_cross_sibling_scopes() {
    for statements in [
        "if (true) { let hidden = 1; } return hidden;",
        "if (true) { let hidden = 1; } else { hidden; } return 0;",
        "for (let hidden = 1; true;) {} return hidden;",
        "for (; true;) { let hidden = 1; } return hidden;",
        "for (; true; let hidden = 1) { hidden; } return 0;",
        "for (; true; hidden = 1) { let hidden = 1; } return 0;",
        "while (true) { let hidden = 1; break; } return hidden;",
    ] {
        let db = TestDb::default();
        let source = format!("pragma solcore; function f() returns (word) {{ {statements} }}");
        let (file, module) = parse_and_module(&db, "control_flow_scope", &source);
        assert!(parse_diagnostics(&db, file).is_empty(), "{source}");
        let resolution = resolve_module(&db, module);
        assert!(
            matches!(resolution.diagnostics.as_slice(),
                [NameresDiagnostic::UndefinedName { name, .. }] if name == "hidden"),
            "{source}: {:?}",
            resolution.diagnostics
        );
    }
}

#[test]
fn control_flow_binding_visibility_uses_each_lexical_extent() {
    let source = "pragma solcore;\nfunction f() returns (word) {
        if (true) { let in_then = 1; in_then; }
        else { let in_else = 2; in_else; }
        for (let counter = 0; true; let in_post = counter) {
            let in_body = counter;
        }
        return 0;
    }";
    let db = TestDb::default();
    let module = parse_module(&db, source);
    assert!(diagnostic_codes(&db, module).is_empty());
    let body = top_function(&db, module, "f").body(&db).expect("body");
    let map = body_map(&db, module, body);
    for (name, expected) in [
        ("in_then", "{ let in_then = 1; in_then; }"),
        ("in_else", "{ let in_else = 2; in_else; }"),
        ("in_post", "let in_post = counter"),
        (
            "in_body",
            "{\n            let in_body = counter;\n        }",
        ),
        (
            "counter",
            "for (let counter = 0; true; let in_post = counter) {\n            let in_body = counter;\n        }",
        ),
    ] {
        let binding = map
            .scoped_bindings
            .iter()
            .find(|binding| binding.name == name)
            .expect("scoped binding");
        let scope = binding.scope.resolve_to_absolute(&db);
        let actual = &source[scope.start().as_usize()..scope.end().as_usize()];
        assert_eq!(actual, expected, "scope for {name}");
    }
}

#[test]
fn contract_fields_beat_top_level_functions_and_params_shadow_fields() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\nfunction balance() returns (word) { return 0; }
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
        "pragma solcore;\ncontract C {
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
fn qualified_ctor_class_method_and_dot_ctor_resolve_as_expected() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\nenum Option { None, Some(word) }
         enum Foo { Foo(word) }
         trait Show<self> { function show(x: self) returns (word); }
         function good(x: word) returns (Option) { return Option.Some(x); }
         function classCall(x: word) returns (word) { return Show.show(x); }
         function dot(x: word) returns (Option) { return .Some(x); }
         function sameName(x: word) returns (Foo) { return Foo(x); }",
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

    let dot = top_function(&db, module, "dot");
    let dot_body = dot.body(&db).expect("body");
    let dot_map = body_map(&db, module, dot_body);
    assert!(
        dot_map.exprs.iter().any(|entry| entry.body == dot_body
            && matches!(entry.resolution, Resolution::DotCtorDeferred))
    );
}

#[test]
fn self_qualified_contract_methods_do_not_shadow_same_named_local_adt_constructors() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
pragma solcore;
contract Option {
  enum Option<a> { None, Some(a) }

  function some(x: word) returns (Option<word>) {
    return Option.Some(x);
  }

  function none() returns (Option<word>) {
    return Option.None;
  }

  function read(o: Option<word>) returns (word) {
    match (o) {
    case Option.Some(x) { return x; }
    case Option.None { return 0; }
    }
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
fn definite_same_name_constructor_beats_unknown_wildcard_import() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore; enum Unit { Unit } function make() returns (Unit) { return Unit; }",
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
            .any(|(name, resolution)| *name == "Unit"
                && matches!(resolution, Resolution::Ctor { .. })),
        "same-name constructor should remain definite: {events:#?}"
    );
    assert!(resolution.diagnostics.is_empty());
}

#[path = "nameres/fixtures.rs"]
mod fixture_tests;
