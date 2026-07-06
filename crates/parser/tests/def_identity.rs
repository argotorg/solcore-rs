use hir::{
    anchor::{DefId, DefKind},
    input::SourceFile,
};
use salsa::Setter;
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

#[derive(Debug, PartialEq, Eq)]
struct DefIdentity {
    owner: Option<Box<DefIdentity>>,
    kind: DefKind,
    name: Option<String>,
    fingerprint: Option<String>,
    disambiguator: u32,
}

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.solc").parse().expect("valid url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn def_identity<'db>(db: &'db TestDb, def: DefId<'db>) -> DefIdentity {
    DefIdentity {
        owner: def.owner(db).map(|owner| Box::new(def_identity(db, owner))),
        kind: def.kind(db),
        name: def.name(db),
        fingerprint: def.fingerprint(db),
        disambiguator: def.disambiguator(db).as_u32(),
    }
}

fn all_defs<'db>(db: &'db TestDb, file: SourceFile) -> Vec<DefId<'db>> {
    parse_file_to_hir(db, file)
        .def_locations(db)
        .entries
        .iter()
        .map(|entry| entry.def_id)
        .collect()
}

fn defs_by_name<'db>(
    db: &'db TestDb,
    file: SourceFile,
    kind: DefKind,
    name: &str,
) -> Vec<DefId<'db>> {
    all_defs(db, file)
        .into_iter()
        .filter(|def| def.kind(db) == kind && def.name(db).as_deref() == Some(name))
        .collect()
}

fn defs_by_fingerprint<'db>(
    db: &'db TestDb,
    file: SourceFile,
    kind: DefKind,
    fingerprint: &str,
) -> Vec<DefId<'db>> {
    all_defs(db, file)
        .into_iter()
        .filter(|def| def.kind(db) == kind && def.fingerprint(db).as_deref() == Some(fingerprint))
        .collect()
}

fn lambda_body_identities(db: &TestDb, file: SourceFile) -> Vec<(String, DefIdentity)> {
    let mut bodies = all_defs(db, file)
        .into_iter()
        .filter(|def| {
            def.kind(db) == DefKind::FuncBody && def.name(db).as_deref() == Some("lambda")
        })
        .map(|def| {
            (
                def.fingerprint(db).expect("lambda body fingerprint"),
                def_identity(db, def),
            )
        })
        .collect::<Vec<_>>();
    bodies.sort_by(|a, b| a.0.cmp(&b.0));
    bodies
}

#[test]
fn same_named_contract_methods_have_container_relative_def_ids() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "contract-methods",
        "contract A {\n  function f() {}\n}\n\ncontract B {\n  function f() {}\n}\n",
    );

    let methods = defs_by_name(&db, file, DefKind::Function, "f");
    assert_eq!(methods.len(), 2);
    assert_ne!(methods[0], methods[1]);
    assert_ne!(methods[0].owner(&db), methods[1].owner(&db));
}

#[test]
fn instances_of_same_class_on_different_heads_have_distinct_def_ids() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "instance-heads",
        "class self:StorageType {}\n\n\
         instance word:StorageType {\n  function rep(x:word) -> word { return x; }\n}\n\n\
         instance uint:StorageType {\n  function rep(x:uint) -> uint { return x; }\n}\n",
    );

    let instances = defs_by_name(&db, file, DefKind::Instance, "StorageType");
    assert_eq!(instances.len(), 2);
    assert_ne!(instances[0], instances[1]);

    let fingerprints = instances
        .iter()
        .map(|def| def.fingerprint(&db))
        .collect::<Vec<_>>();
    assert!(fingerprints.contains(&Some("pred[1]|4:word".to_owned())));
    assert!(fingerprints.contains(&Some("pred[1]|4:uint".to_owned())));
}

#[test]
fn instances_with_same_subject_and_different_class_args_have_distinct_def_ids() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "instance-class-args",
        "class self:Carrier(arg) {}\n\n\
         instance word:Carrier(uint) {}\n\n\
         instance word:Carrier(bool) {}\n",
    );

    let instances = defs_by_name(&db, file, DefKind::Instance, "Carrier");
    assert_eq!(instances.len(), 2);
    assert_ne!(instances[0], instances[1]);

    let fingerprints = instances
        .iter()
        .map(|def| def.fingerprint(&db))
        .collect::<Vec<_>>();
    assert!(fingerprints.contains(&Some("pred[2]|4:word|4:uint".to_owned())));
    assert!(fingerprints.contains(&Some("pred[2]|4:word|4:bool".to_owned())));
}

#[test]
fn imports_have_structural_def_ids() {
    let db = TestDb::default();
    let file = source_file(&db, "imports-distinct", "import A;\nimport B;\n");

    let import_a = defs_by_fingerprint(&db, file, DefKind::Import, "A");
    let import_b = defs_by_fingerprint(&db, file, DefKind::Import, "B");
    assert_eq!(import_a.len(), 1);
    assert_eq!(import_b.len(), 1);
    assert_ne!(import_a[0], import_b[0]);
}

#[test]
fn inserting_import_above_keeps_existing_import_identities_stable() {
    let mut db = TestDb::default();
    let file = source_file(&db, "imports-stable", "import A;\nimport B;\n");

    let before_a = {
        let imports = defs_by_fingerprint(&db, file, DefKind::Import, "A");
        assert_eq!(imports.len(), 1);
        def_identity(&db, imports[0])
    };
    let before_b = {
        let imports = defs_by_fingerprint(&db, file, DefKind::Import, "B");
        assert_eq!(imports.len(), 1);
        def_identity(&db, imports[0])
    };

    file.set_content(&mut db)
        .to(Some("import C;\nimport A;\nimport B;\n".to_owned()));

    let after_a = {
        let imports = defs_by_fingerprint(&db, file, DefKind::Import, "A");
        assert_eq!(imports.len(), 1);
        def_identity(&db, imports[0])
    };
    let after_b = {
        let imports = defs_by_fingerprint(&db, file, DefKind::Import, "B");
        assert_eq!(imports.len(), 1);
        def_identity(&db, imports[0])
    };

    assert_eq!(after_a, before_a);
    assert_eq!(after_b, before_b);
}

#[test]
fn import_selector_fingerprints_are_structural_and_order_independent() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "imports-selector-fingerprints",
        "import A.{x as y, (^^)} hiding {z, w};\n\
         import A.{(^^), x as y} hiding {w, z};\n\
         import A.{x};\n\
         import A.{x as y};\n\
         import A.{*};\n",
    );

    let mut fingerprints = all_defs(&db, file)
        .into_iter()
        .filter(|def| def.kind(&db) == DefKind::Import)
        .map(|def| def.fingerprint(&db).expect("import fingerprint"))
        .collect::<Vec<_>>();

    assert_eq!(fingerprints.len(), 5);
    fingerprints.sort();
    assert_eq!(
        fingerprints
            .windows(2)
            .filter(|pair| pair[0] == pair[1])
            .count(),
        1
    );
    fingerprints.dedup();
    assert_eq!(fingerprints.len(), 4);
}

#[test]
fn import_constructor_selector_fingerprints_are_structural() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "imports-constructor-selector-fingerprints",
        "import A.{T};\n\
         import A.{T(*)};\n\
         import A.{T(A, B)};\n",
    );

    let mut fingerprints = all_defs(&db, file)
        .into_iter()
        .filter(|def| def.kind(&db) == DefKind::Import)
        .map(|def| def.fingerprint(&db).expect("import fingerprint"))
        .collect::<Vec<_>>();

    assert_eq!(fingerprints.len(), 3);
    fingerprints.sort();
    fingerprints.dedup();
    assert_eq!(fingerprints.len(), 3);
}

#[test]
fn inserting_preceding_lambda_keeps_existing_lambda_body_identities_stable() {
    let mut db = TestDb::default();
    let before_src = "function f(z: word) -> word {
        let n = lam (x: word) { return x; };
        let m = lam (y: word) { return y; };
        return m(n(z));
    }";
    let file = source_file(&db, "lambda-bodies-stable", before_src);

    let before = lambda_body_identities(&db, file);
    assert_eq!(before.len(), 2);

    file.set_content(&mut db).to(Some(
        "function f(z: word) -> word {
            let ignored = lam (q: word) { return q + 1; };
            let n = lam (x: word) { return x; };
            let m = lam (y: word) { return y; };
            return m(n(z));
        }"
        .to_owned(),
    ));

    let after = lambda_body_identities(&db, file);
    assert_eq!(after.len(), 3);

    for (fingerprint, identity) in before {
        let after_identity = after
            .iter()
            .find_map(|(after_fingerprint, after_identity)| {
                (after_fingerprint == &fingerprint).then_some(after_identity)
            })
            .expect("original lambda fingerprint after insertion");
        assert_eq!(after_identity, &identity);
    }
}

#[test]
fn lambda_body_edit_keeps_lambda_body_identity_stable() {
    let mut db = TestDb::default();
    let before_src = "function f(z: word) -> word {
        let n = lam (x: word) { return x + 1; };
        return n(z);
    }";
    let file = source_file(&db, "lambda-body-edit-stable", before_src);

    let before = lambda_body_identities(&db, file);
    assert_eq!(before.len(), 1);

    file.set_content(&mut db).to(Some(
        "function f(z: word) -> word {
            let n = lam (x: word) { return x + 2; };
            return n(z);
        }"
        .to_owned(),
    ));

    let after = lambda_body_identities(&db, file);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].1, before[0].1);
}

#[test]
fn inserting_unrelated_item_above_def_keeps_identity_stable() {
    let mut db = TestDb::default();
    let file = source_file(&db, "stable-def", "\nfunction target() {}\n");

    let before = {
        let targets = defs_by_name(&db, file, DefKind::Function, "target");
        assert_eq!(targets.len(), 1);
        def_identity(&db, targets[0])
    };

    file.set_content(&mut db).to(Some(
        "\nfunction helper() {}\n\nfunction target() {}\n".to_owned(),
    ));

    let after = {
        let targets = defs_by_name(&db, file, DefKind::Function, "target");
        assert_eq!(targets.len(), 1);
        def_identity(&db, targets[0])
    };

    assert_eq!(after, before);
}

#[test]
fn leading_whitespace_does_not_change_def_identity() {
    let mut db = TestDb::default();
    let file = source_file(&db, "leading-whitespace", "\nfunction target() {}\n");

    let before = {
        let targets = defs_by_name(&db, file, DefKind::Function, "target");
        assert_eq!(targets.len(), 1);
        def_identity(&db, targets[0])
    };

    file.set_content(&mut db)
        .to(Some("\n\n\nfunction target() {}\n".to_owned()));

    let after = {
        let targets = defs_by_name(&db, file, DefKind::Function, "target");
        assert_eq!(targets.len(), 1);
        def_identity(&db, targets[0])
    };

    assert_eq!(after, before);
}

#[test]
fn well_formed_program_defs_have_zero_disambiguators() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "zero-disambiguators",
        "class self:StorageType {}\n\n\
         instance word:StorageType {\n  function rep(x:word) -> word { return x; }\n}\n\n\
         contract Counter {\n  function main() -> word { return 0; }\n}\n\n\
         function top() {}\n",
    );

    let non_zero = all_defs(&db, file)
        .into_iter()
        .map(|def| def_identity(&db, def))
        .filter(|identity| identity.disambiguator != 0)
        .collect::<Vec<_>>();

    assert_eq!(non_zero, Vec::<DefIdentity>::new());
}
