use super::*;

// Keep both word and uint256 instances so arithmetic alone cannot select one.
const NUMERIC_STD: &str = r#"
pragma solcore;
export { uint256(*), Sub, Ord };
enum uint256 { uint256(word) }
impl Int<uint256> {
    function fromInteger(x: integer) returns (uint256) { return uint256(0); }
}
trait Sub<a> { function sub(x: a, y: a) returns (a); }
impl Sub<word> { function sub(x: word, y: word) returns (word) { return x; } }
impl Sub<uint256> { function sub(x: uint256, y: uint256) returns (uint256) { return x; } }
trait Ord<a> { function gt(x: a, y: a) returns (bool); }
impl Ord<word> { function gt(x: word, y: word) returns (bool) { return true; } }
impl Ord<uint256> { function gt(x: uint256, y: uint256) returns (bool) { return true; } }
"#;

fn numeric_db(source: &str) -> (TestDb, ModuleKey) {
    let (mut db, main) = db_with_main_typeck(source);
    let file = source_file_at_path(&db, &PathBuf::from("/std/std.sol"), NUMERIC_STD);
    db.insert_module_file(
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        file,
    );
    (db, main)
}

#[test]
fn arithmetic_literals_default_to_canonical_uint256() {
    for extra in ["", "enum uint256 { Impostor(word) }"] {
        let source = format!(
            r#"
pragma solcore;
import * from std;
{extra}
function f() returns (bool) {{
    let x = 0;
    let y = x - 1;
    return y > x;
}}
"#
        );
        let (db, key) = numeric_db(&source);
        let module = module_id_from_key(&db, &key);
        let (_, result) = infer_module_function_with_solver(&db, module, "f");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let expected = canonical_std_adt_ty(&db, "uint256", Vec::new());
        assert_eq!(result.let_tys.len(), 2);
        assert!(result.let_tys.iter().all(|binding| binding.ty == expected));
        assert!(
            result
                .obligations
                .iter()
                .enumerate()
                .filter(|(_, obligation)| {
                    matches!(obligation.source, ObligationSource::IntegerLiteral { .. })
                })
                .all(|(index, _)| result
                    .obligation_evidence
                    .iter()
                    .any(|proof| proof.obligation == index))
        );
    }
}

#[test]
fn explicit_word_and_generic_numeric_contexts_are_preserved() {
    let (db, key) = numeric_db(
        r#"
pragma solcore;
import * from std;
function f() returns (bool) {
    let x: word = 0;
    let y = x - 1;
    return y > x;
}
function generic<a>(x: a) returns (a) where a: Int, a: Sub { return x - 1; }
"#,
    );
    let module = module_id_from_key(&db, &key);
    let (_, result) = infer_module_function_with_solver(&db, module, "f");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(
        result
            .let_tys
            .iter()
            .all(|binding| binding.ty == Ty::word(&db))
    );
    let (_, result) = infer_module_function_with_solver(&db, module, "generic");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.root_scheme.binder_count(&db), 1);
}

fn infer_root_literal<'db>(db: &'db TestDb, module: Module<'db>) -> InferenceResult<'db> {
    let info = function_info_named(db, module, "f");
    let body = info.function.body(db).unwrap();
    let resolution = hir_nameres::resolve_module(db, module);
    let ctx = BodyTyContext::new(
        module,
        body_map(db, &resolution, body),
        Vec::new(),
        Vec::new(),
        None,
    );
    infer_body(db, body, ctx)
}

#[test]
fn unconstrained_root_literal_uses_checked_type_without_name_lookup() {
    let (db, key) = numeric_db("pragma solcore; function f() returns (word) { return 1; }");
    let module = module_hir(&db, module_id_from_key(&db, &key)).unwrap();
    let result = infer_root_literal(&db, module);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let TyKind::Function { ret, .. } = result.root_scheme.body(&db).ty(&db).kind(&db) else {
        panic!("function type");
    };
    assert_eq!(*ret, canonical_std_adt_ty(&db, "uint256", Vec::new()));
}

#[test]
fn missing_std_does_not_default_to_word_or_a_same_named_user_type() {
    for declaration in ["", "enum uint256 { uint256(word) }"] {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            &format!("pragma solcore; {declaration} function f() returns (word) {{ return 1; }}"),
        );
        let result = infer_root_literal(&db, module);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic,
                    TypeckDiagnostic::UndefinedTypeConstructor { name, .. } if name == "std.uint256"
                )),
            "{:?}",
            result.diagnostics
        );
        // An actual explicit word context still works without the standard library.
        let (_, explicit) = infer_function(&db, module, "f");
        assert!(
            explicit.diagnostics.is_empty(),
            "{:?}",
            explicit.diagnostics
        );
    }
}

#[test]
fn invalid_numeric_patterns_without_std_do_not_trigger_defaulting_errors() {
    for literal in ["1", "0x1"] {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            &format!(
                r#"
pragma solcore;
function f(x: bool) returns (bool) {{
    match (x) {{
        case {literal} {{ return true; }}
        case _ {{ return false; }}
    }}
}}
"#
            ),
        );
        let (_, direct) = infer_function(&db, module, "f");
        let mut solved = infer_all_functions_with_solver(&db, module);
        let (_, solved) = solved.pop().expect("one function");
        for result in [direct, solved] {
            assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
            assert!(matches!(&result.diagnostics[0],
                TypeckDiagnostic::Mismatch { expected, actual, .. }
                    if expected == "bool" && actual == "numeric"
            ));
        }
    }
}

#[test]
fn uint256_numeric_patterns_share_constructor_coverage() {
    let (db, key) = numeric_db(
        r#"
pragma solcore;
import * from std;
function f(x: uint256) returns (bool) {
    match (x) {
        case 0 { return true; }
        case uint256(0) { return false; }
        case _ { return false; }
    }
}
"#,
    );
    let (_, result) = infer_module_function_with_solver(&db, module_id_from_key(&db, &key), "f");
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    assert!(matches!(
        result.diagnostics[0],
        TypeckDiagnostic::UnreachableMatchArm { .. }
    ));
}

#[test]
fn implicit_literal_pattern_uses_checked_default() {
    let (db, key) = numeric_db(
        r#"
pragma solcore;
import * from std;
function f() returns (bool) {
    let x = 0;
    match (x) {
        case 0 { return true; }
        case _ { return false; }
    }
}
"#,
    );
    let (_, result) = infer_module_function_with_solver(&db, module_id_from_key(&db, &key), "f");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.let_tys[0].ty,
        canonical_std_adt_ty(&db, "uint256", Vec::new())
    );
}

#[test]
fn conditional_literal_branches_take_the_other_branch_int_type() {
    for body in [
        "return c ? a : 0;",
        "return c ? 0x1 : a;",
        "return c ? 1 : 2;",
        "let x = c ? a : 0; return x;",
    ] {
        let source = format!(
            "pragma solcore; import * from std; \
             function f(c: bool, a: uint256) returns (uint256) {{ {body} }}"
        );
        let (db, key) = numeric_db(&source);
        let module = module_id_from_key(&db, &key);
        // Without a trait environment `Int` cannot be decided, so the
        // conditional must not be rejected either.
        let (_, unsolved) = infer_module_function(&db, module, "f");
        assert!(
            unsolved.diagnostics.is_empty(),
            "{body}: {:?}",
            unsolved.diagnostics
        );
        let (_, result) = infer_module_function_with_solver(&db, module, "f");
        assert!(
            result.diagnostics.is_empty(),
            "{body}: {:?}",
            result.diagnostics
        );
        let uint256_int = Pred::in_class(
            &db,
            ClassId::Builtin(BuiltinClassId::Int),
            canonical_std_adt_ty(&db, "uint256", Vec::new()),
            Vec::new(),
        );
        let literals = result
            .obligations
            .iter()
            .enumerate()
            .filter(|(_, obligation)| {
                matches!(obligation.source, ObligationSource::IntegerLiteral { .. })
            })
            .collect::<Vec<_>>();
        assert!(!literals.is_empty(), "{body}");
        for (index, obligation) in literals {
            assert_eq!(obligation.pred, uint256_int, "{body}");
            assert!(
                result
                    .obligation_evidence
                    .iter()
                    .any(|proof| proof.obligation == index),
                "{body}"
            );
        }
    }
}

#[test]
fn conditional_literal_branch_needs_an_int_instance_not_a_builtin_type() {
    let (db, key) = numeric_db(
        r#"
pragma solcore;
import * from std;
enum Meters { Meters(word) }
impl Int<Meters> {
    function fromInteger(x: integer) returns (Meters) { return Meters(0); }
}
enum Feet { Feet(word) }
function numeric(c: bool, m: Meters) returns (Meters) { return c ? m : 3; }
function nonNumeric(c: bool, f: Feet) returns (Feet) {
    let x = c ? 3 : f;
    return x;
}
"#,
    );
    let module = module_id_from_key(&db, &key);
    let (_, numeric) = infer_module_function_with_solver(&db, module, "numeric");
    assert!(numeric.diagnostics.is_empty(), "{:?}", numeric.diagnostics);
    let (_, non_numeric) = infer_module_function_with_solver(&db, module, "nonNumeric");
    assert_eq!(
        non_numeric.diagnostics.len(),
        1,
        "{:?}",
        non_numeric.diagnostics
    );
    assert!(
        matches!(&non_numeric.diagnostics[0],
            TypeckDiagnostic::Mismatch { expected, actual, .. }
                if expected == "numeric" && actual == "Feet"
        ),
        "{:?}",
        non_numeric.diagnostics
    );
}

#[test]
fn uint256_numeric_patterns_reject_values_outside_the_range() {
    for literal in [
        "115792089237316195423570985008687907853269984665640564039457584007913129639936",
        "0x10000000000000000000000000000000000000000000000000000000000000000",
    ] {
        for context in ["x: uint256", "x"] {
            let source = format!(
                r#"
pragma solcore;
import * from std;
function f({context}) returns (bool) {{
    match (x) {{
        case {literal} {{ return true; }}
        case _ {{ return false; }}
    }}
}}
"#
            );
            let (db, key) = numeric_db(&source);
            let (_, result) =
                infer_module_function_with_solver(&db, module_id_from_key(&db, &key), "f");
            assert!(result.diagnostics.iter().any(|diagnostic| matches!(diagnostic,
                TypeckDiagnostic::Mismatch { expected, .. } if expected.contains("uint256 range")
            )), "{context}: {literal}: {:?}", result.diagnostics);
        }
    }
}

// `Read` stands in for the std `RValueIdxAccess` instances: the value type of
// a read is known only once the index type is, and the index accepts any
// `Typedef<word>` type, so a literal index stays open until it is defaulted.
const INDEXED_STD: &str = r#"
pragma solcore;
export { uint256(*), Bag(*), Typedef, Read };
enum uint256 { uint256(word) }
impl Int<uint256> {
    function fromInteger(x: integer) returns (uint256) { return uint256(0); }
}
trait Typedef<abs, rep> { function rep(x: abs) returns (rep); }
impl Typedef<word, word> { function rep(x: word) returns (word) { return x; } }
impl Typedef<uint256, word> { function rep(x: uint256) returns (word) { return 0; } }
enum Bag { Bag(word) }
trait Read<col_idx, val> { function read(xi: col_idx) returns (val); }
impl<i> Read<(Bag, i), uint256> where i: Typedef<word> {
    function read(xi: (Bag, i)) returns (uint256) { return uint256(0); }
}
"#;

#[test]
fn defaulted_literal_index_solves_obligations_created_before_it() {
    // `Typedef.rep(...)` is instantiated before its argument, so its
    // obligation can only be solved after the read below it is: that read is
    // solvable once its literal index defaults to uint256.
    for body in [
        "return Typedef.rep(Read.read((b, 0)));",
        "let i = 0; return Typedef.rep(Read.read((b, i)));",
    ] {
        let source = format!(
            "pragma solcore;\nimport * from std;\nfunction f(b: Bag) returns (word) {{ {body} }}\n"
        );
        let (mut db, main) = db_with_main_typeck(&source);
        let file = source_file_at_path(&db, &PathBuf::from("/std/std.sol"), INDEXED_STD);
        db.insert_module_file(
            ModuleKey {
                library: LibraryId::Std,
                logical_path: vec!["std".to_owned()],
            },
            file,
        );
        let (_, result) =
            infer_module_function_with_solver(&db, module_id_from_key(&db, &main), "f");
        assert!(
            result.diagnostics.is_empty(),
            "{body}: {:?}",
            result.diagnostics
        );
        let uint256 = canonical_std_adt_ty(&db, "uint256", Vec::new());
        let typedef = result
            .obligations
            .iter()
            .position(|obligation| {
                matches!(
                    obligation.pred.kind(&db),
                    PredKind::InClass {
                        class: ClassId::User(class),
                        main,
                        args,
                    } if class.name(&db).as_deref() == Some("Typedef")
                        && *main == uint256
                        && args.as_slice() == [Ty::word(&db)]
                )
            })
            .unwrap_or_else(|| panic!("{body}: Typedef obligation: {:?}", result.obligations));
        assert!(
            (0..result.obligations.len()).all(|index| result
                .obligation_evidence
                .iter()
                .any(|proof| proof.obligation == index)),
            "{body}: typedef obligation {typedef}: {:?}",
            result.obligation_evidence
        );
    }
}
