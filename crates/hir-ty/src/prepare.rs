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
//! a small body edit does not rebuild a whole module. Constructor wrappers,
//! derived instances, and semantic field/call hooks can migrate here in later
//! stages once their required resolution inputs are one-way dependencies.

use std::collections::BTreeSet;

use hir::{
    anchor::{DefId, DefKind, Disambiguator},
    arena::{Arena, Id},
    ast::{
        Ident,
        function::{Expr, ExprKind, FuncBody, FuncParam, FuncSig, LitKind, Stmt, StmtKind},
        item::{
            AdtDef, ContractDef, ContractItem, FuncKind, FunctionDef, InstanceDef, Item, Module,
        },
        ty::{PredRef, PredRefKind, TypeRef, TypeRefKind},
    },
    nameres::ident_text,
    span::{Span, Spanned, SpannedElem},
};

use crate::{Db, contract_needs_generated_dispatch, module_has_canonical_std_dispatch_import};

const GENERATED_MAIN_NAME: &str = "main";
const MAIN_FINGERPRINT: &str = "solcore.generated.std_dispatch.main";
const MAIN_BODY_FINGERPRINT: &str = "solcore.generated.std_dispatch.main.body";

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
}

/// Returns whether `def` is a compiler-owned std.dispatch runtime entry.
///
/// The fingerprint fallback keeps provenance available to consumers that are
/// handed an already-prepared [`Module`] without its [`PreparedModule`]
/// wrapper. Source-lowered functions never receive this reserved fingerprint.
pub fn is_contract_dispatch_main_def(db: &dyn Db, def: DefId<'_>) -> bool {
    def.kind(db) == DefKind::Function && def.fingerprint(db).as_deref() == Some(MAIN_FINGERPRINT)
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

    fn extend(&mut self, origins: impl IntoIterator<Item = GeneratedOrigin<'db>>) {
        self.entries.extend(origins);
    }
}

/// Builds the pre-typecheck HIR overlay for `source`.
///
/// This query is deliberately source-only: it inspects explicit function
/// signatures and the canonical import, but never calls body inference,
/// module type checking, or specialization.  That one-way dependency avoids
/// `typeck -> prepare -> typeck` query cycles. Imports remain source-owned;
/// preparation therefore requires the explicit canonical `std.dispatch`
/// import instead of synthesizing a hidden dependency.
#[salsa::tracked]
pub fn prepare_module<'db>(db: &'db dyn Db, source: Module<'db>) -> PreparedModule<'db> {
    if !module_has_canonical_std_dispatch_import(db, source) {
        return PreparedModule::new(db, source, source, GeneratedOriginMap::default());
    }

    let module_def = source.def_id_value(db);
    let mut generated_items = Vec::new();
    let mut prepared_source_items = Vec::with_capacity(source.items(db).len());
    let mut origins = GeneratedOriginMap::default();

    for item in source.items(db) {
        let Item::ContractDef(contract) = *item else {
            prepared_source_items.push(*item);
            continue;
        };
        if !contract_needs_generated_dispatch(db, contract) {
            prepared_source_items.push(*item);
            continue;
        }
        let Some(artifacts) = prepare_contract_dispatch(db, module_def, contract) else {
            prepared_source_items.push(*item);
            continue;
        };
        generated_items.extend(artifacts.top_level_items.iter().copied());
        origins.extend(artifacts.origins.iter().cloned());
        prepared_source_items.push(Item::ContractDef(artifacts.contract));
    }

    if origins.entries.is_empty() {
        return PreparedModule::new(db, source, source, origins);
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
        contract.name_elem(db),
        contract.ty_param_elems(db).clone(),
        contract.fields(db).clone(),
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
        spanned_ident(db, method.span, &ty_name),
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
            class: spanned_ident(db, method.span, "SigString"),
            args: SpannedElem::new(Vec::new(), method.span),
        },
    );
    let instance = InstanceDef::new(
        db,
        instance_def,
        method.span,
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
    let proxy_ty = named_ty(
        db,
        span,
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
        ret: Some(named_ty(db, span, "string", Vec::new())),
    };
    FunctionDef::new(db, method_def, span, FuncKind::Function, sig, Some(body))
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
        let name = builder.proxy(named_ty(db, method.span, &name_ty, Vec::new()));
        let payability = builder.proxy(named_ty(
            db,
            method.span,
            if method.payable {
                "Payable"
            } else {
                "NonPayable"
            },
            Vec::new(),
        ));
        let args_ty = product_ty(db, method.span, &method.params);
        let args = builder.proxy(args_ty);
        let rets = builder.proxy(method.ret);
        let implementation = builder.ident(&method.name);
        method_values
            .push(builder.call_ident("Method", vec![name, payability, args, rets, implementation]));
    }
    let methods = builder.product_expr(&method_values);
    let fallback = match fallback {
        Some(fallback) => {
            let payability = builder.proxy(named_ty(
                db,
                span,
                if fallback.payable {
                    "Payable"
                } else {
                    "NonPayable"
                },
                Vec::new(),
            ));
            let args_ty = product_ty(db, span, &fallback.params);
            let args = builder.proxy(args_ty);
            let rets = builder.proxy(fallback.ret);
            let implementation = builder.ident(&fallback.name);
            builder.call_ident("Fallback", vec![payability, args, rets, implementation])
        }
        None => {
            let payability = builder.proxy(named_ty(db, span, "NonPayable", Vec::new()));
            let args = builder.proxy(unit_ty(db, span));
            let rets = builder.proxy(unit_ty(db, span));
            let implementation = builder.ident("fallback_default_implementation");
            builder.call_ident("Fallback", vec![payability, args, rets, implementation])
        }
    };
    let contract_value = builder.call_ident("Contract", vec![methods, fallback]);
    let run = builder.class_method_call("RunContract", "exec", vec![contract_value]);
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
    let (stmts, exprs) = builder.finish();
    let body = FuncBody::new(
        db,
        body_def,
        span,
        vec![run_stmt, return_stmt],
        stmts,
        exprs,
        Arena::new(),
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
        sig,
        Some(body),
    ))
}

struct BodyBuilder<'db> {
    db: &'db dyn Db,
    span: Span<'db>,
    stmts: Arena<Stmt<'db>>,
    exprs: Arena<Expr<'db>>,
}

impl<'db> BodyBuilder<'db> {
    fn new(db: &'db dyn Db, span: Span<'db>) -> Self {
        Self {
            db,
            span,
            stmts: Arena::new(),
            exprs: Arena::new(),
        }
    }

    fn finish(self) -> (Arena<Stmt<'db>>, Arena<Expr<'db>>) {
        (self.stmts, self.exprs)
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

    fn ident(&mut self, name: &str) -> Id<Expr<'db>> {
        let ident = spanned_ident(self.db, self.span, name);
        self.alloc_expr(ExprKind::Ident(ident))
    }

    fn call(&mut self, callee: Id<Expr<'db>>, args: Vec<Id<Expr<'db>>>) -> Id<Expr<'db>> {
        self.alloc_expr(ExprKind::Call { callee, args })
    }

    fn call_ident(&mut self, name: &str, args: Vec<Id<Expr<'db>>>) -> Id<Expr<'db>> {
        let callee = self.ident(name);
        self.call(callee, args)
    }

    fn class_method_call(
        &mut self,
        class: &str,
        method: &str,
        args: Vec<Id<Expr<'db>>>,
    ) -> Id<Expr<'db>> {
        let base = self.ident(class);
        let callee = self.alloc_expr(ExprKind::Field {
            base,
            field: spanned_ident(self.db, self.span, method),
        });
        self.call(callee, args)
    }

    fn proxy(&mut self, ty: TypeRef<'db>) -> Id<Expr<'db>> {
        let proxy = self.ident("Proxy");
        let proxy_ty = named_ty(self.db, self.span, "Proxy", vec![ty]);
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
        let dispatch_path = std_root.join("dispatch.solc");
        db.module_fs_snapshot = Some(ModuleFsSnapshot::new(
            &db,
            BTreeSet::from([main_path.clone(), dispatch_path.clone()]),
            BTreeMap::from([
                (main_root, vec!["main".to_owned()]),
                (std_root, vec!["dispatch".to_owned()]),
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
                if function.def_id_value(&db) == main && ident_text(&db, &function.sig(&db).name) == "main"
        )));
    }

    #[test]
    fn requires_canonical_import_and_suppresses_existing_main() {
        let (db, file) = db_with_main(
            "contract C { public function answer() -> uint256 { return uint256(1); } }",
        );
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        assert_eq!(prepared.module(&db), source);
        assert!(prepared.origins(&db).entries().is_empty());

        let (db, file) = db_with_main(
            r#"
import std.dispatch.{*};
contract C { function main() -> () {} }
"#,
        );
        let source = source_module(&db, file);
        let prepared = prepare_module(&db, source);
        assert_eq!(prepared.module(&db), source);
        assert!(prepared.origins(&db).entries().is_empty());
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
