//! Select source-comment scenarios without changing the raw transaction runner.

use hir::ast::item::{ContractItem, Item, Module};

use super::{
    E2eFailure, FailureKind, RawE2eVector, ResolvedE2eCall, configured_anvil_hardfork,
    parse_e2e_directive, parse_e2e_scenario,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eExecution {
    Directives(Vec<ResolvedE2eCall>),
    Raw(RawE2eVector),
}

impl E2eExecution {
    pub fn effective_evm_version(&self) -> String {
        match self {
            Self::Directives(_) => configured_anvil_hardfork(),
            Self::Raw(vector) => vector.effective_evm_version().to_owned(),
        }
    }
}

/// Discover the single contract scenario in the entry module. Reject mixed
/// modes rather than silently ignoring method expectations or a second scenario.
pub fn source_e2e_scenario<'db>(
    db: &'db dyn parser::Db,
    module: Module<'db>,
) -> Result<Option<RawE2eVector>, E2eFailure> {
    let has_directive = |comments: &[hir::ast::item::SourceComment]| {
        comments
            .iter()
            .any(|comment| !matches!(parse_e2e_directive(&comment.text), Ok(None)))
    };
    let mut selected = None;
    let mut method_directives = false;
    for item in module.items(db) {
        match item {
            Item::ContractDef(contract) => {
                let name = contract.name_elem(db).atom().text(db);
                let scenario = parse_e2e_scenario(
                    name,
                    contract
                        .leading_comments(db)
                        .iter()
                        .map(|comment| comment.text.as_str()),
                )
                .map_err(|error| {
                    E2eFailure::new(FailureKind::Directive, format!("{name}: {error}"))
                })?;
                if let Some(scenario) = scenario
                    && selected.replace(scenario).is_some()
                {
                    return Err(E2eFailure::new(
                        FailureKind::Directive,
                        "one fixture may contain only one contract scenario",
                    ));
                }
                for item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = item {
                        method_directives |= has_directive(function.leading_comments(db));
                    }
                }
            }
            Item::FunctionDef(function) => {
                method_directives |= has_directive(function.leading_comments(db));
            }
            Item::InstanceDef(instance) => {
                for function in instance.methods(db) {
                    method_directives |= has_directive(function.leading_comments(db));
                }
            }
            _ if has_directive(item.leading_comments(db)) => {
                return Err(E2eFailure::new(
                    FailureKind::Directive,
                    "E2E directives must precede a contract or a public contract method",
                ));
            }
            _ => {}
        }
    }
    if selected.is_some() && method_directives {
        return Err(E2eFailure::new(
            FailureKind::Directive,
            "cannot mix a contract scenario with method E2E directives",
        ));
    }
    Ok(selected)
}
