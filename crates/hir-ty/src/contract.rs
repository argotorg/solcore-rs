//! Contract-specific typed surfaces and frontend desugar planning.
//!
//! This module intentionally lives in `hir-ty`, not a new `hir-lower` crate:
//! dispatch eligibility, ABI spelling, duplicate public signatures, and field
//! initializer checks all need resolved names and lowered semantic types. The
//! later Hull/codegen stages can consume the typed surface and storage hooks
//! without re-deriving frontend rules from raw HIR.

use std::fmt::Write as _;

use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{Expr, ExprKind, FuncBody, FuncParam, Pat, PatKind, Stmt, StmtKind},
        item::{ContractDef, ContractItem, FuncKind, FunctionDef, Item, Module},
    },
    diag::Diagnostic,
    nameres as hir_nameres,
    span::SpannedElem,
};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;

use crate::{
    AliasNormalizer, BinderEnv, BodyTyContext, BuiltinTyCtor, CallSiteCallee, CallSiteEvidence, Db,
    LoweredFunction, Ty, TyCtor, TyKind, TypeLowering, infer_body,
    trait_env_from_module_resolution, trait_env_with_givens,
};

const PLACEHOLDER_SELECTOR: &str = "<keccak256[0..4] pending>";

/// Typed dispatch/ABI surface for one contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DispatchSurface<'db> {
    /// Owning contract definition.
    pub contract: DefId<'db>,
    /// Contract name.
    pub name: String,
    /// Public methods eligible for selector dispatch.
    pub methods: Vec<DispatchMethod<'db>>,
    /// Constructor entry. A missing source constructor is represented as an
    /// implicit non-payable unit constructor.
    pub constructor: DispatchConstructor,
    /// Fallback entry. A missing source fallback is represented as the default
    /// non-payable unit fallback.
    pub fallback: DispatchFallback<'db>,
    /// Diagnostics produced while building the surface.
    pub diagnostics: Vec<Diagnostic>,
}

/// One public method in the dispatch surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DispatchMethod<'db> {
    /// Function definition.
    pub def: DefId<'db>,
    /// Source declaration index within the contract.
    pub source_index: usize,
    /// Source method name.
    pub name: String,
    /// Whether the method is payable.
    pub payable: bool,
    /// ABI selector preimage, e.g. `transfer(address,uint256)`.
    pub signature: String,
    /// Placeholder until the comptime keccak phase computes the first four
    /// bytes.
    pub selector: String,
    /// ABI input parameters.
    pub inputs: Vec<AbiParam>,
    /// ABI output parameters.
    pub outputs: Vec<AbiParam>,
}

/// Constructor dispatch/ABI entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DispatchConstructor {
    /// Whether the constructor was present in source.
    pub explicit: bool,
    /// Source declaration index within the contract, when explicit.
    pub source_index: Option<usize>,
    /// Whether deployment may receive value.
    pub payable: bool,
    /// ABI input parameters.
    pub inputs: Vec<AbiParam>,
}

/// Fallback dispatch/ABI entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DispatchFallback<'db> {
    /// Source fallback definition, when present.
    pub def: Option<DefId<'db>>,
    /// Whether the fallback was present in source.
    pub explicit: bool,
    /// Source declaration index within the contract, when explicit.
    pub source_index: Option<usize>,
    /// Whether fallback calls may receive value.
    pub payable: bool,
    /// ABI input parameters. Valid Solcore fallbacks are unit.
    pub inputs: Vec<AbiParam>,
    /// ABI output parameters. Valid Solcore fallbacks are unit.
    pub outputs: Vec<AbiParam>,
}

/// ABI parameter or tuple component.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AbiParam {
    /// Parameter name. Outputs and tuple components use the empty name, matching
    /// the reference ABI emitter.
    pub name: String,
    /// Canonical ABI type string.
    pub ty: String,
    /// Tuple components, if `ty == "tuple"`.
    pub components: Vec<AbiParam>,
}

/// Tracked frontend-desugar plan for one module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FrontendDesugarPlan<'db> {
    /// Per-body transform plan entries.
    pub bodies: Vec<BodyDesugarPlan<'db>>,
}

/// Transform plan for one function body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyDesugarPlan<'db> {
    /// Function/method definition.
    pub function: DefId<'db>,
    /// Human-readable function name.
    pub function_name: String,
    /// HIR-to-HIR rewrites and storage hooks in traversal order.
    pub transforms: Vec<FrontendTransform<'db>>,
}

/// One planned frontend rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FrontendTransform<'db> {
    /// `if` statement rewritten to a two-arm match on desugared bool.
    IfStmtToMatch {
        /// Body containing the statement.
        body: FuncBody<'db>,
        /// Statement being rewritten.
        stmt: Id<Stmt<'db>>,
    },
    /// `if ... then ... else ...` expression rewritten through the same
    /// true/false match scheme.
    IfExprToMatch {
        /// Body containing the expression.
        body: FuncBody<'db>,
        /// Expression being rewritten.
        expr: Id<Expr<'db>>,
    },
    /// Bool constructor or pattern rewritten to `inr(())` or `inl(())`.
    BoolToUnitSum {
        /// Body containing the node.
        body: FuncBody<'db>,
        /// Node category.
        node: BoolNode<'db>,
        /// Source constructor/pattern name.
        source: String,
        /// Replacement constructor.
        replacement: String,
    },
    /// Contract field read rewritten through an RVA storage access hook.
    FieldRead {
        /// Body containing the expression.
        body: FuncBody<'db>,
        /// Expression being rewritten.
        expr: Id<Expr<'db>>,
        /// Field identity.
        field: hir_nameres::FieldId<'db>,
        /// Generated selector type/value name.
        selector: String,
        /// Storage access hook for Hull/storage layout.
        hook: String,
    },
    /// Contract field write rewritten through an LVA/RVA assignment hook.
    FieldWrite {
        /// Body containing the statement.
        body: FuncBody<'db>,
        /// Assignment statement being rewritten.
        stmt: Id<Stmt<'db>>,
        /// Field identity.
        field: hir_nameres::FieldId<'db>,
        /// Generated selector type/value name.
        selector: String,
        /// Storage access hook for Hull/storage layout.
        hook: String,
    },
    /// Non-direct call rewritten to `invokable.invoke(callee, indirectArgs(args))`.
    IndirectCall {
        /// Body containing the call.
        body: FuncBody<'db>,
        /// Call expression being rewritten.
        call_expr: Id<Expr<'db>>,
        /// Expression used as the callee.
        callee_expr: Id<Expr<'db>>,
        /// Callee identity used for evidence replay.
        callee: CallSiteCallee<'db>,
        /// Unit, single-argument, or right-nested pair payload shape.
        args: IndirectArgShape<'db>,
        /// Solved call-site evidence for the invokable obligation.
        evidence: Option<CallSiteEvidence<'db>>,
    },
}

/// Category of bool node in a frontend transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BoolNode<'db> {
    /// Expression constructor.
    Expr(Id<Expr<'db>>),
    /// Pattern constructor.
    Pat(Id<Pat<'db>>),
}

/// Payload shape for an indirect-call argument tuple.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum IndirectArgShape<'db> {
    /// No arguments, represented as unit.
    Unit,
    /// One argument, represented without a pair wrapper.
    Single(Id<Expr<'db>>),
    /// Two or more arguments, represented as a right-nested `pair`.
    Pair {
        /// First argument at this level.
        head: Id<Expr<'db>>,
        /// Remaining argument payload.
        tail: Box<IndirectArgShape<'db>>,
    },
}

/// Returns the typed dispatch surface for one contract in `module`.
pub fn contract_dispatch_surface<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    contract: ContractDef<'db>,
) -> DispatchSurface<'db> {
    let _ = module;
    contract_dispatch_surface_by_def(db, contract.def_id_value(db))
}

#[salsa::tracked]
fn contract_dispatch_surface_by_def<'db>(
    db: &'db dyn Db,
    contract_def: DefId<'db>,
) -> DispatchSurface<'db> {
    let module = parse_file_to_hir(db, contract_def.file(db)).module(db);
    let Some(contract) = find_contract_by_def(db, module, contract_def) else {
        return DispatchSurface {
            contract: contract_def,
            name: contract_def
                .name(db)
                .unwrap_or_else(|| "Contract".to_owned()),
            methods: Vec::new(),
            constructor: DispatchConstructor {
                explicit: false,
                payable: false,
                inputs: Vec::new(),
                source_index: None,
            },
            fallback: DispatchFallback {
                def: None,
                explicit: false,
                payable: false,
                inputs: Vec::new(),
                outputs: Vec::new(),
                source_index: None,
            },
            diagnostics: Vec::new(),
        };
    };
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    contract_dispatch_surface_with_resolutions(db, module, &item_resolutions, contract)
}

/// Returns diagnostics for every contract dispatch surface in a module.
pub fn module_contract_diagnostics<'db>(db: &'db dyn Db, module: Module<'db>) -> Vec<Diagnostic> {
    module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .flat_map(|contract| {
            let dispatch_generated = contract_generates_dispatch(db, contract);
            contract_dispatch_surface(db, module, contract)
                .diagnostics
                .into_iter()
                .filter(move |diagnostic| {
                    diagnostic.code.as_deref() != Some("SC0231") || dispatch_generated
                })
        })
        .filter(|diagnostic| {
            matches!(
                diagnostic.code.as_deref(),
                Some("SC0230" | "SC0231" | "SC0232" | "SC0233")
            )
        })
        .collect()
}

/// Returns a tracked frontend-desugar plan for if/bool and contract field
/// access rewrites in `module`.
#[salsa::tracked]
pub fn frontend_desugar_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> FrontendDesugarPlan<'db> {
    let resolution = hir_nameres::resolve_module(db, module);
    let mut bodies = Vec::new();
    for item in module.items(db) {
        collect_desugar_plans(db, module, *item, &resolution, &[], &mut bodies);
    }
    FrontendDesugarPlan { bodies }
}

/// Renders an ABI JSON document mirroring the reference `contractAbiJson`
/// behavior: explicit constructors and user-defined fallbacks are included,
/// while the implicit runtime defaults remain a dispatch-surface detail.
pub fn contract_abi_json<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    contract: ContractDef<'db>,
) -> Result<String, String> {
    let surface = contract_dispatch_surface(db, module, contract);
    let mut entries = Vec::new();
    if surface.constructor.explicit {
        entries.push((
            surface.constructor.source_index.unwrap_or(usize::MAX),
            AbiJsonEntry::Constructor {
                inputs: surface.constructor.inputs,
                payable: surface.constructor.payable,
            },
        ));
    }
    for method in surface.methods {
        entries.push((
            method.source_index,
            AbiJsonEntry::Function {
                name: method.name,
                inputs: method.inputs,
                outputs: method.outputs,
                payable: method.payable,
            },
        ));
    }
    if surface.fallback.explicit {
        entries.push((
            surface.fallback.source_index.unwrap_or(usize::MAX),
            AbiJsonEntry::Fallback {
                payable: surface.fallback.payable,
            },
        ));
    }
    entries.sort_by_key(|(source_index, _)| *source_index);
    let entries = entries
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    render_abi_json(&entries)
}

fn contract_generates_dispatch<'db>(db: &'db dyn Db, contract: ContractDef<'db>) -> bool {
    !contract.items(db).iter().any(|item| {
        let ContractItem::FunctionDef(function) = item else {
            return false;
        };
        ident_text(db, &function.sig(db).name) == "main"
    })
}

fn contract_dispatch_surface_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    contract: ContractDef<'db>,
) -> DispatchSurface<'db> {
    let contract_name = ident_text(db, &contract.name_elem(db));
    let contract_type_vars =
        type_var_bindings(contract.def_id_value(db), contract.ty_param_elems(db));
    let mut diagnostics = Vec::new();
    let mut methods = Vec::new();
    let mut constructor: Option<DispatchConstructor> = None;
    let mut fallback: Option<DispatchFallback<'db>> = None;

    for (source_index, item) in contract.items(db).iter().enumerate() {
        let ContractItem::FunctionDef(function) = *item else {
            continue;
        };
        match function.kind(db) {
            FuncKind::Function => {
                let sig = function.sig(db);
                if sig.public.is_none() || ident_text(db, &sig.name) == "fallback" {
                    continue;
                }
                let type_vars =
                    function_type_vars(db, &contract_type_vars, function.def_id_value(db), sig);
                let lowered =
                    lower_normalized_function(db, module, item_resolutions, function, &type_vars);
                let param_names = param_names(db, sig.params.atom());
                let inputs = abi_params(
                    db,
                    &param_names,
                    &lowered.params,
                    &mut diagnostics,
                    sig.span,
                );
                let outputs = abi_outputs(db, lowered.ret, &mut diagnostics, sig.span);
                let signature =
                    method_signature_string(db, &ident_text(db, &sig.name), &lowered.params)
                        .unwrap_or_else(|err| {
                            diagnostics.push(contract_diag_unsupported_abi_type(
                                db,
                                sig.span,
                                &ident_text(db, &sig.name),
                                &err,
                            ));
                            format!("{}(<unsupported>)", ident_text(db, &sig.name))
                        });
                methods.push(DispatchMethod {
                    def: function.def_id_value(db),
                    source_index,
                    name: ident_text(db, &sig.name),
                    payable: sig.payable.is_some(),
                    signature,
                    selector: PLACEHOLDER_SELECTOR.to_owned(),
                    inputs,
                    outputs,
                });
            }
            FuncKind::Constructor => {
                if constructor.is_some() {
                    diagnostics.push(contract_diag_multiple_constructors(db, function.span(db)));
                    continue;
                }
                let sig = function.sig(db);
                let type_vars =
                    function_type_vars(db, &contract_type_vars, function.def_id_value(db), sig);
                let lowered =
                    lower_normalized_function(db, module, item_resolutions, function, &type_vars);
                let inputs = abi_params(
                    db,
                    &param_names(db, sig.params.atom()),
                    &lowered.params,
                    &mut diagnostics,
                    sig.span,
                );
                constructor = Some(DispatchConstructor {
                    explicit: true,
                    source_index: Some(source_index),
                    payable: sig.payable.is_some(),
                    inputs,
                });
            }
            FuncKind::Fallback => {
                if fallback.is_some() {
                    diagnostics.push(contract_diag_multiple_fallbacks(db, function.span(db)));
                    continue;
                }
                let sig = function.sig(db);
                let type_vars =
                    function_type_vars(db, &contract_type_vars, function.def_id_value(db), sig);
                let lowered =
                    lower_normalized_function(db, module, item_resolutions, function, &type_vars);
                fallback = Some(DispatchFallback {
                    def: Some(function.def_id_value(db)),
                    explicit: true,
                    source_index: Some(source_index),
                    payable: sig.payable.is_some(),
                    inputs: abi_params(
                        db,
                        &param_names(db, sig.params.atom()),
                        &lowered.params,
                        &mut diagnostics,
                        sig.span,
                    ),
                    outputs: abi_outputs(db, lowered.ret, &mut diagnostics, sig.span),
                });
            }
        }
    }

    let constructor = constructor.unwrap_or(DispatchConstructor {
        explicit: false,
        source_index: None,
        payable: false,
        inputs: Vec::new(),
    });
    let fallback = fallback.unwrap_or(DispatchFallback {
        def: None,
        explicit: false,
        source_index: None,
        payable: false,
        inputs: Vec::new(),
        outputs: Vec::new(),
    });

    let mut seen = FxHashMap::<String, DefId<'db>>::default();
    for method in &methods {
        if method.signature.contains("<unsupported>") {
            continue;
        }
        if let Some(previous) = seen.insert(method.signature.clone(), method.def) {
            diagnostics.push(contract_diag_duplicate_signature(
                db,
                method.def,
                previous,
                &contract_name,
                &method.signature,
            ));
        }
    }

    DispatchSurface {
        contract: contract.def_id_value(db),
        name: contract_name,
        methods,
        constructor,
        fallback,
        diagnostics,
    }
}

fn lower_normalized_function<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    function: FunctionDef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> LoweredFunction<'db> {
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(type_vars),
    );
    let mut lowered = lowerer.lower_function(function);
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
    lowered.params = lowered
        .params
        .into_iter()
        .map(|param| normalizer.normalize_ty(param))
        .collect();
    lowered.ret = normalizer.normalize_ty(lowered.ret);
    lowered
}

fn find_contract_by_def<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ContractDef<'db>> {
    module.items(db).iter().find_map(|item| match item {
        Item::ContractDef(contract) if contract.def_id_value(db) == def => Some(*contract),
        _ => None,
    })
}


fn method_signature_string<'db>(
    db: &'db dyn Db,
    name: &str,
    params: &[Ty<'db>],
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(name);
    out.push('(');
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&signature_type_string(db, *param)?);
    }
    out.push(')');
    Ok(out)
}

fn signature_type_string<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Result<String, String> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty() => Ok("uint256".to_owned()),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
            args,
        } if args.is_empty() => Ok("bool".to_owned()),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::String),
            args,
        } if args.is_empty() => Ok("string".to_owned()),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Ok(String::new()),
        TyKind::Tuple(elems) => tuple_signature_string(db, elems),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => tuple_signature_string(db, args),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if user
            .def
            .name(db)
            .as_deref()
            .is_some_and(is_transparent_abi_location)
            && args.len() == 1 =>
        {
            signature_type_string(db, args[0])
        }
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if args.is_empty() => Ok(user
            .def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", user.kind))),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => Err(ty.display(db)),
        TyKind::Named { .. } | TyKind::Function { .. } | TyKind::Comptime(_) => Err(ty.display(db)),
    }
}

fn tuple_signature_string<'db>(db: &'db dyn Db, elems: &[Ty<'db>]) -> Result<String, String> {
    let mut parts = Vec::new();
    for elem in flatten_tuple(db, elems) {
        parts.push(signature_type_string(db, elem)?);
    }
    Ok(parts.join(","))
}

fn abi_params<'db>(
    db: &'db dyn Db,
    names: &[String],
    tys: &[Ty<'db>],
    diagnostics: &mut Vec<Diagnostic>,
    span: hir::span::Span<'db>,
) -> Vec<AbiParam> {
    tys.iter()
        .enumerate()
        .map(|(index, ty)| {
            match abi_param(db, names.get(index).cloned().unwrap_or_default(), *ty) {
                Ok(param) => param,
                Err(err) => {
                    diagnostics.push(contract_diag_unsupported_abi_type(
                        db,
                        span,
                        "ABI parameter",
                        &err,
                    ));
                    AbiParam {
                        name: names.get(index).cloned().unwrap_or_default(),
                        ty: "<unsupported>".to_owned(),
                        components: Vec::new(),
                    }
                }
            }
        })
        .collect()
}

fn abi_outputs<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    diagnostics: &mut Vec<Diagnostic>,
    span: hir::span::Span<'db>,
) -> Vec<AbiParam> {
    if is_unit_ty(db, ty) {
        return Vec::new();
    }
    flatten_output_ty(db, ty)
        .into_iter()
        .map(|ty| match abi_param(db, String::new(), ty) {
            Ok(param) => param,
            Err(err) => {
                diagnostics.push(contract_diag_unsupported_abi_type(
                    db,
                    span,
                    "ABI output",
                    &err,
                ));
                AbiParam {
                    name: String::new(),
                    ty: "<unsupported>".to_owned(),
                    components: Vec::new(),
                }
            }
        })
        .collect()
}

fn abi_param<'db>(db: &'db dyn Db, name: String, ty: Ty<'db>) -> Result<AbiParam, String> {
    let (ty, components) = abi_type_of(db, ty)?;
    Ok(AbiParam {
        name,
        ty,
        components,
    })
}

fn abi_type_of<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Result<(String, Vec<AbiParam>), String> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty() => Ok(("uint256".to_owned(), Vec::new())),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
            args,
        } if args.is_empty() => Ok(("bool".to_owned(), Vec::new())),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::String),
            args,
        } if args.is_empty() => Ok(("string".to_owned(), Vec::new())),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Ok(("".to_owned(), Vec::new())),
        TyKind::Tuple(elems) if elems.is_empty() => Ok(("".to_owned(), Vec::new())),
        TyKind::Tuple(elems) => Ok((
            "tuple".to_owned(),
            flatten_tuple(db, elems)
                .into_iter()
                .map(|elem| abi_param(db, String::new(), elem))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => Ok((
            "tuple".to_owned(),
            flatten_tuple(db, args)
                .into_iter()
                .map(|elem| abi_param(db, String::new(), elem))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if user
            .def
            .name(db)
            .as_deref()
            .is_some_and(is_transparent_abi_location)
            && args.len() == 1 =>
        {
            abi_type_of(db, args[0])
        }
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if args.is_empty() => Ok((
            user.def
                .name(db)
                .unwrap_or_else(|| format!("{:?}", user.kind)),
            Vec::new(),
        )),
        _ => Err(ty.display(db)),
    }
}

fn flatten_output_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Vec<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Tuple(elems) => flatten_tuple(db, elems),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => flatten_tuple(db, args),
        _ => vec![ty],
    }
}

fn flatten_tuple<'db>(db: &'db dyn Db, elems: &[Ty<'db>]) -> Vec<Ty<'db>> {
    let mut out = Vec::new();
    for elem in elems {
        match elem.kind(db) {
            TyKind::Tuple(nested) => out.extend(flatten_tuple(db, nested)),
            TyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => out.extend(flatten_tuple(db, args)),
            _ => out.push(*elem),
        }
    }
    out
}

fn is_unit_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Tuple(elems) if elems.is_empty()
    ) || matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty()
    )
}

fn is_transparent_abi_location(name: &str) -> bool {
    matches!(name, "memory" | "calldata")
}

fn contract_diag_duplicate_signature<'db>(
    db: &'db dyn Db,
    def: DefId<'db>,
    previous: DefId<'db>,
    contract: &str,
    signature: &str,
) -> Diagnostic {
    let _ = (db, def, previous);
    Diagnostic::error(format!(
        "duplicate public ABI signature in contract `{contract}`: {signature}"
    ))
    .with_code("SC0230")
}

fn contract_diag_unsupported_abi_type<'db>(
    db: &'db dyn Db,
    span: hir::span::Span<'db>,
    context: &str,
    ty: &str,
) -> Diagnostic {
    Diagnostic::error(format!("{context} cannot be represented in the ABI: {ty}"))
        .with_code("SC0231")
        .with_primary_label(db, span, Some("unsupported ABI type"))
}

fn contract_diag_multiple_constructors<'db>(
    db: &'db dyn Db,
    span: hir::span::Span<'db>,
) -> Diagnostic {
    Diagnostic::error("contract has more than one constructor")
        .with_code("SC0232")
        .with_primary_label(db, span, Some("extra constructor"))
}

fn contract_diag_multiple_fallbacks<'db>(
    db: &'db dyn Db,
    span: hir::span::Span<'db>,
) -> Diagnostic {
    Diagnostic::error("contract has more than one fallback")
        .with_code("SC0233")
        .with_primary_label(db, span, Some("extra fallback"))
}

enum AbiJsonEntry {
    Function {
        name: String,
        inputs: Vec<AbiParam>,
        outputs: Vec<AbiParam>,
        payable: bool,
    },
    Constructor {
        inputs: Vec<AbiParam>,
        payable: bool,
    },
    Fallback {
        payable: bool,
    },
}

fn render_abi_json(entries: &[AbiJsonEntry]) -> Result<String, String> {
    let mut out = String::new();
    if entries.is_empty() {
        out.push_str("[]\n");
        return Ok(out);
    }
    out.push_str("[\n");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        render_abi_entry(&mut out, entry, 1)?;
    }
    out.push_str("\n]\n");
    Ok(out)
}

fn render_abi_entry(out: &mut String, entry: &AbiJsonEntry, ind: usize) -> Result<(), String> {
    match entry {
        AbiJsonEntry::Function {
            name,
            inputs,
            outputs,
            payable,
        } => {
            line(out, ind, "{");
            render_named_params(out, ind + 1, "inputs", inputs, true)?;
            line(out, ind + 1, &format!("\"name\": {},", json_string(name)));
            render_named_params(out, ind + 1, "outputs", outputs, true)?;
            line(
                out,
                ind + 1,
                &format!("\"stateMutability\": \"{}\",", state_mutability(*payable)),
            );
            line(out, ind + 1, "\"type\": \"function\"");
            write!(out, "{}}}", indent(ind)).unwrap();
        }
        AbiJsonEntry::Constructor { inputs, payable } => {
            line(out, ind, "{");
            render_named_params(out, ind + 1, "inputs", inputs, true)?;
            line(
                out,
                ind + 1,
                &format!("\"stateMutability\": \"{}\",", state_mutability(*payable)),
            );
            line(out, ind + 1, "\"type\": \"constructor\"");
            write!(out, "{}}}", indent(ind)).unwrap();
        }
        AbiJsonEntry::Fallback { payable } => {
            line(out, ind, "{");
            line(
                out,
                ind + 1,
                &format!("\"stateMutability\": \"{}\",", state_mutability(*payable)),
            );
            line(out, ind + 1, "\"type\": \"fallback\"");
            write!(out, "{}}}", indent(ind)).unwrap();
        }
    }
    Ok(())
}

fn render_named_params(
    out: &mut String,
    ind: usize,
    name: &str,
    params: &[AbiParam],
    trailing_comma: bool,
) -> Result<(), String> {
    if params.iter().any(|param| param.ty == "<unsupported>") {
        return Err("cannot represent type in ABI".to_owned());
    }
    if params.is_empty() {
        line(
            out,
            ind,
            &format!("\"{name}\": []{}", if trailing_comma { "," } else { "" }),
        );
        return Ok(());
    }
    line(out, ind, &format!("\"{name}\": ["));
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        render_abi_param(out, ind + 1, param);
    }
    out.push('\n');
    line(
        out,
        ind,
        &format!("]{}", if trailing_comma { "," } else { "" }),
    );
    Ok(())
}

fn render_abi_param(out: &mut String, ind: usize, param: &AbiParam) {
    line(out, ind, "{");
    line(
        out,
        ind + 1,
        &format!("\"internalType\": {},", json_string(&param.ty)),
    );
    line(
        out,
        ind + 1,
        &format!("\"name\": {},", json_string(&param.name)),
    );
    line(
        out,
        ind + 1,
        &format!(
            "\"type\": {}{}",
            json_string(&param.ty),
            if param.components.is_empty() { "" } else { "," }
        ),
    );
    if !param.components.is_empty() {
        render_named_params(out, ind + 1, "components", &param.components, false)
            .expect("components already validated");
    }
    write!(out, "{}}}", indent(ind)).unwrap();
}

fn state_mutability(payable: bool) -> &'static str {
    if payable { "payable" } else { "nonpayable" }
}

fn line(out: &mut String, ind: usize, text: &str) {
    out.push_str(&indent(ind));
    out.push_str(text);
    out.push('\n');
}

fn indent(ind: usize) -> String {
    "  ".repeat(ind)
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => write!(&mut out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn collect_desugar_plans<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item: Item<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
    inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    out: &mut Vec<BodyDesugarPlan<'db>>,
) {
    match item {
        Item::FunctionDef(function) => {
            collect_function_desugar_plan(
                db,
                module,
                function,
                resolution,
                inherited_type_vars,
                out,
            );
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                if let ContractItem::FunctionDef(function) = *item {
                    collect_function_desugar_plan(
                        db, module, function, resolution, &inherited, out,
                    );
                }
            }
        }
        Item::InstanceDef(instance) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                instance.def_id_value(db),
                instance.type_var_elems(db),
            ));
            for method in instance.methods(db) {
                collect_function_desugar_plan(db, module, *method, resolution, &inherited, out);
            }
        }
        Item::TypeAlias(_)
        | Item::AdtDef(_)
        | Item::ClassDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => {}
    }
}

fn collect_function_desugar_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
    inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    out: &mut Vec<BodyDesugarPlan<'db>>,
) {
    let Some(body) = function.body(db) else {
        return;
    };
    let Some(body_map) = body_resolution_for(resolution, body) else {
        return;
    };
    let expr_resolutions = body_map
        .exprs
        .iter()
        .map(|entry| ((entry.body, entry.expr), entry.resolution.clone()))
        .collect::<FxHashMap<_, _>>();
    let pat_resolutions = body_map
        .pats
        .iter()
        .map(|entry| ((entry.body, entry.pat), entry.resolution.clone()))
        .collect::<FxHashMap<_, _>>();
    let call_site_evidence = desugar_inference_result(
        db,
        module,
        function,
        resolution,
        body_map,
        inherited_type_vars,
    )
    .map(|result| {
        result
            .call_site_evidence
            .into_iter()
            .map(|evidence| {
                (
                    (evidence.body, evidence.call_expr, evidence.callee_expr),
                    evidence,
                )
            })
            .collect::<FxHashMap<_, _>>()
    })
    .unwrap_or_default();
    let mut collector = DesugarCollector {
        db,
        body,
        expr_resolutions,
        pat_resolutions,
        call_site_evidence,
        transforms: Vec::new(),
    };
    for stmt in body.top_level_stmts(db) {
        collector.stmt(*stmt);
    }
    if !collector.transforms.is_empty() {
        out.push(BodyDesugarPlan {
            function: function.def_id_value(db),
            function_name: ident_text(db, &function.sig(db).name),
            transforms: collector.transforms,
        });
    }
}

fn desugar_inference_result<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
    body_map: &hir_nameres::BodyResolutionMap<'db>,
    inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<crate::InferenceResult<'db>> {
    if !body_map.diagnostics.is_empty() {
        return None;
    }
    let body = function.body(db)?;
    let sig = function.sig(db);
    let mut type_vars = inherited_type_vars.to_vec();
    type_vars.extend(function_type_vars(db, &[], function.def_id_value(db), sig));
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        &resolution.item_resolutions,
        BinderEnv::from_type_vars(&type_vars),
    );
    let mut normalizer = AliasNormalizer::new(db, module, &resolution.item_resolutions);
    let mut lowered = lowerer.lower_function(function);
    lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
    lowered.params = lowered
        .params
        .into_iter()
        .map(|param| normalizer.normalize_ty(param))
        .collect();
    lowered.ret = normalizer.normalize_ty(lowered.ret);
    let base_trait_env = trait_env_from_module_resolution(db, module, resolution);
    let trait_env = trait_env_with_givens(
        db,
        base_trait_env,
        lowered.scheme.body(db).preds(db).clone(),
    );
    let ctx = BodyTyContext::new(
        module,
        body_map.clone(),
        type_vars,
        lowered.params,
        Some(lowered.ret),
    )
    .with_param_names(param_names(db, sig.params.atom()))
    .with_trait_env(trait_env);
    Some(infer_body(db, body, ctx))
}

struct DesugarCollector<'db> {
    db: &'db dyn Db,
    body: FuncBody<'db>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pat_resolutions: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), hir_nameres::Resolution<'db>>,
    call_site_evidence:
        FxHashMap<(FuncBody<'db>, Id<Expr<'db>>, Id<Expr<'db>>), CallSiteEvidence<'db>>,
    transforms: Vec<FrontendTransform<'db>>,
}

impl<'db> DesugarCollector<'db> {
    fn stmt(&mut self, stmt_id: Id<Stmt<'db>>) {
        match &self.body.stmts(self.db).get(stmt_id).kind {
            StmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    self.expr(*init);
                }
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(*expr);
                }
            }
            StmtKind::Expr(expr) => self.expr(*expr),
            StmtKind::Assign { lhs, rhs }
            | StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                self.field_write(stmt_id, *lhs);
                self.expr(*rhs);
            }
            StmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.expr(*scrutinee);
                }
                for arm in arms {
                    for pat in &arm.pats {
                        self.pat(*pat);
                    }
                    for stmt in &arm.body {
                        self.stmt(*stmt);
                    }
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                for stmt in init {
                    self.stmt(*stmt);
                }
                self.expr(*cond);
                for stmt in post {
                    self.stmt(*stmt);
                }
                for stmt in body {
                    self.stmt(*stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.transforms.push(FrontendTransform::IfStmtToMatch {
                    body: self.body,
                    stmt: stmt_id,
                });
                self.expr(*cond);
                for stmt in then_body {
                    self.stmt(*stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.stmt(*stmt);
                    }
                }
            }
            StmtKind::Block { body } => {
                for stmt in body {
                    self.stmt(*stmt);
                }
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn expr(&mut self, expr_id: Id<Expr<'db>>) {
        if let Some(hir_nameres::Resolution::Field(field)) =
            self.expr_resolutions.get(&(self.body, expr_id))
        {
            let selector = selector_name(self.db, field);
            self.transforms.push(FrontendTransform::FieldRead {
                body: self.body,
                expr: expr_id,
                field: *field,
                selector: selector.clone(),
                hook: format!("RVA.acc(MemberAccessProxy(ContractStorage(_), {selector}))"),
            });
        }
        match &self.body.exprs(self.db).get(expr_id).kind {
            ExprKind::Ident(name) => {
                let text = ident_text(self.db, name);
                if matches!(text.as_str(), "true" | "false") {
                    self.transforms.push(FrontendTransform::BoolToUnitSum {
                        body: self.body,
                        node: BoolNode::Expr(expr_id),
                        source: text.clone(),
                        replacement: if text == "true" { "inr(())" } else { "inl(())" }.to_owned(),
                    });
                }
            }
            ExprKind::DotCtor { name, args, .. } => {
                let text = ident_text(self.db, name);
                if matches!(text.as_str(), "true" | "false") {
                    self.transforms.push(FrontendTransform::BoolToUnitSum {
                        body: self.body,
                        node: BoolNode::Expr(expr_id),
                        source: text.clone(),
                        replacement: if text == "true" { "inr(())" } else { "inl(())" }.to_owned(),
                    });
                }
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Lambda { body, .. } => {
                for stmt in body.top_level_stmts(self.db) {
                    let mut nested = DesugarCollector {
                        db: self.db,
                        body: *body,
                        expr_resolutions: self.expr_resolutions.clone(),
                        pat_resolutions: self.pat_resolutions.clone(),
                        call_site_evidence: self.call_site_evidence.clone(),
                        transforms: Vec::new(),
                    };
                    nested.stmt(*stmt);
                    self.transforms.extend(nested.transforms);
                }
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(*lhs);
                self.expr(*rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(*base);
                self.expr(*index);
            }
            ExprKind::Call { callee, args } => {
                if !self.is_direct_call(*callee) {
                    let evidence = self
                        .call_site_evidence
                        .get(&(self.body, expr_id, *callee))
                        .cloned();
                    let callee_identity = evidence
                        .as_ref()
                        .map(|evidence| evidence.callee.clone())
                        .unwrap_or(CallSiteCallee::Invokable);
                    self.transforms.push(FrontendTransform::IndirectCall {
                        body: self.body,
                        call_expr: expr_id,
                        callee_expr: *callee,
                        callee: callee_identity,
                        args: indirect_arg_shape(args),
                        evidence,
                    });
                }
                self.expr(*callee);
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Field { base, .. } => {
                self.expr(*base);
            }
            ExprKind::TypeAnnot { expr, .. } | ExprKind::UnaryOp { expr, .. } => self.expr(*expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.transforms.push(FrontendTransform::IfExprToMatch {
                    body: self.body,
                    expr: expr_id,
                });
                self.expr(*cond);
                self.expr(*then_expr);
                self.expr(*else_expr);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.expr(*elem);
                }
            }
            ExprKind::Lit(_) | ExprKind::Proxy { .. } | ExprKind::Error => {}
        }
    }

    fn pat(&mut self, pat_id: Id<Pat<'db>>) {
        if let Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
            hir_nameres::BuiltinCtor::True,
        ))) = self.pat_resolutions.get(&(self.body, pat_id))
        {
            self.transforms.push(FrontendTransform::BoolToUnitSum {
                body: self.body,
                node: BoolNode::Pat(pat_id),
                source: "true".to_owned(),
                replacement: "inr(())".to_owned(),
            });
        }
        if let Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
            hir_nameres::BuiltinCtor::False,
        ))) = self.pat_resolutions.get(&(self.body, pat_id))
        {
            self.transforms.push(FrontendTransform::BoolToUnitSum {
                body: self.body,
                node: BoolNode::Pat(pat_id),
                source: "false".to_owned(),
                replacement: "inl(())".to_owned(),
            });
        }
        match &self.body.pats(self.db).get(pat_id).kind {
            PatKind::Ctor { args, .. } | PatKind::Tuple { elems: args } => {
                for arg in args {
                    self.pat(*arg);
                }
            }
            PatKind::ComptimeLabel { expr, .. } => self.expr(*expr),
            PatKind::Wildcard | PatKind::Var(_) | PatKind::Lit(_) | PatKind::Error => {}
        }
    }

    fn field_write(&mut self, stmt_id: Id<Stmt<'db>>, lhs: Id<Expr<'db>>) {
        if let Some(hir_nameres::Resolution::Field(field)) =
            self.expr_resolutions.get(&(self.body, lhs))
        {
            let selector = selector_name(self.db, field);
            self.transforms.push(FrontendTransform::FieldWrite {
                body: self.body,
                stmt: stmt_id,
                field: *field,
                selector: selector.clone(),
                hook: format!(
                    "Assign.assign(LVA.acc(MemberAccessProxy(ContractStorage(_), {selector})), <rhs>)"
                ),
            });
        } else {
            self.expr(lhs);
        }
    }

    fn is_direct_call(&self, callee: Id<Expr<'db>>) -> bool {
        self.expr_resolutions
            .get(&(self.body, callee))
            .is_some_and(is_direct_call_resolution)
    }
}

fn indirect_arg_shape<'db>(args: &[Id<Expr<'db>>]) -> IndirectArgShape<'db> {
    let Some((head, tail)) = args.split_first() else {
        return IndirectArgShape::Unit;
    };
    if tail.is_empty() {
        IndirectArgShape::Single(*head)
    } else {
        IndirectArgShape::Pair {
            head: *head,
            tail: Box::new(indirect_arg_shape(tail)),
        }
    }
}

fn is_direct_call_resolution(resolution: &hir_nameres::Resolution<'_>) -> bool {
    matches!(
        resolution,
        hir_nameres::Resolution::Def {
            kind: hir_nameres::DefResolutionKind::Function,
            ..
        } | hir_nameres::Resolution::Ctor { .. }
            | hir_nameres::Resolution::ClassMethod { .. }
            | hir_nameres::Resolution::Builtin(
                hir_nameres::BuiltinKind::Constructor(_)
                    | hir_nameres::BuiltinKind::Function(_)
                    | hir_nameres::BuiltinKind::ClassMethod(_)
            )
    )
}

fn body_resolution_for<'a, 'db>(
    resolution: &'a hir_nameres::ModuleResolutionMap<'db>,
    body: FuncBody<'db>,
) -> Option<&'a hir_nameres::BodyResolutionMap<'db>> {
    resolution.bodies.iter().find(|map| {
        map.exprs.iter().any(|entry| entry.body == body)
            || map.stmt_bindings.iter().any(|entry| entry.body == body)
            || map.pats.iter().any(|entry| entry.body == body)
    })
}

fn selector_name<'db>(db: &'db dyn HirDb, field: &hir_nameres::FieldId<'db>) -> String {
    let contract = field
        .contract
        .name(db)
        .unwrap_or_else(|| "Contract".to_owned());
    format!("{contract}_field{}_sel", field.index)
}

fn function_type_vars<'db>(
    db: &'db dyn HirDb,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    owner: DefId<'db>,
    sig: &hir::ast::function::FuncSig<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    let mut vars = inherited.to_vec();
    vars.extend(type_var_bindings(owner, &sig.type_vars));
    let _ = db;
    vars
}

fn type_var_bindings<'db>(
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| hir_nameres::TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
}

fn param_names<'db>(db: &'db dyn HirDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(ident_text(db, name))
            }
            FuncParam::Error { .. } => None,
        })
        .collect()
}

fn ident_text<'db>(db: &'db dyn HirDb, ident: &SpannedElem<'db, Ident<'db>>) -> String {
    (*ident.atom()).text(db).to_owned()
}
