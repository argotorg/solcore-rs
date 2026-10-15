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
/// modes and misplaced directives rather than silently ignoring test steps.
pub fn source_e2e_scenario<'db>(
    db: &'db dyn parser::Db,
    module: Module<'db>,
) -> Result<Option<RawE2eVector>, E2eFailure> {
    let has_directive = |comments: &[hir::ast::item::SourceComment]| {
        comments
            .iter()
            .any(|comment| !matches!(parse_e2e_directive(&comment.text), Ok(None)))
    };
    let reject_misplaced = |comments: &[hir::ast::item::SourceComment]| {
        if has_directive(comments) {
            Err(E2eFailure::new(
                FailureKind::Directive,
                "E2E directives must precede a contract or a public contract method",
            ))
        } else {
            Ok(())
        }
    };
    let mut selected = None;
    let mut method_directives = false;
    for item in module.items(db) {
        if !matches!(item, Item::ContractDef(_) | Item::FunctionDef(_)) {
            reject_misplaced(item.leading_comments(db))?;
        }
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
                for (_, comments) in contract.fields_with_comments(db) {
                    reject_misplaced(comments)?;
                }
                for item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = item {
                        method_directives |= has_directive(function.leading_comments(db));
                    } else {
                        reject_misplaced(item.leading_comments(db))?;
                        if let ContractItem::AdtDef(adt) = item {
                            for (_, comments) in adt.ctors_with_comments(db) {
                                reject_misplaced(comments)?;
                            }
                        }
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
            Item::AdtDef(adt) => {
                for (_, comments) in adt.ctors_with_comments(db) {
                    reject_misplaced(comments)?;
                }
            }
            Item::ClassDef(class) => {
                for (_, comments) in class.methods_with_comments(db) {
                    reject_misplaced(comments)?;
                }
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
