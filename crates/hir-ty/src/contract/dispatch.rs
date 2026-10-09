use hir::{
    anchor::DefId,
    ast::item::{ContractDef, ContractItem, FuncKind, Item, Module},
    diag::{Diagnostic, DiagnosticCode, LabelSpan},
    nameres as hir_nameres,
    span::Spanned,
};
use nameres::{LibraryId, module_id_for_source_file};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    abi::{
        AbiAdtEvidence, AbiParam, AbiSelector, AbiSignature, AbiType, abi_outputs, abi_params,
        abi_selector, abi_type_contains_user_adt, contract_diag_unsupported_abi_type,
        method_signature_string,
    },
    helpers::{
        find_contract_by_def, function_type_vars, ident_text, lower_normalized_function,
        param_names, resolve_contract_item_types, type_var_bindings,
    },
};
use crate::{
    ClassId, ClauseOrigin, Db, DerivedClauseKind, Pred, PredKind, TraitEnvId, Ty, TyCtor, TyKind,
    TypeckDiagnostic, UserTyCtorKind,
    solver::{Evidence, Solution, canonical_goal, solve_report, solver_answer_is_closed_over_goal},
    support::canonical_std_adt_def,
};

const MAX_DERIVED_ABI_EVIDENCE: usize = 1_024;

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
    /// Canonical-ABI diagnostics produced specifically by the constructor.
    /// These remain compilation errors even when a source runtime `main`
    /// suppresses generated method dispatch.
    pub constructor_abi_diagnostics: Vec<Diagnostic>,
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
    /// First four bytes of `keccak256(signature)`.
    pub selector: AbiSelector,
    /// ABI input parameters.
    pub inputs: Vec<AbiParam>,
    /// ABI output parameters.
    pub outputs: Vec<AbiParam>,
}

/// Constructor dispatch/ABI entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum DispatchConstructor {
    /// No source constructor: implicit non-payable unit constructor.
    Implicit,
    /// Source constructor declaration.
    Explicit {
        /// Source declaration index within the contract.
        source_index: usize,
        /// Whether deployment may receive value.
        payable: bool,
        /// ABI input parameters.
        inputs: Vec<AbiParam>,
    },
}

/// Fallback dispatch/ABI entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum DispatchFallback<'db> {
    /// No source fallback: default non-payable unit fallback.
    Default,
    /// Source fallback declaration.
    Explicit {
        /// Source fallback definition.
        def: DefId<'db>,
        /// Source declaration index within the contract.
        source_index: usize,
        /// Whether fallback calls may receive value.
        payable: bool,
        /// ABI input parameters. Valid Solcore fallbacks are unit.
        inputs: Vec<AbiParam>,
        /// ABI output parameters. Valid Solcore fallbacks are unit.
        outputs: Vec<AbiParam>,
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

/// Returns the typed dispatch surface for `contract` using the supplied HIR
/// module directly. This is useful for backend-generated HIR overlays whose
/// source file URL may intentionally mirror a user file.
pub fn contract_dispatch_surface_for_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    contract: ContractDef<'db>,
) -> DispatchSurface<'db> {
    let item_resolutions = resolve_contract_item_types(db, module);
    contract_dispatch_surface_with_resolutions(db, module, &item_resolutions, contract)
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
            constructor: DispatchConstructor::Implicit,
            fallback: DispatchFallback::Default,
            constructor_abi_diagnostics: Vec::new(),
            diagnostics: Vec::new(),
        };
    };
    let item_resolutions = resolve_contract_item_types(db, module);
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
            let dispatch_generated = contract_needs_generated_dispatch(db, contract);
            let surface = contract_dispatch_surface(db, module, contract);
            let mut diagnostics = if dispatch_generated {
                surface.diagnostics
            } else {
                let mut diagnostics = surface
                    .diagnostics
                    .into_iter()
                    .filter(|diagnostic| diagnostic.code.as_deref() != Some("SC0231"))
                    .collect::<Vec<_>>();
                diagnostics.extend(surface.constructor_abi_diagnostics);
                diagnostics
            };
            diagnostics.extend(contract_runtime_main_diagnostics(db, contract));
            diagnostics
        })
        .filter(|diagnostic| {
            matches!(
                diagnostic.code.as_deref(),
                Some("SC0230" | "SC0231" | "SC0232" | "SC0233" | "SC0235" | "SC0236")
            )
        })
        .collect()
}

pub(crate) fn module_manual_generic_abi_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    prepared_module: Module<'db>,
    trait_env: TraitEnvId<'db>,
) -> Vec<Diagnostic> {
    let clauses = trait_env.clauses(db);
    let manual_evidence = clauses
        .iter()
        .filter_map(|clause| {
            let ClauseOrigin::Instance { def: instance, .. } = &clause.origin else {
                return None;
            };
            if instance
                .fingerprint(db)
                .as_deref()
                .is_some_and(|fingerprint| {
                    fingerprint.starts_with("solcore.generated.std_dispatch.")
                })
            {
                return None;
            }
            if module_id_for_source_file(db, instance.file(db))
                .is_some_and(|module| matches!(module.library(db), LibraryId::Std))
            {
                return None;
            }
            let PredKind::InClass {
                class: ClassId::User(class),
                main,
                ..
            } = clause.head.kind(db)
            else {
                return None;
            };
            let class_name = canonical_abi_class_name(db, *class)?;
            let generic_adt = if class_name == "Generic" {
                match main.kind(db) {
                    TyKind::Named {
                        ctor: TyCtor::User(user),
                        ..
                    } if user.kind == UserTyCtorKind::Adt => Some(user.def),
                    _ => return None,
                }
            } else {
                None
            };
            Some((*instance, class_name, generic_adt))
        })
        .collect::<Vec<_>>();
    if manual_evidence.is_empty() {
        return Vec::new();
    }
    let manual_instances = manual_evidence
        .iter()
        .map(|(instance, class_name, _)| (*instance, *class_name))
        .collect::<FxHashMap<_, _>>();
    let classes = clauses
        .iter()
        .filter_map(|clause| {
            let PredKind::InClass {
                class: ClassId::User(class),
                ..
            } = clause.head.kind(db)
            else {
                return None;
            };
            Some((canonical_abi_class_name(db, *class)?, *class))
        })
        .collect::<FxHashMap<_, _>>();

    let item_resolutions = resolve_contract_item_types(db, module);
    let mut diagnostics = Vec::new();
    for item in module.items(db) {
        let Item::ContractDef(contract) = *item else {
            continue;
        };
        let dispatch_generated = contract_needs_generated_dispatch(db, contract);
        let contract_name = ident_text(db, &contract.name_elem(db));
        let contract_type_vars =
            type_var_bindings(contract.def_id_value(db), contract.ty_param_elems(db));
        for item in contract.items(db) {
            let ContractItem::FunctionDef(function) = *item else {
                continue;
            };
            let sig = function.sig(db);
            let abi_context = match function.kind(db) {
                FuncKind::Constructor => Some("constructor".to_owned()),
                FuncKind::Function if sig.public.is_some() && dispatch_generated => {
                    Some(format!("public function `{}`", ident_text(db, &sig.name)))
                }
                FuncKind::Function | FuncKind::Fallback => None,
            };
            let Some(abi_context) = abi_context else {
                continue;
            };
            let type_vars =
                function_type_vars(db, &contract_type_vars, function.def_id_value(db), sig);
            let lowered = lower_normalized_function(
                db,
                module,
                &item_resolutions,
                contract.def_id_value(db),
                function,
                &type_vars,
            );
            let mut exposed_tys = lowered.params.clone();
            if function.kind(db) == FuncKind::Function {
                exposed_tys.push(lowered.ret);
            }
            let args = crate::lower::product_ty(db, lowered.params.iter().copied());
            let mut roots = Vec::new();
            let mut push_root = |class_name, main, other_args| {
                if let Some(class) = classes.get(class_name) {
                    roots.push((
                        class_name == "SigString",
                        Pred::in_class(db, ClassId::User(*class), main, other_args),
                    ));
                }
            };
            let reader = if function.kind(db) == FuncKind::Function {
                // Selector.compute signs the generated name and input product,
                // while ExecMethod decodes inputs and encodes the return value.
                push_root("SigString", args, Vec::new());
                let name = crate::contract_dispatch_name_type_name(
                    &contract_name,
                    &ident_text(db, &sig.name),
                );
                if let Some(name_ty) =
                    prepared_module
                        .items(db)
                        .iter()
                        .find_map(|item| match item {
                            Item::AdtDef(adt)
                                if ident_text(db, &adt.name_elem(db)) == name
                                    && adt.def_id_value(db).fingerprint(db).as_deref()
                                        == Some("solcore.generated.std_dispatch.name_type") =>
                            {
                                Some(Ty::named(
                                    db,
                                    TyCtor::User(crate::UserTyCtor {
                                        def: adt.def_id_value(db),
                                        kind: UserTyCtorKind::Adt,
                                    }),
                                    Vec::new(),
                                ))
                            }
                            _ => None,
                        })
                {
                    push_root("SigString", name_ty, Vec::new());
                }
                push_root("ABIAttribs", args, Vec::new());
                push_root("ABIAttribs", lowered.ret, Vec::new());
                push_root("ABIEncode", lowered.ret, Vec::new());
                Some("CalldataWordReader")
            } else if !lowered.params.is_empty() {
                // Constructor argument copying calls abi_decode with a memory
                // reader. An empty constructor has no decoding obligation.
                Some("MemoryWordReader")
            } else {
                None
            };
            if let Some(reader) = reader
                && let Some(reader) = canonical_std_adt_ty(db, reader, Vec::new())
                && let Some(decoder) = canonical_std_adt_ty(db, "ABIDecoder", vec![args, reader])
            {
                push_root("ABIDecode", decoder, vec![args]);
            }
            let mut selected_manual = FxHashSet::default();
            let mut visited_derived = FxHashSet::default();
            let mut exhausted_derived = None;
            for (selector, goal) in roots {
                let report = solve_report(db, trait_env, canonical_goal(db, goal));
                if report.exhausted {
                    continue;
                }
                if let Solution::Unique { subst, evidence } = report.solution
                    && solver_answer_is_closed_over_goal(db, goal, trait_env, &subst, &evidence)
                {
                    collect_manual_abi_evidence(
                        db,
                        &evidence,
                        &manual_instances,
                        selector,
                        &mut selected_manual,
                        &mut visited_derived,
                        &mut exhausted_derived,
                    );
                }
            }
            if let Some(pred) = exhausted_derived {
                diagnostics.push(
                    TypeckDiagnostic::SolverFuelExhausted {
                        span: LabelSpan::from_span(db, sig.span),
                        pred: crate::display::display_pred_source(db, pred, &[]),
                    }
                    .lower(),
                );
            }
            for (instance, class_name, generic_adt) in &manual_evidence {
                if *class_name != "Generic" && !selected_manual.contains(instance) {
                    continue;
                }
                if generic_adt.is_some_and(|adt| {
                    !exposed_tys
                        .iter()
                        .any(|ty| abi_type_contains_user_adt(db, *ty, adt))
                }) {
                    continue;
                }
                let subject = generic_adt.map_or_else(
                    || format!("visible manual `{class_name}` evidence"),
                    |adt| {
                        let adt_name = adt.name(db).unwrap_or_else(|| "<anonymous ADT>".to_owned());
                        format!("`{adt_name}` with visible manual `Generic` evidence")
                    },
                );
                diagnostics.push(
                    Diagnostic::error(format!(
                        "{abi_context} ABI for contract `{contract_name}` cannot use {subject}"
                    ))
                    .with_code("SC0231")
                    .with_primary_label(
                        db,
                        sig.span,
                        Some("external ABI evidence must be compiler-owned and canonical"),
                    )
                    .with_note(format!(
                        "impl `{}` can override canonical `{class_name}` behavior",
                        instance
                            .name(db)
                            .unwrap_or_else(|| class_name.to_string())
                    ))
                    .with_help(
                        "remove the visible manual ABI impl or keep this declaration out of the external ABI",
                    ),
                );
            }
        }
    }
    diagnostics
}

fn canonical_std_adt_ty<'db>(db: &'db dyn Db, name: &str, args: Vec<Ty<'db>>) -> Option<Ty<'db>> {
    canonical_std_adt_def(db, name).map(|def| {
        Ty::named(
            db,
            TyCtor::User(crate::UserTyCtor {
                def,
                kind: UserTyCtorKind::Adt,
            }),
            args,
        )
    })
}

/// Follows selected dictionaries, including representation and superclass
/// subproofs. Selector roots only own SigString evidence; execution roots own
/// encoding, decoding, and layout evidence.
fn collect_manual_abi_evidence<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
    manual_instances: &FxHashMap<DefId<'db>, &'static str>,
    selector: bool,
    selected: &mut FxHashSet<DefId<'db>>,
    visited_derived: &mut FxHashSet<(DerivedClauseKind<'db>, Pred<'db>)>,
    exhausted_derived: &mut Option<Pred<'db>>,
) {
    match evidence {
        Evidence::Instance {
            instance,
            sub_evidence,
            ..
        } => {
            if let Some(class_name) = manual_instances.get(instance)
                && if selector {
                    *class_name == "SigString"
                } else {
                    matches!(*class_name, "ABIAttribs" | "ABIEncode" | "ABIDecode")
                }
            {
                selected.insert(*instance);
            }
            for evidence in sub_evidence {
                collect_manual_abi_evidence(
                    db,
                    evidence,
                    manual_instances,
                    selector,
                    selected,
                    visited_derived,
                    exhausted_derived,
                );
            }
        }
        Evidence::Derived {
            kind,
            pred,
            sub_evidence,
        } => {
            for evidence in sub_evidence {
                collect_manual_abi_evidence(
                    db,
                    evidence,
                    manual_instances,
                    selector,
                    selected,
                    visited_derived,
                    exhausted_derived,
                );
            }
            // Derived ABI bodies resolve their representation dictionaries in
            // the defining module. Bound that additional proof graph just like
            // the solver bounds type-growing tables; cyclic source layouts
            // retain their existing unsupported-ABI diagnostic.
            if !selector
                && matches!(
                    kind,
                    DerivedClauseKind::AbiAttribs { .. } | DerivedClauseKind::AbiDecode { .. }
                )
                && !visited_derived.contains(&(*kind, *pred))
            {
                if visited_derived.len() == MAX_DERIVED_ABI_EVIDENCE {
                    exhausted_derived.get_or_insert(*pred);
                } else {
                    visited_derived.insert((*kind, *pred));
                    match crate::solver::derived_abi_delegated_evidence(db, evidence) {
                        Ok(Some(delegated)) => collect_manual_abi_evidence(
                            db,
                            &delegated,
                            manual_instances,
                            selector,
                            selected,
                            visited_derived,
                            exhausted_derived,
                        ),
                        Err(pred) => {
                            exhausted_derived.get_or_insert(pred);
                        }
                        Ok(None) => {}
                    }
                }
            }
        }
        Evidence::Superclass { child, .. } => {
            collect_manual_abi_evidence(
                db,
                child,
                manual_instances,
                selector,
                selected,
                visited_derived,
                exhausted_derived,
            );
        }
        Evidence::Builtin { .. } => {}
    }
}

fn canonical_abi_class_name(db: &dyn Db, class: DefId<'_>) -> Option<&'static str> {
    let name = class.name(db)?;
    let module = module_id_for_source_file(db, class.file(db))?;
    if module.library(db) != &LibraryId::Std {
        return None;
    }
    match (module.logical_path(db).as_slice(), name.as_str()) {
        ([path], "Generic") if path == "Generic" => Some("Generic"),
        ([path], "ABIAttribs" | "ABIEncode" | "ABIDecode") if path == "std" => {
            match name.as_str() {
                "ABIAttribs" => Some("ABIAttribs"),
                "ABIEncode" => Some("ABIEncode"),
                "ABIDecode" => Some("ABIDecode"),
                _ => None,
            }
        }
        ([path], "SigString") if path == "dispatch" => Some("SigString"),
        _ => None,
    }
}

fn visible_abi_adt_evidence<'db>(db: &'db dyn Db, module: Module<'db>) -> AbiAdtEvidence<'db> {
    let trait_env =
        if let Some(module_id) = module_id_for_source_file(db, module.def_id_value(db).file(db)) {
            crate::trait_env_for_module(db, module_id)
        } else {
            let resolution = hir_nameres::resolve_module(db, module);
            crate::trait_env_from_module_resolution(db, module, &resolution)
        };
    let mut manual_generic_adts = FxHashSet::default();
    let mut derived_abi_attribs = FxHashSet::default();
    let mut derived_abi_decodes = FxHashSet::default();

    for clause in trait_env.clauses(db) {
        match &clause.origin {
            ClauseOrigin::Derived(DerivedClauseKind::AbiAttribs { adt }) => {
                derived_abi_attribs.insert(*adt);
            }
            ClauseOrigin::Derived(DerivedClauseKind::AbiDecode { adt, .. }) => {
                derived_abi_decodes.insert(*adt);
            }
            ClauseOrigin::Instance { .. } => {
                let PredKind::InClass {
                    class: ClassId::User(class),
                    main,
                    ..
                } = clause.head.kind(db)
                else {
                    continue;
                };
                if canonical_abi_class_name(db, *class) != Some("Generic") {
                    continue;
                }
                if let TyKind::Named {
                    ctor: TyCtor::User(user),
                    ..
                } = main.kind(db)
                    && user.kind == UserTyCtorKind::Adt
                {
                    manual_generic_adts.insert(user.def);
                }
            }
            ClauseOrigin::Builtin
            | ClauseOrigin::Derived(_)
            | ClauseOrigin::Given
            | ClauseOrigin::Superclass(_) => {}
        }
    }

    let derived_abi_adts = derived_abi_attribs
        .into_iter()
        .filter(|adt| derived_abi_decodes.contains(adt))
        .collect();
    AbiAdtEvidence::new(manual_generic_adts, derived_abi_adts)
}

fn contract_runtime_main_diagnostics<'db>(
    db: &'db dyn Db,
    contract: ContractDef<'db>,
) -> Vec<Diagnostic> {
    contract
        .items(db)
        .iter()
        .filter_map(|item| {
            let ContractItem::FunctionDef(function) = *item else {
                return None;
            };
            let sig = function.sig(db);
            (function.kind(db) == FuncKind::Function
                && ident_text(db, &sig.name) == "main"
                && !sig.params.atom().is_empty())
            .then(|| {
                Diagnostic::error("contract runtime `main` must not take parameters")
                    .with_code(DiagnosticCode::TYPECK_CONTRACT_RUNTIME_MAIN_ARITY)
                    .with_primary_label(
                        db,
                        sig.params.span(db),
                        Some("runtime entry is called without arguments"),
                    )
                    .with_help("remove the parameters or rename this function")
            })
        })
        .collect()
}

/// Returns whether the compiler must synthesize this contract's runtime entry.
///
/// This deliberately follows the language's existing/Haskell-compatible
/// convention: any contract-local ordinary function named `main` is a
/// user-supplied runtime entry, irrespective of visibility.
pub fn contract_needs_generated_dispatch<'db>(db: &'db dyn Db, contract: ContractDef<'db>) -> bool {
    !contract.has_runtime_main(db)
}

fn contract_dispatch_surface_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    contract: ContractDef<'db>,
) -> DispatchSurface<'db> {
    let contract_name = ident_text(db, &contract.name_elem(db));
    let abi_evidence = visible_abi_adt_evidence(db, module);
    let contract_type_vars =
        type_var_bindings(contract.def_id_value(db), contract.ty_param_elems(db));
    let mut diagnostics = Vec::new();
    let mut methods = Vec::new();
    let mut constructor: Option<DispatchConstructor> = None;
    let mut constructor_abi_diagnostics = Vec::new();
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
                let lowered = lower_normalized_function(
                    db,
                    module,
                    item_resolutions,
                    contract.def_id_value(db),
                    function,
                    &type_vars,
                );
                let param_names = param_names(db, sig.params.atom());
                let inputs = abi_params(
                    db,
                    &param_names,
                    &lowered.params,
                    &mut diagnostics,
                    sig.span,
                    &abi_evidence,
                );
                let outputs =
                    abi_outputs(db, lowered.ret, &mut diagnostics, sig.span, &abi_evidence);
                let signature = method_signature_string(
                    db,
                    &ident_text(db, &sig.name),
                    &lowered.params,
                    &abi_evidence,
                )
                .unwrap_or_else(|err| {
                    diagnostics.push(contract_diag_unsupported_abi_type(
                        db,
                        sig.span,
                        &ident_text(db, &sig.name),
                        &err,
                    ));
                    format!("{}(<unsupported>)", ident_text(db, &sig.name))
                });
                let selector = abi_selector(db, AbiSignature::new(db, signature.clone()));
                methods.push(DispatchMethod {
                    def: function.def_id_value(db),
                    source_index,
                    name: ident_text(db, &sig.name),
                    payable: sig.payable.is_some(),
                    signature,
                    selector,
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
                let lowered = lower_normalized_function(
                    db,
                    module,
                    item_resolutions,
                    contract.def_id_value(db),
                    function,
                    &type_vars,
                );
                let inputs = abi_params(
                    db,
                    &param_names(db, sig.params.atom()),
                    &lowered.params,
                    &mut constructor_abi_diagnostics,
                    sig.span,
                    &abi_evidence,
                );
                diagnostics.extend(constructor_abi_diagnostics.iter().cloned());
                constructor = Some(DispatchConstructor::Explicit {
                    source_index,
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
                let lowered = lower_normalized_function(
                    db,
                    module,
                    item_resolutions,
                    contract.def_id_value(db),
                    function,
                    &type_vars,
                );
                let inputs = abi_params(
                    db,
                    &param_names(db, sig.params.atom()),
                    &lowered.params,
                    &mut diagnostics,
                    sig.span,
                    &abi_evidence,
                );
                let outputs =
                    abi_outputs(db, lowered.ret, &mut diagnostics, sig.span, &abi_evidence);
                if !inputs.is_empty() || !outputs.is_empty() {
                    diagnostics.push(contract_diag_unsupported_fallback_shape(
                        db,
                        function.span(db),
                    ));
                }
                fallback = Some(DispatchFallback::Explicit {
                    def: function.def_id_value(db),
                    source_index,
                    payable: sig.payable.is_some(),
                    inputs,
                    outputs,
                });
            }
        }
    }

    let constructor = constructor.unwrap_or(DispatchConstructor::Implicit);
    let fallback = fallback.unwrap_or(DispatchFallback::Default);

    let mut seen_signatures = FxHashMap::<String, usize>::default();
    let mut seen_selectors = FxHashMap::<AbiSelector, usize>::default();
    for (method_index, method) in methods.iter().enumerate() {
        if abi_params_contain_unsupported(&method.inputs) {
            continue;
        }

        if let Some(&previous_index) = seen_signatures.get(&method.signature) {
            diagnostics.push(contract_diag_duplicate_signature(
                db,
                dispatch_method_span(db, contract, method),
                dispatch_method_span(db, contract, &methods[previous_index]),
                &contract_name,
                &method.signature,
            ));
        } else {
            seen_signatures.insert(method.signature.clone(), method_index);
        }

        if let Some(&previous_index) = seen_selectors.get(&method.selector) {
            let previous = &methods[previous_index];
            if previous.signature != method.signature {
                diagnostics.push(contract_diag_selector_collision(
                    db,
                    dispatch_method_span(db, contract, method),
                    dispatch_method_span(db, contract, previous),
                    &contract_name,
                    method,
                    previous,
                ));
            }
        } else {
            seen_selectors.insert(method.selector, method_index);
        }
    }

    DispatchSurface {
        contract: contract.def_id_value(db),
        name: contract_name,
        methods,
        constructor,
        fallback,
        constructor_abi_diagnostics,
        diagnostics,
    }
}

fn abi_params_contain_unsupported(params: &[AbiParam]) -> bool {
    params.iter().any(|param| {
        matches!(&param.ty, AbiType::Unsupported)
            || abi_params_contain_unsupported(&param.components)
    })
}

fn contract_diag_duplicate_signature<'db>(
    db: &'db dyn Db,
    current_span: hir::span::Span<'db>,
    previous_span: hir::span::Span<'db>,
    contract: &str,
    signature: &str,
) -> Diagnostic {
    Diagnostic::error(format!(
        "duplicate public ABI signature in contract `{contract}`: {signature}"
    ))
    .with_code("SC0230")
    .with_primary_label(db, current_span, Some("duplicate ABI signature"))
    .with_secondary_label(db, previous_span, Some("previous declaration"))
}

fn contract_diag_selector_collision<'db>(
    db: &'db dyn Db,
    current_span: hir::span::Span<'db>,
    previous_span: hir::span::Span<'db>,
    contract: &str,
    current: &DispatchMethod<'db>,
    previous: &DispatchMethod<'db>,
) -> Diagnostic {
    Diagnostic::error(format!(
        "public ABI selector collision in contract `{contract}`: `{}` and `{}` both use {}",
        previous.signature,
        current.signature,
        current.selector.to_hex(),
    ))
    .with_code(DiagnosticCode::TYPECK_CONTRACT_SELECTOR_COLLISION)
    .with_primary_label(
        db,
        current_span,
        Some(format!("`{}` collides here", current.signature)),
    )
    .with_secondary_label(
        db,
        previous_span,
        Some(format!("`{}` first used this selector", previous.signature)),
    )
}

fn dispatch_method_span<'db>(
    db: &'db dyn Db,
    contract: ContractDef<'db>,
    method: &DispatchMethod<'db>,
) -> hir::span::Span<'db> {
    match contract.items(db).get(method.source_index) {
        Some(ContractItem::FunctionDef(function)) => function.sig(db).span,
        _ => contract.name_elem(db).span(db),
    }
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

fn contract_diag_unsupported_fallback_shape<'db>(
    db: &'db dyn Db,
    span: hir::span::Span<'db>,
) -> Diagnostic {
    Diagnostic::error("fallback ABI must be unit -> unit")
        .with_code("SC0231")
        .with_primary_label(db, span, Some("unsupported fallback ABI"))
}
