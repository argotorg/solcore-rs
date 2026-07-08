use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        function::{BinOp, LitKind, UnOp, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
        item::{AdtDef, ContractDef, ContractItem, Item, Module},
        ty::TypeRefKind,
    },
    diag::{Diagnostic, DiagnosticCode},
    span::{Span, Spanned, SpannedElem},
};
use hir_ty::{
    BinderEnv, BuiltinTyCtor, Ty as SemTy, TyCtor, TyKind as SemTyKind, TypeLowering,
    UserTyCtorKind,
};
use parser::parse_file_to_hir;
use specialize::{
    MonoAbiParam, MonoArm, MonoCallOrigin, MonoContract, MonoEntry, MonoExpr, MonoExprKind,
    MonoFunction, MonoIntrinsic, MonoItem, MonoModule, MonoPat, MonoPatKind, MonoStmt,
    MonoStmtKind,
};

use crate::{
    ir::{
        Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
        StmtKind, Ty, TyKind,
    },
    word::wrap_word_literal,
};

mod abi;
mod contract;
mod diagnostics;
mod dispatch;
mod emitter;
mod layout;
mod match_compile;
mod reachability;
mod storage;
mod yul_build;

use abi::{
    AbiWordKind, StaticAbiLayout, abi_layout_slot_kinds, abi_word_kind, abi_word_to_bool_expr,
    abi_words_to_expr, constructor_inputs_are_static_word, dispatcher_input_layouts,
    dispatcher_return_layout, numbered_name, selector_hex, write_expr_to_abi_slots,
};
use diagnostics::prune_emit_diagnostics;
use layout::{
    bool_expr, bool_sum_ty, hull_ty_is_bool_word, hull_ty_word_slots, product_expr,
    product_field_exprs, sem_product_fields, sem_ty_needs_untyped_word_default, sum_right_ty,
};
use match_compile::{
    AdtLayout, CtorLayout, constructor_name_matches, encode_constructor, wrap_lit_text,
};
use reachability::deployment_closure;
use storage::StorageFieldKind;

pub use diagnostics::{EmitDiagnostic, EmitDiagnosticKind, EmitOptions, EmitOutput};
pub use emitter::emit_module;

const ADDRESS_MASK: &str = "0xffffffffffffffffffffffffffffffffffffffff";
const STORAGE_INDEX_READ: &str = "__solcore_storage_index_read";
const STORAGE_INDEX_SLOT: &str = "__solcore_storage_index_slot";
const STORAGE_HASH2_HELPER: &str = "__solcore_storage_hash2";
const STORAGE_MAPPING_VALUE_HELPER: &str = "__solcore_storage_mapping_value";
/// Error selector of the reference std's `Unimplemented` error
/// (`Error(0x6e128399)` raised by `unimplemented()` in std.solc).
const UNIMPLEMENTED_SELECTOR: &str = "0x6e128399";

struct Emitter<'db> {
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    options: EmitOptions,
    diagnostics: Vec<EmitDiagnostic<'db>>,
    scopes: Vec<BTreeMap<String, Expr<'db>>>,
    function_names: BTreeSet<String>,
    layout_stack: Vec<(DefId<'db>, Vec<SemTy<'db>>)>,
    fresh: usize,
}
