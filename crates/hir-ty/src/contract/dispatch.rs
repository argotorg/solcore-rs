use hir::{
    anchor::DefId,
    ast::item::{ContractDef, ContractItem, FuncKind, Item, Module},
    diag::Diagnostic,
    nameres as hir_nameres,
};
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
                fallback = Some(DispatchFallback::Explicit {
                    def: function.def_id_value(db),
                    source_index,
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

    let constructor = constructor.unwrap_or(DispatchConstructor::Implicit);
    let fallback = fallback.unwrap_or(DispatchFallback::Default);

    let mut seen = FxHashMap::<String, DefId<'db>>::default();
    for method in &methods {
        if abi_params_contain_unsupported(&method.inputs) {
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

fn abi_params_contain_unsupported(params: &[AbiParam]) -> bool {
    params.iter().any(|param| {
        matches!(&param.ty, AbiType::Unsupported)
            || abi_params_contain_unsupported(&param.components)
    })
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
