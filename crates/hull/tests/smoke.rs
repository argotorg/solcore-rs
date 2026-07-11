use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{anchor::DefLocationTable, ast::item::Module, input::SourceFile};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key, module_key_for_path, module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter;
use solcore_hull::{
    CheckDiagnosticKind, EmitDiagnostic, EmitDiagnosticKind, EmitOptions, check_program_with_db,
    emit_module, pretty_program,
};
use specialize::{SpecializeOptions, SpecializeOutput, specialize_module};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

impl TestDb {
    fn insert_module_file(&mut self, key: ModuleKey, file: SourceFile) {
        if self.module_files.insert(key, file) == Some(file) {
            return;
        }
        let files = self
            .module_files
            .iter()
            .map(|(key, file)| (key.clone(), *file))
            .collect();
        if let Some(snapshot) = self.module_file_snapshot {
            snapshot.set_files(self).to(files);
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

#[salsa::db]
impl nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.unwrap_or_else(|| {
            ModuleTree::new(
                self,
                PathBuf::from("/main"),
                PathBuf::from("/std"),
                BTreeMap::new(),
            )
        })
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .unwrap_or_else(|| ModuleFsSnapshot::new(self, BTreeSet::new(), BTreeMap::new()))
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .unwrap_or_else(|| ModuleFileSnapshot::new(self, BTreeMap::new()))
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl hir_ty::Db for TestDb {}

#[test]
fn specialization_corpus_subset_emits_and_checks() {
    let cases = [
        (
            "spec/01id",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/01id.solc"),
        ),
        (
            "spec/00answer",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/00answer.solc"),
        ),
        (
            "spec/022add",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/022add.solc"),
        ),
        (
            "spec/024arith",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/024arith.solc"),
        ),
        (
            "spec/031maybe",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/031maybe.solc"),
        ),
        (
            "spec/047rgb",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/047rgb.solc"),
        ),
    ];
    let mut failures = Vec::new();
    for (name, src) in cases {
        let (db, output) = specialize_src(name, src);
        if !output.diagnostics.is_empty() {
            failures.push(format!(
                "{name}: specialize: {}",
                output
                    .diagnostics
                    .iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let emitted = emit_module(
            db,
            &output.module,
            EmitOptions {
                emit_dispatcher_comments: false,
            },
        );
        let non_dispatch: Vec<_> = emitted
            .diagnostics
            .iter()
            .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
            .collect();
        if !non_dispatch.is_empty() {
            failures.push(format!(
                "{name}: emit: {}",
                non_dispatch
                    .into_iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let checked = check_program_with_db(db, &emitted.program);
        if !checked.is_empty() {
            failures.push(format!(
                "{name}: check: {}",
                checked
                    .iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn contract_without_runtime_main_defers_dispatch_to_specialization() {
    let (db, output) = specialize_src(
        "dispatch_word",
        r#"
contract C {
  function main() -> () {}
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let mut module = output.module;
    for item in &mut module.items {
        if let specialize::MonoItem::Contract(contract) = item {
            contract
                .entries
                .retain(|entry| !matches!(entry, specialize::MonoEntry::RuntimeMain { .. }));
        }
    }
    let emitted = emit_module(db, &module, EmitOptions::default());
    assert!(
        emitted.diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            EmitDiagnosticKind::DispatcherDeferred { contract } if contract == "C"
        )),
        "{:?}",
        emitted.diagnostics
    );
    let hull = pretty_program(db, &emitted.program);
    assert!(!hull.contains("calldataload(0)"), "{hull}");
    assert!(!hull.contains("dispatch_selector"), "{hull}");
}

#[test]
fn compiler_owned_std_dispatch_needs_no_source_import() {
    let (db, output) = specialize_src_with_std(
        "implicit_std_dispatch",
        r#"
contract C {
  public function echo(x : word) -> word { return x; }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("dispatch_selector_matches"), "{hull}");
    assert!(hull.contains("opcodes_calldataload"), "{hull}");
}

#[test]
fn dispatch_basic_fixture_uses_std_dispatch_main() {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/basic.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("basic_C_main_"), "{hull}");
    assert!(hull.contains("dispatch_selector_matches"), "{hull}");
    assert!(hull.contains("std_abi_decode_debc005b9$calldataLbytesJ_CalldataWordReader_memoryLstringJ_memoryLstringJ"), "{hull}");
    assert!(hull.contains("opcodes_mcopy"), "{hull}");
    assert!(!hull.contains("dispatch_ret12_abi_head0_offset"), "{hull}");
}

#[test]
fn deployment_objects_copy_runtime_and_guard_constructor_value() {
    let repo = repo_root();
    let fixture = repo.join(
        "crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/empty_no_constructor.solc",
    );
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("object \"CDeploy\""), "{hull}");
    assert!(hull.contains("object \"C\""), "{hull}");
    assert!(
        hull.contains("codecopy(0, dataoffset(\"C\"), datasize(\"C\"))"),
        "{hull}"
    );

    let fixture = repo
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/nonpayable_ctor.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    let outer = hull
        .split("object \"NonPayableCtor\" {")
        .next()
        .expect("outer object");
    assert!(outer.contains("object \"NonPayableCtorDeploy\""), "{hull}");
    assert!(outer.contains("mstore(64, memoryguard(128))"), "{hull}");
    assert!(
        outer.contains("datasize(\"NonPayableCtorDeploy\")"),
        "{hull}"
    );
    assert!(outer.contains("if callvalue()"), "{hull}");
    assert!(outer.contains("0xb5988ea3"), "{hull}");
    assert!(outer.matches("_start").count() >= 2, "{hull}");
    assert!(
        outer.contains("codecopy(0, dataoffset(\"NonPayableCtor\"), datasize(\"NonPayableCtor\"))"),
        "{hull}"
    );
    assert!(outer.contains("return(0, size)"), "{hull}");
    let runtime = hull
        .split("object \"NonPayableCtor\" {")
        .nth(1)
        .expect("runtime object");
    assert!(!runtime.contains("_start"), "{hull}");

    let fixture = repo
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/payable_ctor.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    let hull = pretty_program(db, &emitted.program);
    let outer = hull
        .split("object \"PayableCtor\" {")
        .next()
        .expect("outer object");
    assert!(!outer.contains("0xb5988ea3"), "{hull}");
}

#[test]
fn importless_nullary_constructor_uses_overlay_deployment_entry() {
    let (db, output) = specialize_src(
        "nullary_ctor_overlay",
        r#"
contract C {
  constructor() {}

  function main() -> () {
    return ();
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    let outer = hull.split("object \"C\" {").next().expect("outer object");
    assert!(outer.contains("_start"), "{hull}");
    assert!(outer.contains("init_"), "{hull}");
    assert!(
        outer.contains("codecopy(0, dataoffset(\"C\"), datasize(\"C\"))"),
        "{hull}"
    );
    assert!(!outer.contains("constructor_arg"), "{hull}");
}

#[test]
fn std_constructor_overlay_decodes_appended_arguments_in_deployment_closure() {
    let (db, output) = specialize_src_with_std(
        "std_ctor_overlay_args",
        r#"
import std.{*};
import std.dispatch.{*};

data Config = Config(word, bool);

contract C {
  constructor(config : Config, label : memory(string)) {
    let saved_config = config;
    let saved_label = label;
  }

  public function echo(config : Config) -> Config { return config; }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    let outer = hull.split("object \"C\" {").next().expect("outer object");
    assert!(outer.contains("copy_arguments_for_constructor"), "{hull}");
    assert!(outer.contains("abi_decode"), "{hull}");
    assert!(outer.contains("BoundedMemoryWordReader"), "{hull}");
    for function in [
        "WordReader_advance$BoundedMemoryWordReader",
        "WordReader_read$BoundedMemoryWordReader",
        "WordReader_copyToMem$BoundedMemoryWordReader",
    ] {
        let body = hull_function(&hull, function);
        assert!(body.contains("0x08638556"), "{body}");
    }
    assert!(outer.contains("Generic_to$Config"), "{hull}");
    assert!(
        outer.contains("argSize := sub(codesize(), programSize)"),
        "{hull}"
    );
    assert!(outer.contains("minimumSize"), "{hull}");
    assert!(outer.contains("if lt(argSize, "), "{hull}");
    assert!(outer.contains("mstore(0, 0x08638556)"), "{hull}");
    assert!(
        outer.contains("codecopy(memoryDataOffset, programSize, argSize)"),
        "{hull}"
    );
    assert!(
        !outer.contains("codecopy(0, datasize(\"CDeploy\"), 32)"),
        "{hull}"
    );
    assert!(!outer.contains("constructor_arg"), "{hull}");
    assert!(outer.matches("_start").count() >= 2, "{hull}");

    let runtime = hull.split("object \"C\" {").nth(1).expect("runtime object");
    assert!(
        !runtime.contains("copy_arguments_for_constructor"),
        "{hull}"
    );
    assert!(!runtime.contains("_start"), "{hull}");
    assert!(runtime.contains("Generic_to$Config"), "{hull}");
    assert!(runtime.contains("Generic_from$Config"), "{hull}");
}

#[test]
fn std_dispatch_bool_uses_std_abi_decode_and_encode() {
    let (db, output) = specialize_src_with_std(
        "std_bool_dispatch",
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  constructor(initial : bool) { let saved = initial; }
  public function echo(x : bool) -> bool { return x; }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(
        hull.contains("ABIDecode_decode$ABIDecoderLbool_CalldataWordReaderJ"),
        "{hull}"
    );
    assert!(
        hull.contains("ABIDecode_decode$ABIDecoderLbool_BoundedMemoryWordReaderJ"),
        "{hull}"
    );
    assert!(hull.contains("ABIEncode_encodeInto$bool"), "{hull}");
    for reader in ["CalldataWordReader", "BoundedMemoryWordReader"] {
        let decoder = hull_function(
            &hull,
            &format!("ABIDecode_decode$ABIDecoderLbool_{reader}J"),
        );
        // Both calldata dispatch and constructor-memory decoding must retain
        // the canonical-bool rejection path from the shared std instance.
        assert!(decoder.contains("0x0557dbbf"), "{decoder}");
    }
}

#[test]
fn std_dispatch_product_adt_uses_generic_abi_decode_and_encode() {
    let (db, output) = specialize_src_with_std(
        "std_product_adt_dispatch",
        r#"
import std.{*};
import std.dispatch.{*};

data Point = Point(word, bool);

contract C {
  public function roundtrip(p : Point) -> Point { return p; }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("Generic_to$Point"), "{hull}");
    assert!(hull.contains("Generic_from$Point"), "{hull}");
    assert!(
        hull.contains("ABIDecode_decode$ABIDecoderLPoint_CalldataWordReaderJ"),
        "{hull}"
    );
    assert!(hull.contains("ABIEncode_encodeInto$Point"), "{hull}");
}

#[test]
fn dynamic_product_adt_uses_abi_tuple_boundaries_at_nonzero_offsets() {
    let (db, output) = specialize_src_with_std(
        "std_dynamic_product_adt_dispatch",
        r#"
import std.{*};
import std.dispatch.{*};

data Payload = Payload(memory(string), bool);

contract C {
  constructor(prefix : word, payload : Payload) {}

  public function roundtrip(prefix : word, payload : Payload) -> (word, Payload) {
    return (prefix, payload);
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert_eq!(
        check_program_with_db(db, &emitted.program),
        Vec::new(),
        "{hull}"
    );
    assert!(hull.contains("Generic_to$Payload"), "{hull}");
    assert!(hull.contains("Generic_from$Payload"), "{hull}");
    assert!(
        hull.contains("ABIDecode_decode$ABIDecoderLABITuple"),
        "{hull}"
    );
    let tuple_decoder = hull_function(&hull, "ABIDecode_decode$ABIDecoderLABITuple");
    assert!(
        tuple_decoder.contains("Add_add$word(currentHeadOffset, 32)"),
        "{tuple_decoder}"
    );
    assert!(
        !tuple_decoder.contains("ABIAttribs_headSize"),
        "a dynamic ADT tail is bounded by its outer offset slot, not its wider product head:\n{tuple_decoder}"
    );
    assert!(hull.contains("ABIEncode_encodeInto$ABITuple"), "{hull}");
    assert!(
        hull.contains("Add_add$word(basePtr, offset), Sub_sub$word(tail, basePtr)"),
        "{hull}"
    );
}

#[test]
fn std_dispatch_address_decode_rejects_dirty_high_bits() {
    let (db, output) = specialize_src_with_std(
        "std_address_dispatch",
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  public function id_address(a : address) -> address { return a; }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(
        hull.contains("ABIDecode_decode$ABIDecoderLaddress_CalldataWordReaderJ"),
        "{hull}"
    );
    assert!(hull.contains("(160, raw)"), "{hull}");
    assert!(hull.contains("0x7cc04fa7"), "{hull}");
}

#[test]
fn std_dispatch_explicit_fallback_stops_after_execution() {
    let (db, output) = specialize_src_with_std(
        "std_fallback_dispatch",
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  public function answer() -> uint256 { return uint256(42); }
  fallback() -> () {}
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("stop()"), "{hull}");
}

#[test]
fn for_loop_emits_hull_for_and_loop_control() {
    let repo = repo_root();
    let fixture =
        repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/for-break.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert!(
        !emitted.diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                EmitDiagnosticKind::UnsupportedMonoConstruct { construct }
                    if construct == "for loop" || construct == "loop control"
            )
        }),
        "{:?}",
        emitted.diagnostics
    );
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("for ("), "{hull}");
    assert!(hull.contains("break"), "{hull}");
    let checked = check_program_with_db(db, &emitted.program);
    assert!(
        !checked.iter().any(|diagnostic| {
            matches!(diagnostic.kind, CheckDiagnosticKind::ExpectedBool { .. })
        }),
        "{checked:?}"
    );
}

#[test]
fn word_storage_fixture_reaches_word_slot_ops() {
    let repo = repo_root();
    let fixture =
        repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec/120basicCounter.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("sload") || hull.contains("sstore"), "{hull}");
    assert!(
        !emitted.diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                EmitDiagnosticKind::UnsupportedMonoConstruct { construct }
                    if construct == "field access" || construct == "index access"
            )
        }),
        "{:?}",
        emitted.diagnostics
    );
}

#[test]
fn single_constructor_matches_project_payloads_from_scrutinee() {
    assert_fixture_emits_and_checks("cases/encoder1.solc");
    assert_fixture_has_no_unbound_alt("cases/mptc-multi-instance.solc");
}

#[test]
fn decision_tree_match_lowering_preserves_priority_nested_and_multi_scrutinee_cases() {
    for fixture in [
        "spec/033join.solc",
        "spec/038food0.solc",
        "cases/Option.solc",
        "cases/option2.solc",
        "cases/dot-pattern-nested-constructor.solc",
        "cases/Logic.solc",
        "cases/Ackermann.solc",
        "cases/false-redundant-warning.solc",
        "cases/super-class.solc",
    ] {
        assert_fixture_emits_without_match_lowering_regressions(fixture);
    }
}

#[test]
fn decision_tree_shape_preserves_specific_constructors_before_wildcard_defaults() {
    let dwarves = pretty_src_hull(
        "dwarves_runtime_shape",
        r#"
contract Dwarves {
  data Dwarf = Doc | Grumpy | Sleepy | Bashful | Happy | Sneezy | Dopey;

  public function fromEnum(c : Dwarf) -> word {
    assembly { mstore(0, 0) }
    match c {
      | Dwarf.Doc => return 1;
      | Dwarf.Grumpy => return 2;
      | Dwarf.Sleepy => return 3;
      | Dwarf.Bashful => return 4;
      | Dwarf.Happy => return 5;
      | _ => return 0;
    }
  }

  function main() -> word { return fromEnum(Dwarf.Happy); }
}
"#,
    );
    assert_contains_in_order(
        "037dwarves",
        &dwarves,
        &[
            "/* Doc */",
            "return 1",
            "/* Grumpy */",
            "return 2",
            "/* Sleepy */",
            "return 3",
            "/* Bashful */",
            "return 4",
            "/* Happy */",
            "return 5",
            "return 0",
        ],
    );

    let food0_actual = pretty_fixture_hull("spec/038food0.solc");
    assert!(
        food0_actual.contains("function 038food0_FoodContract_main"),
        "{food0_actual}"
    );
    assert!(food0_actual.contains("return 42"), "{food0_actual}");

    let food0_shape = pretty_src_hull(
        "food0_runtime_shape",
        r#"
data Food = Curry | Beans | Other;
data CFood = Red(Food) | Green(Food) | Nocolor;

function fromEnum(x : CFood) -> word {
  assembly { mstore(0, 0) }
  match x {
    | CFood.Red(Food.Curry) => return 1;
    | CFood.Green(Food.Beans) => return 42;
    | _ => return 3;
  }
}

contract FoodContract {
  function main() -> word { return fromEnum(CFood.Green(Food.Beans)); }
}
"#,
    );
    assert_contains_in_order(
        "food0 runtime shape",
        &food0_shape,
        &[
            "/* Red */",
            "/* Curry */",
            "return 1",
            "/* Green */",
            "/* Beans */",
            "return 42",
        ],
    );

    let food = pretty_fixture_hull("spec/039food.solc");
    assert!(
        food.contains("function 039food_FoodContract_main") && food.contains("return 42"),
        "{food}"
    );

    let wildcard_after_ctor = pretty_src_hull(
        "wildcard_after_ctor",
        r#"
data Tiny = A | B | C;

contract C {
  public function pick(t : Tiny) -> word {
    assembly { mstore(0, 0) }
    match t {
      | Tiny.B => return 2;
      | _ => return 9;
    }
  }

  function main() -> word { return pick(Tiny.B); }
}
"#,
    );
    assert_contains_in_order(
        "minimal wildcard after constructor",
        &wildcard_after_ctor,
        &["/* B */", "return 2", "return 9"],
    );
}

#[test]
fn cited_terminal_yul_fixtures_do_not_fail_missing_terminator() {
    for fixture in [
        "cases/yul-return.solc",
        "cases/undefined.solc",
        "cases/copytomem.solc",
    ] {
        let kinds = check_fixture_kinds(fixture);
        assert!(
            !kinds
                .iter()
                .any(|kind| { matches!(kind, CheckDiagnosticKind::MissingTerminator { .. }) }),
            "{fixture}: {kinds:?}"
        );
    }
}

#[test]
fn recursive_adt_layouts_are_cycle_safe() {
    for fixture in ["cases/PeanoMatch.solc", "cases/listid.solc"] {
        assert_fixture_emits_and_checks(fixture);
    }
}

#[test]
fn logical_not_lowers_as_bool_sum_branch_swap() {
    let (db, output) = specialize_src_with_std(
        "logical_not",
        r#"
import std.{*};
import std.dispatch.{*};

function neq(x : word, y : word) -> bool {
  return !(x == y);
}

contract C {
  public function neqEntry(x : word, y : word) -> bool {
    return neq(x, y);
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    // Dispatch eligibility is covered by the dispatcher tests; this test cares
    // about expression lowering only.
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(non_dispatch, Vec::<&EmitDiagnostic>::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(
        hull.contains("return if<(unit + unit)> primEqWord(x, y)"),
        "{hull}"
    );
    assert!(
        hull.contains("then (inl<(unit + unit)>(())) else (inr<(unit + unit)>(()))"),
        "{hull}"
    );
    assert!(hull.contains("if<"), "{hull}");
}

#[test]
fn out_of_range_word_literals_wrap_in_hull_exprs_and_patterns() {
    const TWO_256: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639936";
    const TWO_256_PLUS_ONE: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639937";

    let hull = pretty_src_hull_with_std(
        "word_literal_wrap",
        &format!(
            r#"
import std.{{*}};
import std.dispatch.{{*}};

contract C {{
  public function exact() -> word {{
    return {TWO_256};
  }}

  public function plus() -> word {{
    return {TWO_256_PLUS_ONE};
  }}

  public function pick(x : word) -> word {{
    match x {{
      | {TWO_256} => return 10;
      | {TWO_256_PLUS_ONE} => return 11;
      | _ => return 12;
    }}
  }}

}}
"#
        ),
    );

    assert!(!hull.contains(TWO_256), "{hull}");
    assert!(!hull.contains(TWO_256_PLUS_ONE), "{hull}");
    assert!(hull.contains("rets := 0"), "{hull}");
    assert!(hull.contains("rets := 1"), "{hull}");

    let pick = hull_function(&hull, "main_C_pick_");
    assert_contains_in_order(
        "wrapped word pattern literals",
        pick,
        &[
            "match<word>",
            "0 ",
            "return 10",
            "1 ",
            "return 11",
            "return 12",
        ],
    );
}

#[test]
fn evaluator_does_not_fold_past_unknown_return() {
    let hull = pretty_src_hull_with_std(
        "eval_return_unknown_abort",
        r#"
import std.{*};
import std.dispatch.{*};

contract RetUnknown {
  function pick(flag: bool, y: word) -> word {
    match flag {
      | true => return y;
      | false => return 5;
    }
    return 0;
  }

  public function get(x: word) -> word {
    return pick(true, x);
  }

}
"#,
    );
    let get = hull_function(&hull, "main_RetUnknown_get_");
    assert!(get.contains("_pick_"), "{get}\n{hull}");
    assert!(!get.contains("return 0"), "{get}\n{hull}");
}

#[test]
fn evaluator_does_not_inline_storage_writing_helpers() {
    let mapping_hull = pretty_src_hull_with_std(
        "eval_storage_writer_mapping",
        r#"
import std.{*};

contract MappingWriter {
  m: mapping(word, word);

  function set(k: word, v: word) -> word {
    m[k] = v;
    return v;
  }

  public function main() -> word {
    let a : word = set(1, 42);
    return m[1];
  }
}
"#,
    );
    let mapping_main = hull_function(&mapping_hull, "_main_");
    assert!(
        mapping_main.contains("_set_"),
        "{mapping_main}\n{mapping_hull}"
    );
    assert!(mapping_hull.contains("sstore("), "{mapping_hull}");
    assert!(
        mapping_main.contains("sload(__solcore_storage_hash2(0, 1))"),
        "{mapping_main}\n{mapping_hull}"
    );

    let direct_hull = pretty_src_hull_with_std(
        "eval_storage_writer_direct",
        r#"
import std.{*};

contract DirectWriter {
  x: word;

  function setv(v: word) -> word {
    x = v;
    return v;
  }

  public function main() -> word {
    let a : word = setv(9);
    return x;
  }
}
"#,
    );
    let direct_main = hull_function(&direct_hull, "_main_");
    assert!(
        direct_main.contains("_setv_"),
        "{direct_main}\n{direct_hull}"
    );
    assert!(direct_hull.contains("sstore(0,"), "{direct_hull}");
    assert!(
        direct_main.contains("return sload(0)"),
        "{direct_main}\n{direct_hull}"
    );
    assert!(
        !direct_main.contains("return 9"),
        "{direct_main}\n{direct_hull}"
    );
}

#[test]
fn storage_index_assignment_materializes_slot_before_rhs() {
    let hull = pretty_src_hull_with_std(
        "storage_index_order",
        r#"
import std.{*};

contract StorageIndexOrder {
  counter: word;
  m: mapping(word, word);

  function next() -> word {
    let cur: word = counter;
    let res: word;
    assembly {
      res := add(cur, 1)
    }
    counter = res;
    return res;
  }

  public function main() -> word {
    counter = 0;
    m[next()] = next();
    return m[1];
  }
}
"#,
    );
    let main = hull_function(&hull, "_main_");
    assert_contains_in_order(
        "storage index assignment order",
        main,
        &[
            "storage_store_storage_index_slot_1 := __solcore_storage_hash2(1, main_StorageIndexOrder_next_",
            "storage_store_storage_index_2 := main_StorageIndexOrder_next_",
            "sstore(storage_store_storage_index_slot_1, storage_store_storage_index_2)",
        ],
    );

    let compound_hull = pretty_src_hull_with_std(
        "storage_index_compound",
        r#"
import std.{*};

contract StorageIndexCompound {
  counter: word;
  m: mapping(word, word);

  function next() -> word {
    let cur: word = counter;
    let res: word;
    assembly {
      res := add(cur, 1)
    }
    counter = res;
    return res;
  }

  public function main() -> word {
    counter = 0;
    m[1] = 10;
    m[next()] += next();
    return m[1];
  }
}
"#,
    );
    let compound_main = hull_function(&compound_hull, "_main_");
    assert_contains_in_order(
        "compound storage index assignment order",
        compound_main,
        &[
            "storage_store_storage_index_slot_3 := __solcore_storage_hash2(1, main_StorageIndexCompound_next_",
            "storage_store_storage_index_4 := add(sload(storage_store_storage_index_slot_3), main_StorageIndexCompound_next_",
            "sstore(storage_store_storage_index_slot_3, storage_store_storage_index_4)",
        ],
    );
    assert_eq!(
        compound_main
            .matches("main_StorageIndexCompound_next_")
            .count(),
        2,
        "{compound_main}"
    );
}

#[test]
fn evaluator_invalidates_storage_bindings_after_residual_calls() {
    let hull = pretty_src_hull(
        "eval_stale_storage_call",
        r#"
contract StaleCall {
  x: word;

  function setx() -> () {
    x = 8;
  }

  public function main() -> word {
    x = 7;
    setx();
    return x;
  }
}
"#,
    );
    let main = hull_function(&hull, "_main_");
    assert_contains_in_order(
        "stale storage call main",
        main,
        &["sstore(0,", "_setx_", "return sload(0)"],
    );
    assert!(!main.contains("return 7"), "{main}\n{hull}");
}

#[test]
fn evaluator_invalidates_residual_assembly_branch_assignments() {
    let if_hull = pretty_src_hull_with_std(
        "eval_if_asm_assignment",
        r#"
import std.{*};
import std.dispatch.{*};

contract IfAsm {
  public function f(b: bool) -> word {
    let x : word = 1;
    if (b) {
      assembly { x := 5 }
    }
    return x;
  }

}
"#,
    );
    let f = hull_function(&if_hull, "main_IfAsm_f_");
    assert!(f.contains("x := 5"), "{f}\n{if_hull}");
    assert!(f.contains("return x"), "{f}\n{if_hull}");
    assert!(!f.contains("return 1"), "{f}\n{if_hull}");

    let match_hull = pretty_src_hull_with_std(
        "eval_match_asm_assignment",
        r#"
import std.{*};
import std.dispatch.{*};

contract MatchAsm {
  public function g(b: bool) -> word {
    let x : word = 1;
    match b {
      | true => assembly { x := 5 }
      | false => {}
    }
    return x;
  }

}
"#,
    );
    let g = hull_function(&match_hull, "main_MatchAsm_g_");
    assert!(g.contains("x := 5"), "{g}\n{match_hull}");
    assert!(g.contains("return x"), "{g}\n{match_hull}");
    assert!(!g.contains("return 1"), "{g}\n{match_hull}");
}

#[test]
fn cited_nested_layout_fixtures_check_cleanly() {
    for fixture in [
        "spec/032simplejoin.solc",
        "spec/034cojoin.solc",
        "spec/043fstsnd.solc",
    ] {
        let kinds = check_fixture_kinds(fixture);
        assert!(kinds.is_empty(), "{fixture}: {kinds:?}");
    }
}

fn try_check_fixture_kinds(fixture: &str) -> Result<Vec<CheckDiagnosticKind>, String> {
    let repo = repo_root();
    let fixture_path = repo
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(fixture);
    let (db, output) = specialize_fixture(&fixture_path);
    if !output.diagnostics.is_empty() {
        return Err(format!("specialize: {:?}", output.diagnostics));
    }
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    if !non_dispatch.is_empty() {
        return Err(format!("emit: {non_dispatch:?}"));
    }
    Ok(check_program_with_db(db, &emitted.program)
        .into_iter()
        .map(|diagnostic| diagnostic.kind)
        .collect())
}

fn check_fixture_kinds(fixture: &str) -> Vec<CheckDiagnosticKind> {
    match try_check_fixture_kinds(fixture) {
        Ok(kinds) => kinds,
        Err(stage) => panic!("{fixture}: {stage}"),
    }
}

#[test]
fn mapping_field_in_value_position_lowers_to_unimplemented_trap() {
    // The reference compiles whole-mapping reads/stores via the
    // `storage(mapping(k, v)) : CanStore` instance, whose load/store are
    // `unimplemented()` runtime traps. This must not escape as an internal
    // hull-check error (previously: UndefinedVariable { name: "bal" }).
    let read_src = r#"
data mapping(key, value) = mapping(word);

contract C {
  bal : mapping(word, word);

  public function main() -> word {
    let b = bal;
    return 7;
  }
}
"#;
    let store_src = r#"
data mapping(key, value) = mapping(word);

contract C {
  bal : mapping(word, word);

  public function main() -> word {
    bal = bal;
    return 7;
  }
}
"#;
    for (name, src) in [
        ("mapping_value_read", read_src),
        ("mapping_value_store", store_src),
    ] {
        let (db, output) = specialize_src(name, src);
        assert_eq!(output.diagnostics, Vec::new(), "specialize for {name}");
        let emitted = emit_module(db, &output.module, EmitOptions::default());
        assert_eq!(emitted.diagnostics, Vec::new(), "emit for {name}");
        assert_eq!(
            check_program_with_db(db, &emitted.program),
            Vec::new(),
            "check for {name}"
        );
        let hull = pretty_program(db, &emitted.program);
        assert!(
            hull.contains("__solcore_storage_mapping_value"),
            "{name}: {hull}"
        );
        assert!(hull.contains("0x6e128399"), "{name}: {hull}");
    }
}

fn specialize_src(name: &str, src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(db, name, src);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn parse_module<'db>(db: &'db TestDb, name: &str, src: &str) -> Module<'db> {
    let url = format!("memory:///{name}.solc").parse().expect("valid URL");
    let file = SourceFile::new(db, url, Some(src.to_owned()));
    parse_file_to_hir(db, file).module(db)
}

/// Specializes an in-memory source with the standard library on the module
/// path. Unlike `specialize_src`, this mirrors the real driver: `import std`
/// and its instances (e.g. `word:Int`) resolve, so integer literals are typed
/// by their use rather than by eager defaulting.
fn specialize_src_with_std(name: &str, src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let main_root = repo_root().join("target/hull-smoke-tmp").join(name);
    fs::create_dir_all(&main_root).expect("create temp main root");
    let path = main_root.join("main.solc");
    fs::write(&path, src).expect("write temp source");
    specialize_fixture(&path)
}

fn specialize_fixture(path: &Path) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = path.parent().expect("fixture parent").to_path_buf();
    let repo = repo_root();
    let std_root = repo.join("crates/parser/tests/fixtures/corpus/ok/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        db,
        [main_root.as_path(), std_root.as_path()],
    ));
    let source = fs::read_to_string(path).expect("fixture source");
    let key =
        module_key_for_path(LibraryId::Main, &main_root, path).expect("fixture under main root");
    let file = SourceFile::new(
        db,
        url::Url::from_file_path(path).expect("file URL"),
        Some(source),
    );
    db.insert_module_file(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn module_fs_snapshot_for_roots<'a>(
    db: &TestDb,
    roots: impl IntoIterator<Item = &'a Path>,
) -> ModuleFsSnapshot {
    let mut existing_files = BTreeSet::new();
    let mut sibling_stems = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for root in roots {
        collect_module_fs_snapshot(root, &mut existing_files, &mut sibling_stems);
    }
    let sibling_stems = sibling_stems
        .into_iter()
        .map(|(parent, stems)| (parent, stems.into_iter().collect()))
        .collect();
    ModuleFsSnapshot::new(db, existing_files, sibling_stems)
}

fn collect_module_fs_snapshot(
    dir: &Path,
    existing_files: &mut BTreeSet<PathBuf>,
    sibling_stems: &mut BTreeMap<PathBuf, BTreeSet<String>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("solc") {
            if path.is_file() {
                existing_files.insert(path.clone());
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                sibling_stems
                    .entry(dir.to_path_buf())
                    .or_default()
                    .insert(stem.to_owned());
            }
        }
        if path.is_dir() {
            collect_module_fs_snapshot(&path, existing_files, sibling_stems);
        }
    }
}

fn load_reachable_modules(db: &mut TestDb, entry: ModuleKey) -> Vec<String> {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();
    let mut unresolved = Vec::new();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .chain(refs.compiler_refs)
                .filter_map(
                    |path| match resolve_module_path_candidate(&*db, module, &path) {
                        Ok(resolved) => Some((resolved.module.key(&*db), resolved.file_path)),
                        Err(_) => {
                            unresolved.push(format!(
                                "{} imports `{}`",
                                module.display(&*db),
                                module_path_display(&*db, &path)
                            ));
                            None
                        }
                    },
                )
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) {
                match fs::read_to_string(&file_path) {
                    Ok(source) => {
                        let file = SourceFile::new(
                            db,
                            url::Url::from_file_path(&file_path).expect("file URL"),
                            Some(source),
                        );
                        db.insert_module_file(target_key.clone(), file);
                    }
                    Err(err) => unresolved.push(format!("{}: {err}", file_path.display())),
                }
            }
            queue.push_back(target_key);
        }
    }
    unresolved
}

fn assert_fixture_emits_and_checks(relative: &str) {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {relative:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {relative:?}"
    );
    assert_eq!(
        check_program_with_db(db, &emitted.program),
        Vec::new(),
        "check diagnostics for {relative:?}"
    );
}

#[test]
fn overloaded_binary_operators_emit_instance_results() {
    let custom_uint =
        pretty_src_hull_with_std("operator-custom-uint-add", OPERATOR_CUSTOM_UINT_ADD);
    assert!(
        custom_uint.contains("42"),
        "custom uint Add instance was not reflected in Hull:\n{custom_uint}"
    );

    let meters = pretty_src_hull_with_std("operator-meters-add", OPERATOR_METERS_ADD);
    assert!(
        meters.contains("3"),
        "meters Add instance did not emit the expected result:\n{meters}"
    );

    let meters_ord = pretty_src_hull_with_std("operator-meters-ord", OPERATOR_METERS_ORD);
    assert!(
        meters_ord.contains("42"),
        "meters Ord instance did not emit the expected result:\n{meters_ord}"
    );

    let word = pretty_src_hull_with_std("operator-word-add", OPERATOR_WORD_ADD);
    assert!(
        word.contains("3"),
        "word Add instance changed observable Hull result:\n{word}"
    );
}

const OPERATOR_CUSTOM_UINT_ADD: &str = r#"
import std.{*};

data uint = u(word);

instance uint:Add {
  function add(x:uint, y:uint) -> uint {
    return uint.u(42);
  }
}

function unwrap(x:uint) -> word {
  match x {
  | uint.u(w) => return w;
  }
}

contract C {
  public function main() -> word {
    let a:uint = uint.u(1);
    let b:uint = uint.u(2);
    let c:uint = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ADD: &str = r#"
import std.{*};

data meters = meters(word);

instance meters:Add {
  function add(x:meters, y:meters) -> meters {
    match x, y {
    | meters(xw), meters(yw) => return meters(addWord(xw, yw));
    }
  }
}

function unwrap(x:meters) -> word {
  match x {
  | meters(w) => return w;
  }
}

contract C {
  public function main() -> word {
    let a:meters = meters(1);
    let b:meters = meters(2);
    let c:meters = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ORD: &str = r#"
import std.{*};

data meters = meters(word);

instance meters:Eq {
  function eq(x:meters, y:meters) -> bool {
    match x, y {
    | meters(xw), meters(yw) => return eqWord(xw, yw);
    }
  }
}

instance meters:Ord {
  function gt(x:meters, y:meters) -> bool {
    match x, y {
    | meters(xw), meters(yw) => return gtWord(xw, yw);
    }
  }
}

contract C {
  public function main() -> word {
    let a:meters = meters(1);
    let b:meters = meters(2);
    if (a < b) {
      return 42;
    } else {
      return 0;
    }
  }
}
"#;

const OPERATOR_WORD_ADD: &str = r#"
import std.{*};

contract C {
  public function main() -> word {
    return 1 + 2;
  }
}
"#;

fn pretty_fixture_hull(relative: &str) -> String {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    pretty_output_hull(db, output, relative)
}

fn pretty_src_hull(name: &str, src: &str) -> String {
    let (db, output) = specialize_src(name, src);
    pretty_output_hull(db, output, name)
}

fn pretty_src_hull_with_std(name: &str, src: &str) -> String {
    let (db, output) = specialize_src_with_std(name, src);
    pretty_output_hull(db, output, name)
}

fn pretty_output_hull(
    db: &'static TestDb,
    output: SpecializeOutput<'static>,
    label: &str,
) -> String {
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {label:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {label:?}"
    );
    assert_eq!(
        check_program_with_db(db, &emitted.program),
        Vec::new(),
        "check diagnostics for {label:?}"
    );
    pretty_program(db, &emitted.program)
}

fn assert_contains_in_order(label: &str, haystack: &str, needles: &[&str]) {
    let mut offset = 0;
    for needle in needles {
        let Some(found) = haystack[offset..].find(needle) else {
            panic!("{label}: missing ordered snippet {needle:?}\n{haystack}");
        };
        offset += found + needle.len();
    }
}

fn hull_function<'a>(hull: &'a str, name_fragment: &str) -> &'a str {
    let mut search_from = 0;
    while let Some(relative_start) = hull[search_from..].find("function ") {
        let start = search_from + relative_start;
        let header_line_end = hull[start..]
            .find('\n')
            .map(|offset| start + offset)
            .unwrap_or(hull.len());
        let header_end = hull[start..header_line_end]
            .rfind('{')
            .map(|offset| start + offset)
            .expect("function header has body");
        let header = &hull[start..header_end];
        let body_start = header_end + 1;
        let mut depth = 1usize;
        for (offset, ch) in hull[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let end = body_start + offset + ch.len_utf8();
                        if header.contains(name_fragment) {
                            return &hull[start..end];
                        }
                        search_from = end;
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    panic!("missing function containing {name_fragment:?}\n{hull}");
}

fn assert_fixture_emits_without_match_lowering_regressions(relative: &str) {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {relative:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {relative:?}"
    );

    let checked = check_program_with_db(db, &emitted.program);
    assert!(
        !checked.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            CheckDiagnosticKind::UndefinedVariable { name } if name.starts_with("$alt")
        )),
        "unbound alt diagnostic for {relative:?}: {checked:?}"
    );
    let unexpected: Vec<_> = checked
        .iter()
        .filter(|diagnostic| {
            !matches!(
                diagnostic.kind,
                CheckDiagnosticKind::ExprAnnotationMismatch { .. }
                    | CheckDiagnosticKind::TypeMismatch { .. }
            )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected check diagnostics for {relative:?}: {unexpected:?}"
    );
}

fn assert_fixture_has_no_unbound_alt(relative: &str) {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {relative:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {relative:?}"
    );
    let checked = check_program_with_db(db, &emitted.program);
    assert!(
        !checked.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            CheckDiagnosticKind::UndefinedVariable { name } if name.starts_with("$alt")
        )),
        "unbound alt diagnostic for {relative:?}: {checked:?}"
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under repo/crates/hull")
        .to_path_buf()
}
