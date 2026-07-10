use hir::{
    anchor::DefId,
    ast::item::{ContractDef, ContractItem, FuncKind, Import, ImportSelector, Item, Module},
    diag::{Diagnostic, DiagnosticCode},
    nameres as hir_nameres,
    span::Spanned,
};
use nameres::{LibraryId, module_id_for_source_file, resolve_direct_import_target_candidate};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;

use super::{
    abi::{
        AbiParam, AbiSelector, AbiSignature, AbiType, abi_outputs, abi_params, abi_selector,
        contract_diag_unsupported_abi_type, method_signature_string,
    },
    helpers::{
        find_contract_by_def, function_type_vars, ident_text, lower_normalized_function,
        param_names, resolve_contract_item_types, type_var_bindings,
    },
};
use crate::Db;

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
            diagnostics: Vec::new(),
        };
    };
    let item_resolutions = resolve_contract_item_types(db, module);
    contract_dispatch_surface_with_resolutions(db, module, &item_resolutions, contract)
}

/// Returns diagnostics for every contract dispatch surface in a module.
pub fn module_contract_diagnostics<'db>(db: &'db dyn Db, module: Module<'db>) -> Vec<Diagnostic> {
    let has_std_dispatch_import = module_has_canonical_std_dispatch_import(db, module);
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
            let surface_has_dispatch_errors = surface.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code.as_deref(),
                    Some("SC0230" | "SC0231" | "SC0232" | "SC0233" | "SC0235")
                )
            });
            let mut diagnostics = surface
                .diagnostics
                .into_iter()
                .filter(move |diagnostic| {
                    diagnostic.code.as_deref() != Some("SC0231") || dispatch_generated
                })
                .collect::<Vec<_>>();
            diagnostics.extend(contract_runtime_main_diagnostics(db, contract));
            if dispatch_generated && !has_std_dispatch_import && !surface_has_dispatch_errors {
                diagnostics.push(contract_diag_missing_std_dispatch_import(
                    db,
                    contract,
                    &surface.name,
                ));
            }
            diagnostics
        })
        .filter(|diagnostic| {
            matches!(
                diagnostic.code.as_deref(),
                Some("SC0230" | "SC0231" | "SC0232" | "SC0233" | "SC0234" | "SC0235" | "SC0236")
            )
        })
        .collect()
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
    !contract.items(db).iter().any(|item| {
        let ContractItem::FunctionDef(function) = item else {
            return false;
        };
        function.kind(db) == FuncKind::Function && ident_text(db, &function.sig(db).name) == "main"
    })
}

/// Returns whether `module` explicitly wildcard-imports the canonical standard
/// dispatch module required by generated contract entries.
pub fn module_has_canonical_std_dispatch_import<'db>(db: &'db dyn Db, module: Module<'db>) -> bool {
    let Some(importing) = module_id_for_source_file(db, module.def_id_value(db).file(db)) else {
        return false;
    };
    module.items(db).iter().any(|item| {
        let Item::Import(import) = *item else {
            return false;
        };
        is_std_dispatch_wildcard_import(db, importing, import)
    })
}

fn is_std_dispatch_wildcard_import<'db>(
    db: &'db dyn Db,
    importing: nameres::ModuleId<'db>,
    import: Import<'db>,
) -> bool {
    import.external(db).is_none()
        && import.alias_elem(db).is_none()
        && import.hiding(db).is_empty()
        && matches!(import.selector(db), Some(ImportSelector::Wildcard))
        && import_path_is(db, import, &["std", "dispatch"])
        && resolve_direct_import_target_candidate(db, importing, import).is_ok_and(|target| {
            target.module.library(db) == &LibraryId::Std
                && target.module.logical_path(db).as_slice() == ["dispatch"]
        })
}

fn import_path_is(db: &dyn Db, import: Import<'_>, expected: &[&str]) -> bool {
    let path = import.path_elems(db);
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(elem, expected)| ident_text(db, elem) == *expected)
}

fn contract_dispatch_surface_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
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
                    &mut diagnostics,
                    sig.span,
                );
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
                );
                let outputs = abi_outputs(db, lowered.ret, &mut diagnostics, sig.span);
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

fn contract_diag_missing_std_dispatch_import<'db>(
    db: &'db dyn Db,
    contract: ContractDef<'db>,
    name: &str,
) -> Diagnostic {
    Diagnostic::error(format!(
        "contract `{name}` needs `import std.dispatch.{{*}};` for generated dispatch"
    ))
    .with_code(DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT)
    .with_primary_label(
        db,
        contract.name_elem(db).span(db),
        Some("generated dispatch is required for this contract"),
    )
    .with_note("contract dispatch is provided by `std.dispatch`")
    .with_help("add `import std.dispatch.{*};` to this module")
}
