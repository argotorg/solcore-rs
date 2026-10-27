use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        function::{
            AssignOp, BinOp, LitKind, UnOp, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
        },
        item::{AdtDef, ContractDef, ContractItem, Item, Module},
        ty::TypeRefKind,
    },
    diag::{Diagnostic, DiagnosticCode},
    span::{Span, Spanned, SpannedElem},
};
use hir_ty::{
    AliasNormalizer, BinderEnv, BuiltinTyCtor, Ty as SemTy, TyCtor, TyKind as SemTyKind,
    TypeLowering, UserTyCtorKind, is_canonical_std_def_named,
};
use parser::parse_file_to_hir;
use specialize::{
    MonoArm, MonoBuiltinCtor, MonoCallOrigin, MonoContract, MonoEntry, MonoExpr, MonoExprArm,
    MonoExprKind, MonoFunction, MonoId, MonoIntrinsic, MonoItem, MonoModule, MonoPat, MonoPatKind,
    MonoStmt, MonoStmtKind, MonoStorageIndexKind, collect_leaking_let_stmts, decode_string_literal,
};

use crate::{
    ir::{
        Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
        StmtKind, Ty, TyKind,
    },
    scope_stack::ScopeStack,
    word::{canonical_word_literal, wrap_word_literal},
};

mod assembly;
mod contract;
mod diagnostics;
mod emitter;
mod layout;
mod layout_finiteness;
mod match_compile;
mod reachability;
mod storage;
mod yul_build;

use diagnostics::prune_emit_diagnostics;
pub use diagnostics::{EmitDiagnostic, EmitDiagnosticKind, EmitOptions, EmitOutput};
pub use emitter::emit_module;
use layout::{
    bool_expr, hull_ty_word_slots, product_expr, product_field_exprs, sem_product_fields,
    sem_ty_needs_untyped_word_default, sum_right_ty,
};
use match_compile::{AdtLayout, CtorLayout, constructor_index, encode_constructor, wrap_lit_text};
use reachability::deployment_closure;
use storage::StorageFieldKind;

const STORAGE_INDEX_READ: &str = "__solcore_storage_index_read";
const STORAGE_INDEX_SLOT: &str = "__solcore_storage_index_slot";
const STORAGE_HASH2_HELPER: &str = "__solcore_storage_hash2";
const STORAGE_ARRAY_SLOT_HELPER: &str = "__solcore_storage_array_slot";
const STORAGE_MAPPING_VALUE_HELPER: &str = "__solcore_storage_mapping_value";
/// Error selector of std's `Unimplemented` error
/// (`Error(0x6e128399)` raised by `unimplemented()` in std.sol).
const UNIMPLEMENTED_SELECTOR: &str = "0x6e128399";
const OUT_OF_BOUNDS_SELECTOR: &str = "0xb4120f14";

struct Emitter<'db> {
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    _options: EmitOptions,
    diagnostics: Vec<EmitDiagnostic<'db>>,
    scopes: ScopeStack<BTreeMap<String, Expr<'db>>>,
    function_names: BTreeSet<String>,
    layout_stack: Vec<(DefId<'db>, Vec<SemTy<'db>>)>,
    layout_finiteness: layout_finiteness::LayoutFiniteness<'db>,
    predeclared_lets: Vec<PredeclaredLet<'db>>,
    /// Runtime allocators for compile-time string literals, keyed by decoded
    /// UTF-8 bytes so equivalent literal spellings share one helper.
    string_literals: BTreeMap<Vec<u8>, StringLiteralHelper<'db>>,
    next_string_literal: usize,
    /// Runtime helpers for literal revert messages. A helper call remains an
    /// expression, preserving conditional and argument evaluation boundaries.
    revert_literals: BTreeMap<String, RevertLiteralHelper<'db>>,
    next_revert_literal: usize,
    storage_array_index_used: bool,
    fresh: usize,
}

/// Identity of a declaration occurrence in the immutable Mono tree currently
/// being emitted. Source spans identify provenance, not bindings: generated
/// captures and cloned bodies can contain distinct declarations with equal spans.
/// Entries are retained only while their owning control-flow tree is borrowed;
/// this pointer is compared for identity and is never dereferenced.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DeclarationId<'db>(*const MonoStmt<'db>);

impl<'db> DeclarationId<'db> {
    fn of(stmt: &MonoStmt<'db>) -> Self {
        Self(std::ptr::from_ref(stmt))
    }
}

#[derive(Clone)]
struct PredeclaredLet<'db> {
    declaration: DeclarationId<'db>,
    backend_name: String,
    ty: Ty<'db>,
}

#[derive(Clone)]
struct StringLiteralHelper<'db> {
    span: Span<'db>,
    name: String,
}

#[derive(Clone)]
struct RevertLiteralHelper<'db> {
    span: Span<'db>,
    name: String,
}
