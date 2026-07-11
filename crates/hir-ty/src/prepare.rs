//! Compiler-owned HIR overlays built before type checking.
//!
//! Preparation never changes or reparses the user's [`hir::input::SourceFile`].  It keeps
//! the parsed module as the source-of-record and constructs a second tracked
//! [`Module`] containing unresolved generated declarations.  Name resolution,
//! type checking, and specialization can therefore process generated code by
//! the same paths as source HIR while editor-facing consumers keep using the
//! source module.
//!
//! The overlay is intentionally the home for declaration-level rewrites that
//! create names, instances, or cross-body entry points. Body-local tuple,
//! boolean, conditional, and implicit-return desugars remain tracked views so
//! a small body edit does not rebuild a whole module. Contract constructor and
//! deployment wrappers live here alongside runtime dispatch; derived instances
//! and semantic field/call hooks can migrate here once their required
//! resolution inputs are one-way dependencies.

use std::collections::BTreeSet;

use hir::{
    anchor::{DefId, DefKind, Disambiguator},
    arena::{Arena, Id},
    ast::{
        Ident,
        function::{
            AssignOp, Expr, ExprKind, FuncBody, FuncParam, FuncSig, LitKind, MatchArm, Pat,
            PatKind, Stmt, StmtKind, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
        },
        item::{
            AdtDef, ContractDef, ContractItem, FuncKind, FunctionDef, Import, ImportSelector,
            InstanceDef, Item, Module, SelectedName,
        },
        ty::{PredRef, PredRefKind, TypeRef, TypeRefKind},
    },
    nameres::ident_text,
    span::{Span, Spanned, SpannedElem},
};

use crate::{Db, contract_needs_generated_dispatch};
use nameres::{LibraryId, module_id_for_source_file, resolve_direct_import_target_candidate};

const GENERATED_MAIN_NAME: &str = "$solcore$dispatch$main";
const MAIN_FINGERPRINT: &str = "solcore.generated.std_dispatch.main";
const MAIN_BODY_FINGERPRINT: &str = "solcore.generated.std_dispatch.main.body";
const CONSTRUCTOR_INIT_NAME: &str = "$solcore$constructor$init_";
const CONSTRUCTOR_COPY_NAME: &str = "$solcore$constructor$copy_arguments_for_constructor";
const DEPLOYMENT_MAIN_NAME: &str = "$solcore$constructor$start";
const CONSTRUCTOR_INIT_FINGERPRINT: &str = "solcore.generated.constructor.init";
const CONSTRUCTOR_INIT_BODY_FINGERPRINT: &str = "solcore.generated.constructor.init.body";
const CONSTRUCTOR_COPY_FINGERPRINT: &str = "solcore.generated.constructor.copy_arguments";
const CONSTRUCTOR_COPY_BODY_FINGERPRINT: &str = "solcore.generated.constructor.copy_arguments.body";
const DEPLOYMENT_MAIN_FINGERPRINT: &str = "solcore.generated.constructor.deployment_main";
const DEPLOYMENT_MAIN_BODY_FINGERPRINT: &str = "solcore.generated.constructor.deployment_main.body";
// `$` is not accepted by the source lexer, so user declarations cannot
// capture these effective-HIR-only bindings.
const STD_MODULE_ALIAS: &str = "$solcore$std";
const STD_DISPATCH_MODULE_ALIAS: &str = "$solcore$std$dispatch";
const SIG_STRING_CLASS_ALIAS: &str = "$solcore$SigString";
const STD_MODULE_IMPORT_FINGERPRINT: &str = "solcore.generated.import.std";
const STD_DISPATCH_MODULE_IMPORT_FINGERPRINT: &str = "solcore.generated.import.std_dispatch";
const SIG_STRING_IMPORT_FINGERPRINT: &str = "solcore.generated.import.std_dispatch.sig_string";

/// A source module paired with its compiler-prepared HIR overlay.
///
/// `source` and `module` intentionally remain separate even though their
/// module [`DefId`] values are equal.  The tracked `Module` handles distinguish
/// source-only queries from prepared semantic queries; consumers must not use
/// only the module `DefId` as a cache key when both can be present.
#[salsa::tracked(debug)]
pub struct PreparedModule<'db> {
    /// Parsed, user-authored HIR.  LSP and source diagnostics use this module.
    #[tracked]
    #[returns(copy)]
    pub source: Module<'db>,

    /// Effective HIR consumed by name resolution, type checking, and backends.
    #[tracked]
    #[returns(copy)]
    pub module: Module<'db>,

    /// Provenance for compiler-owned definitions in `module`.
    #[tracked]
    #[returns(ref)]
    pub origins: GeneratedOriginMap<'db>,
}

impl<'db> PreparedModule<'db> {
    /// Returns the generated origin for `def`, if it belongs to this overlay.
    pub fn origin_for_def(
        self,
        db: &'db dyn Db,
        def: DefId<'db>,
    ) -> Option<&'db GeneratedOrigin<'db>> {
        self.origins(db).origin_for_def(def)
    }

    /// Returns the compiler-owned runtime main for `contract`, if generated.
    pub fn contract_dispatch_main(
        self,
        db: &'db dyn Db,
        contract: DefId<'db>,
    ) -> Option<DefId<'db>> {
        self.origins(db).contract_dispatch_main(contract)
    }

    /// Returns the compiler-owned deployment entry for `contract`, if generated.
    pub fn contract_deployment_main(
        self,
        db: &'db dyn Db,
        contract: DefId<'db>,
    ) -> Option<DefId<'db>> {
        self.origins(db).contract_deployment_main(contract)
    }
}

/// Why a definition exists only in a prepared module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum GeneratedOriginKind {
    /// Compiler-owned contract runtime entry backed by `std.dispatch`.
    ContractDispatchMain,
    /// Phantom method-name data type used by `std.dispatch.Method`.
    ContractDispatchNameType,
    /// `SigString` instance for a phantom method-name type.
    ContractDispatchSigStringInstance,
    /// `sigStr` method (or its body) in a generated `SigString` instance.
    ContractDispatchSigStringMethod,
    /// Private compiler-only function containing the source constructor body.
    ContractConstructorInit,
    /// Private constructor-argument copying and ABI-decoding helper.
    ContractConstructorCopyArguments,
    /// Compiler-owned deployment entry that installs the runtime object.
    ContractDeploymentMain,
}

/// Returns whether `def` is a compiler-owned std.dispatch runtime entry.
///
/// The fingerprint fallback keeps provenance available to consumers that are
/// handed an already-prepared [`Module`] without its [`PreparedModule`]
/// wrapper. Source-lowered functions never receive this reserved fingerprint.
pub fn is_contract_dispatch_main_def(db: &dyn Db, def: DefId<'_>) -> bool {
    def.kind(db) == DefKind::Function && def.fingerprint(db).as_deref() == Some(MAIN_FINGERPRINT)
}

/// Returns whether `def` is a compiler-owned contract deployment entry.
///
/// Like [`is_contract_dispatch_main_def`], the fingerprint fallback supports
/// consumers handed an already-prepared module without its wrapper.
pub fn is_contract_deployment_main_def(db: &dyn Db, def: DefId<'_>) -> bool {
    def.kind(db) == DefKind::Function
        && def.fingerprint(db).as_deref() == Some(DEPLOYMENT_MAIN_FINGERPRINT)
}

/// Stable backend spelling for a compiler-private contract overlay function.
///
/// Effective-HIR names are intentionally impossible to spell in source so
/// they cannot capture user references. Backends may retain the established
/// readable names because their qualified names also carry a DefId hash.
pub fn contract_overlay_backend_name(db: &dyn Db, def: DefId<'_>) -> Option<&'static str> {
    if def.kind(db) != DefKind::Function {
        return None;
    }
    match def.fingerprint(db).as_deref()? {
        MAIN_FINGERPRINT => Some("main"),
        CONSTRUCTOR_INIT_FINGERPRINT => Some("init_"),
        CONSTRUCTOR_COPY_FINGERPRINT => Some("copy_arguments_for_constructor"),
        DEPLOYMENT_MAIN_FINGERPRINT => Some("_start"),
        _ => None,
    }
}

/// User-source provenance for one compiler-owned definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct GeneratedOrigin<'db> {
    pub def: DefId<'db>,
    pub kind: GeneratedOriginKind,
    pub contract: DefId<'db>,
    pub method: Option<DefId<'db>>,
    pub span: Span<'db>,
}

/// Deterministically ordered generated-definition provenance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, salsa::Update)]
pub struct GeneratedOriginMap<'db> {
    entries: Vec<GeneratedOrigin<'db>>,
}

impl<'db> GeneratedOriginMap<'db> {
    pub fn entries(&self) -> &[GeneratedOrigin<'db>] {
        &self.entries
    }

    pub fn origin_for_def(&self, def: DefId<'db>) -> Option<&GeneratedOrigin<'db>> {
        self.entries.iter().find(|origin| origin.def == def)
    }

    pub fn contract_dispatch_main(&self, contract: DefId<'db>) -> Option<DefId<'db>> {
        self.entries.iter().find_map(|origin| {
            (origin.contract == contract
                && origin.kind == GeneratedOriginKind::ContractDispatchMain)
                .then_some(origin.def)
        })
    }

    pub fn contract_deployment_main(&self, contract: DefId<'db>) -> Option<DefId<'db>> {
        self.entries.iter().find_map(|origin| {
            (origin.contract == contract
                && origin.kind == GeneratedOriginKind::ContractDeploymentMain)
                .then_some(origin.def)
        })
    }

    fn extend(&mut self, origins: impl IntoIterator<Item = GeneratedOrigin<'db>>) {
        self.entries.extend(origins);
    }
}

/// Builds the pre-typecheck HIR overlay for `source`.
///
/// This query is deliberately source-only: it inspects explicit function
/// signatures, but never calls body inference,
/// module type checking, or specialization.  That one-way dependency avoids
/// `typeck -> prepare -> typeck` query cycles. Runtime dispatch dependencies
/// are compiler-owned imports with private aliases in the effective module;
/// ABI-decoding a non-empty constructor still requires canonical `std` in the
/// source module.
#[salsa::tracked]
pub fn prepare_module<'db>(db: &'db dyn Db, source: Module<'db>) -> PreparedModule<'db> {
    let has_std = module_has_canonical_std_import(db, source);

    let module_def = source.def_id_value(db);
    let mut generated_items = Vec::new();
    let mut prepared_source_items = Vec::with_capacity(source.items(db).len());
    let mut origins = GeneratedOriginMap::default();
    let mut generated_dispatch = false;
    let mut generated_constructor_std = false;

    for item in source.items(db) {
        let Item::ContractDef(contract) = *item else {
            prepared_source_items.push(*item);
            continue;
        };

        let mut prepared_contract = contract;
        let constructor_needs_std = contract_constructor_needs_std(db, contract);
        if !contract_has_prepared_constructor(db, contract)
            && (has_std || !constructor_needs_std)
            && let Some(artifacts) = prepare_contract_constructor(db, prepared_contract)
        {
            generated_constructor_std |= constructor_needs_std;
            origins.extend(artifacts.origins.iter().cloned());
            prepared_contract = artifacts.contract;
        }

        if contract_needs_generated_dispatch(db, contract)
            && !contract_has_prepared_dispatch(db, prepared_contract)
            && let Some(artifacts) = prepare_contract_dispatch(db, module_def, prepared_contract)
        {
            generated_dispatch = true;
            generated_items.extend(artifacts.top_level_items.iter().copied());
            origins.extend(artifacts.origins.iter().cloned());
            prepared_contract = artifacts.contract;
        }
        prepared_source_items.push(Item::ContractDef(prepared_contract));
    }

    if origins.entries.is_empty() {
        return PreparedModule::new(db, source, source, origins);
    }

    if generated_dispatch {
        generated_items.splice(
            0..0,
            generated_dispatch_imports(db, module_def, source.span(db))
                .into_iter()
                .map(Item::Import),
        );
    } else if generated_constructor_std {
        generated_items.insert(
            0,
            Item::Import(generated_std_import(db, module_def, source.span(db))),
        );
    }

    generated_items.extend(prepared_source_items);
    let effective = Module::new(
        db,
        source.def_id_value(db),
        source.span(db),
        generated_items,
    );
    PreparedModule::new(db, source, effective, origins)
}

fn generated_dispatch_imports<'db>(
    db: &'db dyn Db,
    module: DefId<'db>,
    span: Span<'db>,
) -> [Import<'db>; 3] {
    [
        generated_std_import(db, module, span),
        generated_module_alias_import(
            db,
            module,
            span,
            &["std", "dispatch"],
            STD_DISPATCH_MODULE_ALIAS,
            STD_DISPATCH_MODULE_IMPORT_FINGERPRINT,
        ),
        generated_selected_alias_import(
            db,
            module,
            span,
            &["std", "dispatch"],
            "SigString",
            SIG_STRING_CLASS_ALIAS,
            SIG_STRING_IMPORT_FINGERPRINT,
        ),
    ]
}

fn generated_std_import<'db>(db: &'db dyn Db, module: DefId<'db>, span: Span<'db>) -> Import<'db> {
    generated_module_alias_import(
        db,
        module,
        span,
        &["std"],
        STD_MODULE_ALIAS,
        STD_MODULE_IMPORT_FINGERPRINT,
    )
}

fn generated_module_alias_import<'db>(
    db: &'db dyn Db,
    module: DefId<'db>,
    span: Span<'db>,
    path: &[&str],
    alias: &str,
    fingerprint: &str,
) -> Import<'db> {
    Import::new(
        db,
        generated_def(db, module, DefKind::Import, alias, fingerprint),
        span,
        Vec::new(),
        None,
        path.iter()
            .map(|segment| spanned_ident(db, span, segment))
            .collect(),
        Some(spanned_ident(db, span, alias)),
        None,
        Vec::new(),
    )
}

fn generated_selected_alias_import<'db>(
    db: &'db dyn Db,
    module: DefId<'db>,
    span: Span<'db>,
    path: &[&str],
    name: &str,
    alias: &str,
    fingerprint: &str,
) -> Import<'db> {
    Import::new(
        db,
        generated_def(db, module, DefKind::Import, alias, fingerprint),
        span,
        Vec::new(),
        None,
        path.iter()
            .map(|segment| spanned_ident(db, span, segment))
            .collect(),
        None,
        Some(ImportSelector::Names(vec![SelectedName {
            name: spanned_ident(db, span, name),
            alias: Some(spanned_ident(db, span, alias)),
            constructors: None,
            is_operator: false,
        }])),
        Vec::new(),
    )
}

/// Returns whether `module` explicitly wildcard-imports canonical `std`.
///
/// Non-empty constructor wrappers depend on `abi_decode`, `memory(bytes)`,
/// `Proxy`, `slice`, and `BoundedMemoryWordReader`. The enablement gate remains
/// source-owned, while generated references use a compiler-private qualified
/// alias after the import has been validated. Nullary and implicit constructors
/// have no std dependency and are always prepared.
pub fn module_has_canonical_std_import<'db>(db: &'db dyn Db, module: Module<'db>) -> bool {
    let Some(importing) = module_id_for_source_file(db, module.def_id_value(db).file(db)) else {
        return false;
    };
    module.items(db).iter().any(|item| {
        let Item::Import(import) = *item else {
            return false;
        };
        is_canonical_std_wildcard_import(db, importing, import)
    })
}

pub fn contract_constructor_needs_std(db: &dyn Db, contract: ContractDef<'_>) -> bool {
    let mut constructors = contract.items(db).iter().filter_map(|item| match item {
        ContractItem::FunctionDef(function) if function.kind(db) == FuncKind::Constructor => {
            Some(*function)
        }
        _ => None,
    });
    let Some(constructor) = constructors.next() else {
        return false;
    };
    constructors.next().is_none()
        && !constructor.sig(db).params.atom().is_empty()
        && explicit_param_types(constructor.sig(db).params.atom()).is_some()
}

fn contract_has_prepared_constructor(db: &dyn Db, contract: ContractDef<'_>) -> bool {
    contract.items(db).iter().any(|item| {
        matches!(
            item,
            ContractItem::FunctionDef(function)
                if is_contract_deployment_main_def(db, function.def_id_value(db))
        )
    })
}

fn contract_has_prepared_dispatch(db: &dyn Db, contract: ContractDef<'_>) -> bool {
    contract.items(db).iter().any(|item| {
        matches!(
            item,
            ContractItem::FunctionDef(function)
                if is_contract_dispatch_main_def(db, function.def_id_value(db))
        )
    })
}

fn is_canonical_std_wildcard_import<'db>(
    db: &'db dyn Db,
    importing: nameres::ModuleId<'db>,
    import: Import<'db>,
) -> bool {
    import.external(db).is_none()
        && import.alias_elem(db).is_none()
        && import.hiding(db).is_empty()
        && matches!(import.selector(db), Some(ImportSelector::Wildcard))
        && matches!(
            import.path_elems(db).as_slice(),
            [segment] if ident_text(db, segment) == "std"
        )
        && resolve_direct_import_target_candidate(db, importing, import).is_ok_and(|target| {
            target.module.library(db) == &LibraryId::Std
                && target.module.logical_path(db).as_slice() == ["std"]
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
struct PreparedContractArtifacts<'db> {
    contract: ContractDef<'db>,
    top_level_items: Vec<Item<'db>>,
    origins: Vec<GeneratedOrigin<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
struct RawMethod<'db> {
    def: DefId<'db>,
    name: String,
    span: Span<'db>,
    payable: bool,
    params: Vec<TypeRef<'db>>,
    ret: TypeRef<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
struct RawFallback<'db> {
    name: String,
    payable: bool,
    params: Vec<TypeRef<'db>>,
    ret: TypeRef<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
struct RawConstructor<'db> {
    def: Option<DefId<'db>>,
    span: Span<'db>,
    params: Vec<FuncParam<'db>>,
    param_types: Vec<TypeRef<'db>>,
    body: Option<FuncBody<'db>>,
    payable: bool,
}

#[salsa::tracked]
fn prepare_contract_constructor<'db>(
    db: &'db dyn Db,
    contract: ContractDef<'db>,
) -> Option<PreparedContractArtifacts<'db>> {
    let constructors = contract
        .items(db)
        .iter()
        .filter_map(|item| match item {
            ContractItem::FunctionDef(function) if function.kind(db) == FuncKind::Constructor => {
                Some(*function)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if constructors.len() > 1 {
        // Source diagnostics own the duplicate-constructor error. Do not add a
        // deployment entry to structurally invalid recovery input.
        return None;
    }

    let span = constructors.first().map_or_else(
        || contract.name_elem(db).span(db),
        |function| function.span(db),
    );
    let constructor = if let Some(function) = constructors.first().copied() {
        let sig = function.sig(db);
        RawConstructor {
            def: Some(function.def_id_value(db)),
            span,
            params: sig.params.atom().clone(),
            param_types: explicit_param_types(sig.params.atom())?,
            body: function.body(db),
            payable: sig.payable.is_some(),
        }
    } else {
        RawConstructor {
            def: None,
            span,
            params: Vec::new(),
            param_types: Vec::new(),
            body: None,
            payable: false,
        }
    };

    let contract_def = contract.def_id_value(db);
    let contract_name = ident_text(db, &contract.name_elem(db));
    let (init, mut init_origins) = generated_constructor_init(db, contract_def, &constructor);
    let (copy, mut copy_origins) =
        generated_constructor_copy_arguments(db, contract_def, &contract_name, &constructor);
    let (start, mut start_origins) =
        generated_deployment_main(db, contract_def, &contract_name, &constructor);

    let mut contract_items = contract
        .items(db)
        .iter()
        .filter(|item| {
            !matches!(
                item,
                ContractItem::FunctionDef(function)
                    if function.kind(db) == FuncKind::Constructor
            )
        })
        .copied()
        .collect::<Vec<_>>();
    contract_items.extend([
        ContractItem::FunctionDef(init),
        ContractItem::FunctionDef(copy),
        ContractItem::FunctionDef(start),
    ]);
    let prepared_contract = ContractDef::new(
        db,
        contract_def,
        contract.span(db),
        contract.leading_comments(db).clone(),
        contract.name_elem(db),
        contract.ty_param_elems(db).clone(),
        contract.fields(db).clone(),
        contract.field_comments(db).clone(),
        contract_items,
    );
    let mut origins = Vec::new();
    origins.append(&mut init_origins);
    origins.append(&mut copy_origins);
    origins.append(&mut start_origins);
    Some(PreparedContractArtifacts {
        contract: prepared_contract,
        top_level_items: Vec::new(),
        origins,
    })
}

fn generated_constructor_init<'db>(
    db: &'db dyn Db,
    contract: DefId<'db>,
    constructor: &RawConstructor<'db>,
) -> (FunctionDef<'db>, Vec<GeneratedOrigin<'db>>) {
    let function_def = generated_def(
        db,
        contract,
        DefKind::Function,
        CONSTRUCTOR_INIT_NAME,
        CONSTRUCTOR_INIT_FINGERPRINT,
    );
    let body_def = generated_def(
        db,
        function_def,
        DefKind::FuncBody,
        CONSTRUCTOR_INIT_NAME,
        CONSTRUCTOR_INIT_BODY_FINGERPRINT,
    );
    let body = constructor.body.map_or_else(
        || generated_unit_body(db, body_def, constructor.span),
        |source| clone_body_with_def(db, source, body_def),
    );
    let sig = FuncSig {
        span: constructor.span,
        type_vars: Vec::new(),
        preds: Vec::new(),
        public: None,
        payable: None,
        name: spanned_ident(db, constructor.span, CONSTRUCTOR_INIT_NAME),
        params: SpannedElem::new(constructor.params.clone(), constructor.span),
        ret: Some(unit_ty(db, constructor.span)),
    };
    let function = FunctionDef::new(
        db,
        function_def,
        constructor.span,
        FuncKind::Function,
        Vec::new(),
        sig,
        Some(body),
    );
    (
        function,
        generated_function_origins(
            function_def,
            body_def,
            GeneratedOriginKind::ContractConstructorInit,
            contract,
            constructor.def,
            constructor.span,
        ),
    )
}

fn generated_constructor_copy_arguments<'db>(
    db: &'db dyn Db,
    contract: DefId<'db>,
    contract_name: &str,
    constructor: &RawConstructor<'db>,
) -> (FunctionDef<'db>, Vec<GeneratedOrigin<'db>>) {
    let function_def = generated_def(
        db,
        contract,
        DefKind::Function,
        CONSTRUCTOR_COPY_NAME,
        CONSTRUCTOR_COPY_FINGERPRINT,
    );
    let body_def = generated_def(
        db,
        function_def,
        DefKind::FuncBody,
        CONSTRUCTOR_COPY_NAME,
        CONSTRUCTOR_COPY_BODY_FINGERPRINT,
    );
    let args_ty = product_ty(db, constructor.span, &constructor.param_types);
    let mut builder = BodyBuilder::new(db, constructor.span);
    let top_level_stmts = if constructor.params.is_empty() {
        let unit = builder.alloc_expr(ExprKind::Tuple(Vec::new()));
        vec![builder.alloc_stmt(StmtKind::Return(Some(unit)))]
    } else {
        let res = builder.let_stmt("res", Some(args_ty), None);
        let args_proxy = builder.qualified_proxy(STD_MODULE_ALIAS, args_ty);
        let minimum_size_value = builder.call_path(
            &[STD_MODULE_ALIAS, "ABIAttribs", "headSize"],
            vec![args_proxy],
        );
        let minimum_size = builder.let_stmt(
            "minimumSize",
            Some(named_ty(db, constructor.span, "word", Vec::new())),
            Some(minimum_size_value),
        );
        let arg_size = builder.let_stmt(
            "argSize",
            Some(named_ty(db, constructor.span, "word", Vec::new())),
            None,
        );
        let memory_offset = builder.let_stmt(
            "memoryDataOffset",
            Some(named_ty(db, constructor.span, "word", Vec::new())),
            None,
        );
        let copy = builder.alloc_stmt(StmtKind::Assembly {
            body: constructor_copy_yul(db, constructor.span, contract_name),
        });
        let offset = builder.ident("memoryDataOffset");
        let memory_value = builder.call_path(&[STD_MODULE_ALIAS, "memory", "memory"], vec![offset]);
        let arg_size_expr = builder.ident("argSize");
        let bounded_source = builder.call_path(
            &[STD_MODULE_ALIAS, "slice", "slice"],
            vec![memory_value, arg_size_expr],
        );
        let source_ty = qualified_named_ty(
            db,
            constructor.span,
            STD_MODULE_ALIAS,
            "slice",
            vec![qualified_named_ty(
                db,
                constructor.span,
                STD_MODULE_ALIAS,
                "memory",
                vec![qualified_named_ty(
                    db,
                    constructor.span,
                    STD_MODULE_ALIAS,
                    "bytes",
                    Vec::new(),
                )],
            )],
        );
        let source = builder.let_stmt("source", Some(source_ty), Some(bounded_source));
        let source_expr = builder.ident("source");
        let args_proxy = builder.qualified_proxy(STD_MODULE_ALIAS, args_ty);
        let reader_proxy = builder.qualified_proxy(
            STD_MODULE_ALIAS,
            qualified_named_ty(
                db,
                constructor.span,
                STD_MODULE_ALIAS,
                "BoundedMemoryWordReader",
                Vec::new(),
            ),
        );
        let decoded = builder.call_path(
            &[STD_MODULE_ALIAS, "abi_decode"],
            vec![source_expr, args_proxy, reader_proxy],
        );
        let lhs = builder.ident("res");
        let assign = builder.alloc_stmt(StmtKind::Assign {
            op: AssignOp::Plain,
            lhs,
            rhs: decoded,
        });
        let result = builder.ident("res");
        let ret = builder.alloc_stmt(StmtKind::Return(Some(result)));
        vec![
            res,
            minimum_size,
            arg_size,
            memory_offset,
            copy,
            source,
            assign,
            ret,
        ]
    };
    let (stmts, exprs, pats) = builder.finish();
    let body = FuncBody::new(
        db,
        body_def,
        constructor.span,
        top_level_stmts,
        stmts,
        exprs,
        pats,
    );
    let sig = FuncSig {
        span: constructor.span,
        type_vars: Vec::new(),
        preds: Vec::new(),
        public: None,
        payable: None,
        name: spanned_ident(db, constructor.span, CONSTRUCTOR_COPY_NAME),
        params: SpannedElem::new(Vec::new(), constructor.span),
        ret: Some(args_ty),
    };
    let function = FunctionDef::new(
        db,
        function_def,
        constructor.span,
        FuncKind::Function,
        Vec::new(),
        sig,
        Some(body),
    );
    (
        function,
        generated_function_origins(
            function_def,
            body_def,
            GeneratedOriginKind::ContractConstructorCopyArguments,
            contract,
            constructor.def,
            constructor.span,
        ),
    )
}

fn generated_deployment_main<'db>(
    db: &'db dyn Db,
    contract: DefId<'db>,
    contract_name: &str,
    constructor: &RawConstructor<'db>,
) -> (FunctionDef<'db>, Vec<GeneratedOrigin<'db>>) {
    let function_def = generated_def(
        db,
        contract,
        DefKind::Function,
        DEPLOYMENT_MAIN_NAME,
        DEPLOYMENT_MAIN_FINGERPRINT,
    );
    let body_def = generated_def(
        db,
        function_def,
        DefKind::FuncBody,
        DEPLOYMENT_MAIN_NAME,
        DEPLOYMENT_MAIN_BODY_FINGERPRINT,
    );
    let args_ty = product_ty(db, constructor.span, &constructor.param_types);
    let mut builder = BodyBuilder::new(db, constructor.span);
    let mut top_level_stmts = vec![builder.alloc_stmt(StmtKind::Assembly {
        body: deployment_setup_yul(db, constructor.span, contract_name),
    })];
    if !constructor.payable {
        top_level_stmts.push(builder.alloc_stmt(StmtKind::Assembly {
            body: nonpayable_constructor_yul(db, constructor.span),
        }));
    }
    let copy_args = builder.call_ident(CONSTRUCTOR_COPY_NAME, Vec::new());
    top_level_stmts.push(builder.let_stmt("conargs", Some(args_ty), Some(copy_args)));
    // Haskell invokes its constructor init helper indirectly with the product. Rust's frontend can
    // express the same operation without dictionary evidence: destructure a
    // multi-argument product, then make an ordinary direct call.
    match constructor.params.len() {
        0 => {
            let invoke = builder.call_ident(CONSTRUCTOR_INIT_NAME, Vec::new());
            top_level_stmts.push(builder.alloc_stmt(StmtKind::Expr(invoke)));
        }
        1 => {
            let conargs = builder.ident("conargs");
            let invoke = builder.call_ident(CONSTRUCTOR_INIT_NAME, vec![conargs]);
            top_level_stmts.push(builder.alloc_stmt(StmtKind::Expr(invoke)));
        }
        len => {
            let names = (0..len)
                .map(|index| format!("conarg{index}"))
                .collect::<Vec<_>>();
            let pat = builder.product_pat(&names);
            let args = names.iter().map(|name| builder.ident(name)).collect();
            let invoke = builder.call_ident(CONSTRUCTOR_INIT_NAME, args);
            let invoke = builder.alloc_stmt(StmtKind::Expr(invoke));
            let conargs = builder.ident("conargs");
            top_level_stmts.push(builder.alloc_stmt(StmtKind::Match {
                scrutinees: vec![conargs],
                arms: vec![MatchArm {
                    span: constructor.span,
                    pats: vec![pat],
                    body: vec![invoke],
                }],
            }));
        }
    }
    top_level_stmts.push(builder.alloc_stmt(StmtKind::Assembly {
        body: return_runtime_object_yul(db, constructor.span, contract_name),
    }));
    let (stmts, exprs, pats) = builder.finish();
    let body = FuncBody::new(
        db,
        body_def,
        constructor.span,
        top_level_stmts,
        stmts,
        exprs,
        pats,
    );
    let sig = FuncSig {
        span: constructor.span,
        type_vars: Vec::new(),
        preds: Vec::new(),
        public: None,
        payable: None,
        name: spanned_ident(db, constructor.span, DEPLOYMENT_MAIN_NAME),
        params: SpannedElem::new(Vec::new(), constructor.span),
        ret: Some(unit_ty(db, constructor.span)),
    };
    let function = FunctionDef::new(
        db,
        function_def,
        constructor.span,
        FuncKind::Function,
        Vec::new(),
        sig,
        Some(body),
    );
    (
        function,
        generated_function_origins(
            function_def,
            body_def,
            GeneratedOriginKind::ContractDeploymentMain,
            contract,
            constructor.def,
            constructor.span,
        ),
    )
}

fn generated_function_origins<'db>(
    function: DefId<'db>,
    body: DefId<'db>,
    kind: GeneratedOriginKind,
    contract: DefId<'db>,
    source: Option<DefId<'db>>,
    span: Span<'db>,
) -> Vec<GeneratedOrigin<'db>> {
    vec![
        GeneratedOrigin {
            def: function,
            kind,
            contract,
            method: source,
            span,
        },
        GeneratedOrigin {
            def: body,
            kind,
            contract,
            method: source,
            span,
        },
    ]
}

fn clone_body_with_def<'db>(
    db: &'db dyn Db,
    source: FuncBody<'db>,
    def: DefId<'db>,
) -> FuncBody<'db> {
    FuncBody::new(
        db,
        def,
        source.span(db),
        source.top_level_stmts(db).clone(),
        source.stmts(db).clone(),
        source.exprs(db).clone(),
        source.pats(db).clone(),
    )
}

fn generated_unit_body<'db>(db: &'db dyn Db, def: DefId<'db>, span: Span<'db>) -> FuncBody<'db> {
    let mut builder = BodyBuilder::new(db, span);
    let unit = builder.alloc_expr(ExprKind::Tuple(Vec::new()));
    let ret = builder.alloc_stmt(StmtKind::Return(Some(unit)));
    let (stmts, exprs, pats) = builder.finish();
    FuncBody::new(db, def, span, vec![ret], stmts, exprs, pats)
}

fn constructor_copy_yul<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    contract_name: &str,
) -> Vec<YulStmt<'db>> {
    let deployer = format!("{contract_name}Deploy");
    let program_size = yul_call(db, span, "datasize", vec![yul_string(span, &deployer)]);
    let codesize = yul_call(db, span, "codesize", Vec::new());
    let arg_size = yul_call(
        db,
        span,
        "sub",
        vec![codesize, yul_ident_expr(db, span, "programSize")],
    );
    let free_ptr = yul_call(db, span, "mload", vec![yul_number(span, "64")]);
    let unrounded_free_ptr = yul_call(
        db,
        span,
        "add",
        vec![
            yul_ident_expr(db, span, "memoryDataOffset"),
            yul_ident_expr(db, span, "argSize"),
        ],
    );
    let add_rounding = yul_call(
        db,
        span,
        "add",
        vec![unrounded_free_ptr, yul_number(span, "31")],
    );
    let mask = yul_call(db, span, "not", vec![yul_number(span, "31")]);
    let new_free_ptr = yul_call(db, span, "and", vec![add_rounding, mask]);
    let update_free_ptr = yul_call(
        db,
        span,
        "mstore",
        vec![yul_number(span, "64"), new_free_ptr],
    );
    let copy = yul_call(
        db,
        span,
        "codecopy",
        vec![
            yul_ident_expr(db, span, "memoryDataOffset"),
            yul_ident_expr(db, span, "programSize"),
            yul_ident_expr(db, span, "argSize"),
        ],
    );
    let truncated = yul_call(
        db,
        span,
        "lt",
        vec![
            yul_ident_expr(db, span, "argSize"),
            yul_ident_expr(db, span, "minimumSize"),
        ],
    );
    let store_selector = yul_call(
        db,
        span,
        "mstore",
        vec![yul_number(span, "0"), yul_hex(span, "0x08638556")],
    );
    let revert = yul_call(
        db,
        span,
        "revert",
        vec![yul_number(span, "28"), yul_number(span, "4")],
    );
    vec![
        yul_let(db, span, "programSize", Some(program_size)),
        yul_assign(db, span, "argSize", arg_size),
        yul_assign(db, span, "memoryDataOffset", free_ptr),
        YulStmt {
            span,
            kind: YulStmtKind::If {
                cond: truncated,
                body: vec![
                    yul_expr_stmt(span, store_selector),
                    yul_expr_stmt(span, revert),
                ],
            },
        },
        yul_expr_stmt(span, update_free_ptr),
        yul_expr_stmt(span, copy),
    ]
}

fn deployment_setup_yul<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    contract_name: &str,
) -> Vec<YulStmt<'db>> {
    let deployer = format!("{contract_name}Deploy");
    let memoryguard = yul_call(db, span, "memoryguard", vec![yul_number(span, "128")]);
    let initialize_memory = yul_call(
        db,
        span,
        "mstore",
        vec![yul_number(span, "64"), memoryguard],
    );
    let codesize = yul_call(db, span, "codesize", Vec::new());
    let deployer_size = yul_call(db, span, "datasize", vec![yul_string(span, &deployer)]);
    let truncated = yul_call(db, span, "lt", vec![codesize, deployer_size]);
    let revert = yul_call(
        db,
        span,
        "revert",
        vec![yul_number(span, "0"), yul_number(span, "0")],
    );
    vec![
        yul_expr_stmt(span, initialize_memory),
        YulStmt {
            span,
            kind: YulStmtKind::If {
                cond: truncated,
                body: vec![yul_expr_stmt(span, revert)],
            },
        },
    ]
}

fn nonpayable_constructor_yul<'db>(db: &'db dyn Db, span: Span<'db>) -> Vec<YulStmt<'db>> {
    let callvalue = yul_call(db, span, "callvalue", Vec::new());
    let store_selector = yul_call(
        db,
        span,
        "mstore",
        vec![yul_number(span, "0"), yul_hex(span, "0xb5988ea3")],
    );
    let revert = yul_call(
        db,
        span,
        "revert",
        vec![yul_number(span, "28"), yul_number(span, "4")],
    );
    vec![YulStmt {
        span,
        kind: YulStmtKind::If {
            cond: callvalue,
            body: vec![
                yul_expr_stmt(span, store_selector),
                yul_expr_stmt(span, revert),
            ],
        },
    }]
}

fn return_runtime_object_yul<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    contract_name: &str,
) -> Vec<YulStmt<'db>> {
    let runtime = yul_string(span, contract_name);
    let size = yul_call(db, span, "datasize", vec![runtime.clone()]);
    let offset = yul_call(db, span, "dataoffset", vec![runtime.clone()]);
    let runtime_size = yul_call(db, span, "datasize", vec![runtime]);
    let copy = yul_call(
        db,
        span,
        "codecopy",
        vec![yul_number(span, "0"), offset, runtime_size],
    );
    let ret = yul_call(
        db,
        span,
        "return",
        vec![yul_number(span, "0"), yul_ident_expr(db, span, "size")],
    );
    vec![
        yul_let(db, span, "size", Some(size)),
        yul_expr_stmt(span, copy),
        yul_expr_stmt(span, ret),
    ]
}

fn yul_ident<'db>(db: &'db dyn Db, span: Span<'db>, name: &str) -> SpannedElem<'db, Ident<'db>> {
    spanned_ident(db, span, name)
}

fn yul_ident_expr<'db>(db: &'db dyn Db, span: Span<'db>, name: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Ident(yul_ident(db, span, name)),
    }
}

fn yul_number<'db>(span: Span<'db>, value: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Lit(YulLitKind::Number(value.to_owned())),
    }
}

fn yul_hex<'db>(span: Span<'db>, value: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Lit(YulLitKind::Hex(value.to_owned())),
    }
}

fn yul_string<'db>(span: Span<'db>, value: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Lit(YulLitKind::String(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))),
    }
}

fn yul_call<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
    args: Vec<YulExpr<'db>>,
) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Call {
            name: yul_ident(db, span, name),
            args,
        },
    }
}

fn yul_let<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
    init: Option<YulExpr<'db>>,
) -> YulStmt<'db> {
    YulStmt {
        span,
        kind: YulStmtKind::Let {
            names: vec![yul_ident(db, span, name)],
            init,
        },
    }
}

fn yul_assign<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
    value: YulExpr<'db>,
) -> YulStmt<'db> {
    YulStmt {
        span,
        kind: YulStmtKind::Assign {
            names: vec![yul_ident(db, span, name)],
            value,
        },
    }
}

fn yul_expr_stmt<'db>(span: Span<'db>, expr: YulExpr<'db>) -> YulStmt<'db> {
    YulStmt {
        span,
        kind: YulStmtKind::Expr(expr),
    }
}

#[salsa::tracked]
fn prepare_contract_dispatch<'db>(
    db: &'db dyn Db,
    module_def: DefId<'db>,
    contract: ContractDef<'db>,
) -> Option<PreparedContractArtifacts<'db>> {
    let mut methods = Vec::new();
    let mut fallback = None;
    for item in contract.items(db) {
        let ContractItem::FunctionDef(function) = *item else {
            continue;
        };
        let sig = function.sig(db);
        match function.kind(db) {
            FuncKind::Function
                if sig.public.is_some() && ident_text(db, &sig.name) != "fallback" =>
            {
                methods.push(RawMethod {
                    def: function.def_id_value(db),
                    name: ident_text(db, &sig.name),
                    span: sig.span,
                    payable: sig.payable.is_some(),
                    params: explicit_param_types(sig.params.atom())?,
                    ret: sig.ret.unwrap_or_else(|| unit_ty(db, sig.span)),
                });
            }
            FuncKind::Fallback => {
                fallback = Some(RawFallback {
                    name: ident_text(db, &sig.name),
                    payable: sig.payable.is_some(),
                    params: explicit_param_types(sig.params.atom())?,
                    ret: sig.ret.unwrap_or_else(|| unit_ty(db, sig.span)),
                });
            }
            FuncKind::Function | FuncKind::Constructor => {}
        }
    }

    let contract_def = contract.def_id_value(db);
    let contract_name = ident_text(db, &contract.name_elem(db));
    let mut top_level_items = Vec::new();
    let mut origins = Vec::new();
    let mut declared_names = BTreeSet::new();
    for method in &methods {
        if !declared_names.insert(method.name.clone()) {
            continue;
        }
        let (adt, instance, mut generated) =
            dispatch_name_declarations(db, module_def, contract_def, &contract_name, method);
        top_level_items.push(Item::AdtDef(adt));
        top_level_items.push(Item::InstanceDef(instance));
        origins.append(&mut generated);
    }

    let main = generated_dispatch_main(db, contract, &contract_name, &methods, fallback.as_ref())?;
    let main_def = main.def_id_value(db);
    origins.push(GeneratedOrigin {
        def: main_def,
        kind: GeneratedOriginKind::ContractDispatchMain,
        contract: contract_def,
        method: None,
        span: contract.name_elem(db).span(db),
    });
    if let Some(body) = main.body(db) {
        origins.push(GeneratedOrigin {
            def: body.def_id(db),
            kind: GeneratedOriginKind::ContractDispatchMain,
            contract: contract_def,
            method: None,
            span: contract.name_elem(db).span(db),
        });
    }

    let mut contract_items = contract.items(db).clone();
    contract_items.push(ContractItem::FunctionDef(main));
    let prepared_contract = ContractDef::new(
        db,
        contract_def,
        contract.span(db),
        contract.leading_comments(db).clone(),
        contract.name_elem(db),
        contract.ty_param_elems(db).clone(),
        contract.fields(db).clone(),
        contract.field_comments(db).clone(),
        contract_items,
    );
    Some(PreparedContractArtifacts {
        contract: prepared_contract,
        top_level_items,
        origins,
    })
}

fn explicit_param_types<'db>(params: &[FuncParam<'db>]) -> Option<Vec<TypeRef<'db>>> {
    params
        .iter()
        .map(|param| match param {
            FuncParam::Typed { ty, .. } => Some(*ty),
            FuncParam::Untyped { .. } | FuncParam::Error { .. } => None,
        })
        .collect()
}

fn dispatch_name_declarations<'db>(
    db: &'db dyn Db,
    module_def: DefId<'db>,
    contract: DefId<'db>,
    contract_name: &str,
    method: &RawMethod<'db>,
) -> (AdtDef<'db>, InstanceDef<'db>, Vec<GeneratedOrigin<'db>>) {
    let ty_name = format!("DispatchNameTy_{contract_name}_{}", method.name);
    let adt_def = generated_def(
        db,
        module_def,
        DefKind::Adt,
        &ty_name,
        "solcore.generated.std_dispatch.name_type",
    );
    let adt = AdtDef::new(
        db,
        adt_def,
        method.span,
        Vec::new(),
        spanned_ident(db, method.span, &ty_name),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    let instance_def = generated_def(
        db,
        module_def,
        DefKind::Instance,
        "SigString",
        &format!("solcore.generated.std_dispatch.sig_string.{ty_name}"),
    );
    let method_def = generated_def(
        db,
        instance_def,
        DefKind::Function,
        "sigStr",
        "solcore.generated.std_dispatch.sig_string.method",
    );
    let body_def = generated_def(
        db,
        method_def,
        DefKind::FuncBody,
        "sigStr",
        "solcore.generated.std_dispatch.sig_string.method.body",
    );
    let sig_string_method = sig_string_method(
        db,
        method_def,
        body_def,
        method.span,
        &ty_name,
        &method.name,
    );
    let head = PredRef::new(
        db,
        PredRefKind {
            ty: named_ty(db, method.span, &ty_name, Vec::new()),
            class: spanned_ident(db, method.span, SIG_STRING_CLASS_ALIAS),
            args: SpannedElem::new(Vec::new(), method.span),
        },
    );
    let instance = InstanceDef::new(
        db,
        instance_def,
        method.span,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        head,
        vec![sig_string_method],
    );
    let origins = vec![
        GeneratedOrigin {
            def: adt_def,
            kind: GeneratedOriginKind::ContractDispatchNameType,
            contract,
            method: Some(method.def),
            span: method.span,
        },
        GeneratedOrigin {
            def: instance_def,
            kind: GeneratedOriginKind::ContractDispatchSigStringInstance,
            contract,
            method: Some(method.def),
            span: method.span,
        },
        GeneratedOrigin {
            def: method_def,
            kind: GeneratedOriginKind::ContractDispatchSigStringMethod,
            contract,
            method: Some(method.def),
            span: method.span,
        },
        GeneratedOrigin {
            def: body_def,
            kind: GeneratedOriginKind::ContractDispatchSigStringMethod,
            contract,
            method: Some(method.def),
            span: method.span,
        },
    ];
    (adt, instance, origins)
}

fn sig_string_method<'db>(
    db: &'db dyn Db,
    method_def: DefId<'db>,
    body_def: DefId<'db>,
    span: Span<'db>,
    ty_name: &str,
    method_name: &str,
) -> FunctionDef<'db> {
    let mut exprs = Arena::new();
    let value = exprs.alloc(Expr {
        span,
        kind: ExprKind::Lit(LitKind::String(format!("\"{method_name}\""))),
    });
    let mut stmts = Arena::new();
    let ret = stmts.alloc(Stmt {
        span,
        kind: StmtKind::Return(Some(value)),
    });
    let body = FuncBody::new(db, body_def, span, vec![ret], stmts, exprs, Arena::new());
    let proxy_ty = qualified_named_ty(
        db,
        span,
        STD_MODULE_ALIAS,
        "Proxy",
        vec![named_ty(db, span, ty_name, Vec::new())],
    );
    let sig = FuncSig {
        span,
        type_vars: Vec::new(),
        preds: Vec::new(),
        public: None,
        payable: None,
        name: spanned_ident(db, span, "sigStr"),
        params: SpannedElem::new(
            vec![FuncParam::Typed {
                comptime: None,
                name: spanned_ident(db, span, "p"),
                ty: proxy_ty,
            }],
            span,
        ),
        ret: Some(qualified_named_ty(
            db,
            span,
            STD_MODULE_ALIAS,
            "string",
            Vec::new(),
        )),
    };
    FunctionDef::new(
        db,
        method_def,
        span,
        FuncKind::Function,
        Vec::new(),
        sig,
        Some(body),
    )
}

fn generated_dispatch_main<'db>(
    db: &'db dyn Db,
    contract: ContractDef<'db>,
    contract_name: &str,
    methods: &[RawMethod<'db>],
    fallback: Option<&RawFallback<'db>>,
) -> Option<FunctionDef<'db>> {
    let span = contract.name_elem(db).span(db);
    let mut builder = BodyBuilder::new(db, span);
    let mut method_values = Vec::with_capacity(methods.len());
    for method in methods {
        let name_ty = format!("DispatchNameTy_{contract_name}_{}", method.name);
        let name = builder.qualified_proxy(
            STD_MODULE_ALIAS,
            named_ty(db, method.span, &name_ty, Vec::new()),
        );
        let payability = builder.qualified_proxy(
            STD_MODULE_ALIAS,
            qualified_named_ty(
                db,
                method.span,
                STD_DISPATCH_MODULE_ALIAS,
                if method.payable {
                    "Payable"
                } else {
                    "NonPayable"
                },
                Vec::new(),
            ),
        );
        let args_ty = product_ty(db, method.span, &method.params);
        let args = builder.qualified_proxy(STD_MODULE_ALIAS, args_ty);
        let rets = builder.qualified_proxy(STD_MODULE_ALIAS, method.ret);
        let implementation = builder.ident(&method.name);
        method_values.push(builder.call_path(
            &[STD_DISPATCH_MODULE_ALIAS, "Method", "Method"],
            vec![name, payability, args, rets, implementation],
        ));
    }
    let methods = builder.product_expr(&method_values);
    let fallback = match fallback {
        Some(fallback) => {
            let payability = builder.qualified_proxy(
                STD_MODULE_ALIAS,
                qualified_named_ty(
                    db,
                    span,
                    STD_DISPATCH_MODULE_ALIAS,
                    if fallback.payable {
                        "Payable"
                    } else {
                        "NonPayable"
                    },
                    Vec::new(),
                ),
            );
            let args_ty = product_ty(db, span, &fallback.params);
            let args = builder.qualified_proxy(STD_MODULE_ALIAS, args_ty);
            let rets = builder.qualified_proxy(STD_MODULE_ALIAS, fallback.ret);
            let implementation = builder.ident(&fallback.name);
            builder.call_path(
                &[STD_DISPATCH_MODULE_ALIAS, "Fallback", "Fallback"],
                vec![payability, args, rets, implementation],
            )
        }
        None => {
            let payability = builder.qualified_proxy(
                STD_MODULE_ALIAS,
                qualified_named_ty(
                    db,
                    span,
                    STD_DISPATCH_MODULE_ALIAS,
                    "NonPayable",
                    Vec::new(),
                ),
            );
            let args = builder.qualified_proxy(STD_MODULE_ALIAS, unit_ty(db, span));
            let rets = builder.qualified_proxy(STD_MODULE_ALIAS, unit_ty(db, span));
            let implementation =
                builder.path(&[STD_DISPATCH_MODULE_ALIAS, "fallback_default_implementation"]);
            builder.call_path(
                &[STD_DISPATCH_MODULE_ALIAS, "Fallback", "Fallback"],
                vec![payability, args, rets, implementation],
            )
        }
    };
    let contract_value = builder.call_path(
        &[STD_DISPATCH_MODULE_ALIAS, "Contract", "Contract"],
        vec![methods, fallback],
    );
    let run = builder.call_path(
        &[STD_DISPATCH_MODULE_ALIAS, "RunContract", "exec"],
        vec![contract_value],
    );
    let run_stmt = builder.alloc_stmt(StmtKind::Expr(run));
    let unit = builder.alloc_expr(ExprKind::Tuple(Vec::new()));
    let return_stmt = builder.alloc_stmt(StmtKind::Return(Some(unit)));

    let contract_def = contract.def_id_value(db);
    let function_def = generated_def(
        db,
        contract_def,
        DefKind::Function,
        GENERATED_MAIN_NAME,
        MAIN_FINGERPRINT,
    );
    let body_def = generated_def(
        db,
        function_def,
        DefKind::FuncBody,
        GENERATED_MAIN_NAME,
        MAIN_BODY_FINGERPRINT,
    );
    let (stmts, exprs, pats) = builder.finish();
    let body = FuncBody::new(
        db,
        body_def,
        span,
        vec![run_stmt, return_stmt],
        stmts,
        exprs,
        pats,
    );
    let sig = FuncSig {
        span,
        type_vars: Vec::new(),
        preds: Vec::new(),
        public: None,
        payable: None,
        name: spanned_ident(db, span, GENERATED_MAIN_NAME),
        params: SpannedElem::new(Vec::new(), span),
        ret: Some(unit_ty(db, span)),
    };
    Some(FunctionDef::new(
        db,
        function_def,
        span,
        FuncKind::Function,
        Vec::new(),
        sig,
        Some(body),
    ))
}

struct BodyBuilder<'db> {
    db: &'db dyn Db,
    span: Span<'db>,
    stmts: Arena<Stmt<'db>>,
    exprs: Arena<Expr<'db>>,
    pats: Arena<Pat<'db>>,
}

impl<'db> BodyBuilder<'db> {
    fn new(db: &'db dyn Db, span: Span<'db>) -> Self {
        Self {
            db,
            span,
            stmts: Arena::new(),
            exprs: Arena::new(),
            pats: Arena::new(),
        }
    }

    fn finish(self) -> (Arena<Stmt<'db>>, Arena<Expr<'db>>, Arena<Pat<'db>>) {
        (self.stmts, self.exprs, self.pats)
    }

    fn alloc_stmt(&mut self, kind: StmtKind<'db>) -> Id<Stmt<'db>> {
        self.stmts.alloc(Stmt {
            span: self.span,
            kind,
        })
    }

    fn alloc_expr(&mut self, kind: ExprKind<'db>) -> Id<Expr<'db>> {
        self.exprs.alloc(Expr {
            span: self.span,
            kind,
        })
    }

    fn alloc_pat(&mut self, kind: PatKind<'db>) -> Id<Pat<'db>> {
        self.pats.alloc(Pat {
            span: self.span,
            kind,
        })
    }

    fn ident(&mut self, name: &str) -> Id<Expr<'db>> {
        let ident = spanned_ident(self.db, self.span, name);
        self.alloc_expr(ExprKind::Ident(ident))
    }

    fn path(&mut self, segments: &[&str]) -> Id<Expr<'db>> {
        let (first, rest) = segments
            .split_first()
            .expect("compiler-generated paths are non-empty");
        let mut expr = self.ident(first);
        for segment in rest {
            expr = self.alloc_expr(ExprKind::Field {
                base: expr,
                field: spanned_ident(self.db, self.span, segment),
            });
        }
        expr
    }

    fn call(&mut self, callee: Id<Expr<'db>>, args: Vec<Id<Expr<'db>>>) -> Id<Expr<'db>> {
        self.alloc_expr(ExprKind::Call { callee, args })
    }

    fn call_ident(&mut self, name: &str, args: Vec<Id<Expr<'db>>>) -> Id<Expr<'db>> {
        let callee = self.ident(name);
        self.call(callee, args)
    }

    fn call_path(&mut self, path: &[&str], args: Vec<Id<Expr<'db>>>) -> Id<Expr<'db>> {
        let callee = self.path(path);
        self.call(callee, args)
    }

    fn let_stmt(
        &mut self,
        name: &str,
        ty: Option<TypeRef<'db>>,
        init: Option<Id<Expr<'db>>>,
    ) -> Id<Stmt<'db>> {
        self.alloc_stmt(StmtKind::Let {
            comptime: None,
            name: spanned_ident(self.db, self.span, name),
            ty,
            init,
        })
    }

    fn qualified_proxy(&mut self, qualifier: &str, ty: TypeRef<'db>) -> Id<Expr<'db>> {
        let proxy = self.path(&[qualifier, "Proxy", "Proxy"]);
        let proxy_ty = qualified_named_ty(self.db, self.span, qualifier, "Proxy", vec![ty]);
        self.alloc_expr(ExprKind::TypeAnnot {
            expr: proxy,
            ty: proxy_ty,
        })
    }

    fn product_expr(&mut self, elems: &[Id<Expr<'db>>]) -> Id<Expr<'db>> {
        match elems {
            [] => self.alloc_expr(ExprKind::Tuple(Vec::new())),
            [one] => *one,
            [head, tail @ ..] => {
                let tail = self.product_expr(tail);
                self.alloc_expr(ExprKind::Tuple(vec![*head, tail]))
            }
        }
    }

    fn product_pat(&mut self, names: &[String]) -> Id<Pat<'db>> {
        match names {
            [] => self.alloc_pat(PatKind::Tuple { elems: Vec::new() }),
            [one] => {
                let name = spanned_ident(self.db, self.span, one);
                self.alloc_pat(PatKind::Var(name))
            }
            [head, tail @ ..] => {
                let head_name = spanned_ident(self.db, self.span, head);
                let head = self.alloc_pat(PatKind::Var(head_name));
                let tail = self.product_pat(tail);
                self.alloc_pat(PatKind::Tuple {
                    elems: vec![head, tail],
                })
            }
        }
    }
}

fn generated_def<'db>(
    db: &'db dyn Db,
    owner: DefId<'db>,
    kind: DefKind,
    name: &str,
    fingerprint: &str,
) -> DefId<'db> {
    DefId::new(
        db,
        owner.file(db),
        Some(owner),
        kind,
        Some(name.to_owned()),
        Some(fingerprint.to_owned()),
        Disambiguator::ZERO,
    )
}

fn product_ty<'db>(db: &'db dyn Db, span: Span<'db>, elems: &[TypeRef<'db>]) -> TypeRef<'db> {
    match elems {
        [] => unit_ty(db, span),
        [one] => *one,
        [head, tail @ ..] => tuple_ty(db, span, vec![*head, product_ty(db, span, tail)]),
    }
}

fn unit_ty<'db>(db: &'db dyn Db, span: Span<'db>) -> TypeRef<'db> {
    tuple_ty(db, span, Vec::new())
}

fn tuple_ty<'db>(db: &'db dyn Db, span: Span<'db>, elems: Vec<TypeRef<'db>>) -> TypeRef<'db> {
    TypeRef::new(
        db,
        TypeRefKind::Tuple {
            elems: SpannedElem::new(elems, span),
        },
    )
}

fn named_ty<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
    args: Vec<TypeRef<'db>>,
) -> TypeRef<'db> {
    TypeRef::new(
        db,
        TypeRefKind::Named {
            qualifier: None,
            name: spanned_ident(db, span, name),
            args: SpannedElem::new(args, span),
        },
    )
}

fn qualified_named_ty<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    qualifier: &str,
    name: &str,
    args: Vec<TypeRef<'db>>,
) -> TypeRef<'db> {
    TypeRef::new(
        db,
        TypeRefKind::Named {
            qualifier: Some(spanned_ident(db, span, qualifier)),
            name: spanned_ident(db, span, name),
            args: SpannedElem::new(args, span),
        },
    )
}

fn spanned_ident<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(Ident::new(db, name.to_owned()), span)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use hir::{
        anchor::DefLocationTable,
        ast::item::{ContractItem, Item},
        input::SourceFile,
    };
    use nameres::{
        LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_id_from_key,
    };
    use parser::parse_file_to_hir;
    use rustc_hash::FxHashMap;
    use salsa::Setter;

    use super::*;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        module_tree: Option<ModuleTree>,
        module_fs_snapshot: Option<ModuleFsSnapshot>,
        module_files: FxHashMap<ModuleKey, SourceFile>,
        executed: Arc<Mutex<Vec<String>>>,
    }

    impl Default for TestDb {
        fn default() -> Self {
            let executed = Arc::new(Mutex::new(Vec::new()));
            Self {
                storage: salsa::Storage::new(Some(Box::new({
                    let executed = executed.clone();
                    move |event| {
                        if let salsa::EventKind::WillExecute { database_key } = event.kind {
                            executed
                                .lock()
                                .expect("execution log lock")
                                .push(format!("{database_key:?}"));
                        }
                    }
                }))),
                module_tree: None,
                module_fs_snapshot: None,
                module_files: FxHashMap::default(),
                executed,
            }
        }
    }

    impl TestDb {
        fn take_executed(&self) -> Vec<String> {
            std::mem::take(&mut *self.executed.lock().expect("execution log lock"))
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
            self.module_tree.expect("module tree")
        }

        fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
            self.module_fs_snapshot.expect("filesystem snapshot")
        }

        fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
            self.module_files.get(&module.key(self)).copied()
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {}

    fn db_with_main(src: &str) -> (TestDb, SourceFile) {
        let mut db = TestDb::default();
        let main_root = PathBuf::from("/main");
        let std_root = PathBuf::from("/std");
        db.module_tree = Some(ModuleTree::new(
            &db,
            main_root.clone(),
            std_root.clone(),
            BTreeMap::new(),
        ));
        let main_path = main_root.join("main.solc");
        let std_path = std_root.join("std.solc");
        let dispatch_path = std_root.join("dispatch.solc");
        db.module_fs_snapshot = Some(ModuleFsSnapshot::new(
            &db,
            BTreeSet::from([main_path.clone(), std_path.clone(), dispatch_path.clone()]),
            BTreeMap::from([
                (main_root, vec!["main".to_owned()]),
                (std_root, vec!["std".to_owned(), "dispatch".to_owned()]),
            ]),
        ));
        let main_file = SourceFile::new(
            &db,
            url::Url::from_file_path(&main_path).expect("main URL"),
            Some(src.to_owned()),
        );
        let dispatch_file = SourceFile::new(
            &db,
            url::Url::from_file_path(&dispatch_path).expect("dispatch URL"),
            Some(String::new()),
        );
        let std_file = SourceFile::new(
            &db,
            url::Url::from_file_path(&std_path).expect("std URL"),
            Some(String::new()),
        );
        db.module_files.insert(
            ModuleKey {
                library: LibraryId::Std,
                logical_path: vec!["std".to_owned()],
            },
            std_file,
        );
        db.module_files.insert(
            ModuleKey {
                library: LibraryId::Main,
                logical_path: vec!["main".to_owned()],
            },
            main_file,
        );
        db.module_files.insert(
            ModuleKey {
                library: LibraryId::Std,
                logical_path: vec!["dispatch".to_owned()],
            },
            dispatch_file,
        );
        let _ = module_id_from_key(
            &db,
            &ModuleKey {
                library: LibraryId::Main,
                logical_path: vec!["main".to_owned()],
            },
        );
        (db, main_file)
    }

    fn source_module<'db>(db: &'db TestDb, file: SourceFile) -> Module<'db> {
        parse_file_to_hir(db, file).module(db)
    }

    fn first_contract<'db>(db: &'db TestDb, module: Module<'db>) -> ContractDef<'db> {
        module
            .items(db)
            .iter()
            .find_map(|item| match item {
                Item::ContractDef(contract) => Some(*contract),
                _ => None,
            })
            .expect("contract")
    }

    #[test]
    fn preserves_source_and_builds_effective_dispatch_overlay() {
        let src = r#"
import std.dispatch.{*};
contract C { public function answer(x:uint256) -> uint256 { return x; } }
"#;
        let (db, file) = db_with_main(src);
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        assert_eq!(prepared.source(&db), source);
        assert_ne!(prepared.module(&db), source);
        assert_eq!(file.content(&db).as_deref(), Some(src));

        let contract = first_contract(&db, prepared.module(&db));
        let main = prepared
            .contract_dispatch_main(&db, contract.def_id_value(&db))
            .expect("generated runtime main");
        assert!(matches!(
            prepared.origin_for_def(&db, main).map(|origin| origin.kind),
            Some(GeneratedOriginKind::ContractDispatchMain)
        ));
        assert!(contract.items(&db).iter().any(|item| matches!(
            item,
            ContractItem::FunctionDef(function)
                if function.def_id_value(&db) == main
                    && ident_text(&db, &function.sig(&db).name) == GENERATED_MAIN_NAME
        )));
        assert_eq!(
            prepare_module(&db, prepared.module(&db)).module(&db),
            prepared.module(&db)
        );
    }

    #[test]
    fn preparation_preserves_contract_and_field_comments() {
        let src = r#"
import std.{*};
import std.dispatch.{*};
// contract documentation
contract C {
  // stored value documentation
  stored: word;
  // constructor documentation
  constructor() {}
  // method documentation
  public function answer(x:uint256) -> uint256 { return x; }
}
"#;
        let (db, file) = db_with_main(src);
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        let source_contract = first_contract(&db, prepared.source(&db));
        let effective_contract = first_contract(&db, prepared.module(&db));

        assert_eq!(
            trimmed_comment_texts(source_contract.leading_comments(&db)),
            ["contract documentation"]
        );
        assert_eq!(
            effective_contract.leading_comments(&db),
            source_contract.leading_comments(&db)
        );
        assert_eq!(
            effective_contract.field_comments(&db),
            source_contract.field_comments(&db)
        );

        let fields = effective_contract
            .fields_with_comments(&db)
            .collect::<Vec<_>>();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            trimmed_comment_texts(fields[0].1),
            ["stored value documentation"]
        );

        let mut generated_functions = 0;
        for item in effective_contract.items(&db) {
            let ContractItem::FunctionDef(function) = item else {
                continue;
            };
            if prepared
                .origin_for_def(&db, function.def_id_value(&db))
                .is_some()
            {
                generated_functions += 1;
                assert!(function.leading_comments(&db).is_empty());
            }
        }
        assert!(generated_functions > 0);

        let mut generated_imports = 0;
        let mut generated_adts = 0;
        let mut generated_instances = 0;
        for item in prepared.module(&db).items(&db) {
            match item {
                Item::Import(import)
                    if matches!(
                        import.def_id(&db).fingerprint(&db).as_deref(),
                        Some(
                            STD_MODULE_IMPORT_FINGERPRINT
                                | STD_DISPATCH_MODULE_IMPORT_FINGERPRINT
                                | SIG_STRING_IMPORT_FINGERPRINT
                        )
                    ) =>
                {
                    generated_imports += 1;
                    assert!(import.leading_comments(&db).is_empty());
                }
                Item::AdtDef(adt)
                    if prepared
                        .origin_for_def(&db, adt.def_id_value(&db))
                        .is_some() =>
                {
                    generated_adts += 1;
                    assert!(adt.leading_comments(&db).is_empty());
                    assert_eq!(adt.ctors(&db).len(), adt.ctor_comments(&db).len());
                    assert!(
                        adt.ctors_with_comments(&db)
                            .all(|(_, comments)| comments.is_empty())
                    );
                }
                Item::InstanceDef(instance)
                    if prepared
                        .origin_for_def(&db, instance.def_id_value(&db))
                        .is_some() =>
                {
                    generated_instances += 1;
                    assert!(instance.leading_comments(&db).is_empty());
                    assert!(
                        instance
                            .methods(&db)
                            .iter()
                            .all(|method| method.leading_comments(&db).is_empty())
                    );
                }
                _ => {}
            }
        }
        assert!(generated_imports > 0);
        assert!(generated_adts > 0);
        assert!(generated_instances > 0);
    }

    fn trimmed_comment_texts(comments: &[hir::ast::SourceComment]) -> Vec<&str> {
        comments.iter().map(|comment| comment.text.trim()).collect()
    }

    #[test]
    fn runtime_dispatch_is_implicit_and_existing_main_suppresses_it() {
        let (db, file) = db_with_main(
            "contract C { public function answer() -> uint256 { return uint256(1); } }",
        );
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        assert_ne!(prepared.module(&db), source);
        let contract = first_contract(&db, prepared.module(&db));
        assert!(
            prepared
                .contract_deployment_main(&db, contract.def_id_value(&db))
                .is_some()
        );
        assert!(
            prepared
                .contract_dispatch_main(&db, contract.def_id_value(&db))
                .is_some()
        );

        let (db, file) = db_with_main(
            r#"
import std.dispatch.{*};
contract C { function main() -> () {} }
"#,
        );
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        assert_ne!(prepared.module(&db), source);
        let contract = first_contract(&db, prepared.module(&db));
        assert!(
            prepared
                .contract_deployment_main(&db, contract.def_id_value(&db))
                .is_some()
        );
        assert!(
            prepared
                .contract_dispatch_main(&db, contract.def_id_value(&db))
                .is_none()
        );
    }

    #[test]
    fn nonempty_constructor_waits_for_canonical_std_import() {
        let (db, file) =
            db_with_main("contract C { constructor(x:word) {} function main() -> () {} }");
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        assert_eq!(prepared.module(&db), source);
        assert!(prepared.origins(&db).entries().is_empty());
    }

    #[test]
    fn constructor_overlay_preserves_source_and_generates_deployment_entry() {
        let (db, file) = db_with_main(
            r#"
import std.{*};
import std.dispatch.{*};
contract C {
  payable constructor(x:word, y:word) { let z = x; }
  function main() -> () { return (); }
}
"#,
        );
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        let source_contract = first_contract(&db, prepared.source(&db));
        assert!(source_contract.items(&db).iter().any(|item| matches!(
            item,
            ContractItem::FunctionDef(function)
                if function.kind(&db) == FuncKind::Constructor
        )));

        let effective = first_contract(&db, prepared.module(&db));
        assert!(!effective.items(&db).iter().any(|item| matches!(
            item,
            ContractItem::FunctionDef(function)
                if function.kind(&db) == FuncKind::Constructor
        )));
        let names = effective
            .items(&db)
            .iter()
            .filter_map(|item| match item {
                ContractItem::FunctionDef(function) => {
                    Some(ident_text(&db, &function.sig(&db).name))
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        assert!(names.contains(CONSTRUCTOR_INIT_NAME), "{names:?}");
        assert!(names.contains(CONSTRUCTOR_COPY_NAME), "{names:?}");
        assert!(names.contains(DEPLOYMENT_MAIN_NAME), "{names:?}");
        let deployment = prepared
            .contract_deployment_main(&db, effective.def_id_value(&db))
            .expect("generated deployment main");
        assert!(is_contract_deployment_main_def(&db, deployment));
        assert!(matches!(
            prepared
                .origin_for_def(&db, deployment)
                .map(|origin| origin.kind),
            Some(GeneratedOriginKind::ContractDeploymentMain)
        ));
        assert!(
            prepared
                .contract_dispatch_main(&db, effective.def_id_value(&db))
                .is_none()
        );
    }

    #[test]
    fn explicit_constructor_overlay_is_idempotent() {
        let (db, file) = db_with_main(
            r#"
import std.{*};
contract C {
  payable constructor(x:word) { let saved = x; }
  function main() -> () { return (); }
}
"#,
        );
        let source = source_module(&db, file);
        let first = prepare_module(&db, source).module(&db);
        let second = prepare_module(&db, first).module(&db);
        assert_eq!(second, first);

        let contract = first_contract(&db, second);
        let generated = contract
            .items(&db)
            .iter()
            .filter_map(|item| match item {
                ContractItem::FunctionDef(function) => function.def_id_value(&db).fingerprint(&db),
                _ => None,
            })
            .collect::<Vec<_>>();
        for fingerprint in [
            CONSTRUCTOR_INIT_FINGERPRINT,
            CONSTRUCTOR_COPY_FINGERPRINT,
            DEPLOYMENT_MAIN_FINGERPRINT,
        ] {
            assert_eq!(
                generated
                    .iter()
                    .filter(|candidate| candidate.as_str() == fingerprint)
                    .count(),
                1,
                "{generated:?}"
            );
        }
    }

    #[test]
    fn constructor_body_edit_keeps_generated_wrapper_identity() {
        let before = r#"
import std.{*};
import std.dispatch.{*};
contract C {
  constructor(x:word) { let z = 1; }
  function main() -> () { return (); }
}
"#;
        let after = r#"
import std.{*};
import std.dispatch.{*};
contract C {
  constructor(x:word) { let z = 2; }
  function main() -> () { return (); }
}
"#;
        let (mut db, file) = db_with_main(before);
        let before_module = prepare_module(&db, source_module(&db, file)).module(&db);
        let before_contract = first_contract(&db, before_module);
        let before_defs = constructor_wrapper_defs(&db, before_contract);

        file.set_content(&mut db).to(Some(after.to_owned()));
        let after_module = prepare_module(&db, source_module(&db, file)).module(&db);
        let after_contract = first_contract(&db, after_module);
        let after_defs = constructor_wrapper_defs(&db, after_contract);
        assert_eq!(before_defs, after_defs);
    }

    fn constructor_wrapper_defs(
        db: &TestDb,
        contract: ContractDef<'_>,
    ) -> BTreeMap<String, String> {
        contract
            .items(db)
            .iter()
            .filter_map(|item| match item {
                ContractItem::FunctionDef(function) => {
                    let name = ident_text(db, &function.sig(db).name);
                    [
                        CONSTRUCTOR_INIT_NAME,
                        CONSTRUCTOR_COPY_NAME,
                        DEPLOYMENT_MAIN_NAME,
                    ]
                    .contains(&name.as_str())
                    .then(|| (name, format!("{:?}", function.def_id_value(db))))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn deduplicates_overloaded_method_name_declarations() {
        let (db, file) = db_with_main(
            r#"
import std.dispatch.{*};
contract C {
  public function get(x:uint256) -> uint256 { return x; }
  public function get(x:bool) -> bool { return x; }
}
"#,
        );
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        let generated_name_types = prepared
            .module(&db)
            .items(&db)
            .iter()
            .filter(|item| matches!(item, Item::AdtDef(adt) if ident_text(&db, &adt.name_elem(&db)) == "DispatchNameTy_C_get"))
            .count();
        assert_eq!(generated_name_types, 1);
    }

    #[test]
    fn omitted_return_uses_unit_and_body_edit_keeps_generated_identity() {
        let before = r#"
import std.dispatch.{*};
contract C { public function ping() { let x = 1; } }
"#;
        let after = r#"
import std.dispatch.{*};
contract C { public function ping() { let x = 2; } }
"#;
        let (mut db, file) = db_with_main(before);
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        let contract = first_contract(&db, prepared.module(&db));
        let before_main = prepared
            .contract_dispatch_main(&db, contract.def_id_value(&db))
            .expect("generated main");
        let generated_main = contract
            .items(&db)
            .iter()
            .find_map(|item| match item {
                ContractItem::FunctionDef(function)
                    if function.def_id_value(&db) == before_main =>
                {
                    Some(*function)
                }
                _ => None,
            })
            .expect("generated main function");
        let body = generated_main.body(&db).expect("generated main body");
        let unit_proxy_count = body
            .exprs(&db)
            .iter()
            .filter(|(_, expr)| {
                let ExprKind::TypeAnnot { ty, .. } = expr.kind else {
                    return false;
                };
                let TypeRefKind::Named { name, args, .. } = ty.kind(&db) else {
                    return false;
                };
                if ident_text(&db, name) != "Proxy" {
                    return false;
                }
                matches!(
                    args.atom().as_slice(),
                    [arg]
                        if matches!(
                            arg.kind(&db),
                            TypeRefKind::Tuple { elems } if elems.atom().is_empty()
                        )
                )
            })
            .count();
        // ping's empty argument product and omitted return, plus the default
        // fallback's empty argument and return products.
        assert_eq!(unit_proxy_count, 4);
        let before_main = format!("{before_main:?}");
        let _ = db.take_executed();

        file.set_content(&mut db).to(Some(after.to_owned()));
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        let contract = first_contract(&db, prepared.module(&db));
        let after_main = prepared
            .contract_dispatch_main(&db, contract.def_id_value(&db))
            .expect("generated main");
        assert_eq!(before_main, format!("{after_main:?}"));
        let executed = db.take_executed();
        assert!(
            executed
                .iter()
                .any(|event| event.contains("parse_file_to_hir")),
            "body edit must exercise a new parse revision: {executed:#?}"
        );
        assert_eq!(
            executed
                .iter()
                .filter(|event| event.contains("prepare_module"))
                .count(),
            0,
            "body-only edits must backdate module preparation: {executed:#?}"
        );
        assert_eq!(
            executed
                .iter()
                .filter(|event| event.contains("prepare_contract_dispatch"))
                .count(),
            0,
            "body-only edits must backdate the generated contract overlay: {executed:#?}"
        );
    }
}
