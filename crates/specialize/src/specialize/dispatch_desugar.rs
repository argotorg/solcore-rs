use std::collections::BTreeSet;

use hir::{
    ast::item::{ContractDef, ContractItem, FuncKind, Item, Module},
    input::SourceFile,
    nameres::ident_text,
};
use hir_ty::{
    AbiParam, AbiType, Db, DispatchFallback, DispatchMethod, contract_dispatch_surface_for_module,
};
use parser::parse_file_to_hir;

use super::naming::module_id_for_source_file;

pub(super) const GENERATED_MARKER: &str = "solcore-rs generated std dispatch";

pub(super) fn module_with_std_dispatch_main<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> Option<Module<'db>> {
    let file = module.def_id_value(db).file(db);
    let source = file.content(db).as_ref()?;
    if source.contains(GENERATED_MARKER) {
        return None;
    }
    module_id_for_source_file(db, file)?;

    let mut top_level = String::new();
    let mut insertions = Vec::new();
    for item in module.items(db) {
        let Item::ContractDef(contract) = *item else {
            continue;
        };
        if contract_has_main(db, contract) {
            continue;
        }
        let surface = contract_dispatch_surface_for_module(db, module, contract);
        if !surface.diagnostics.is_empty() {
            continue;
        }
        top_level.push_str(&dispatch_name_decls(&surface.name, &surface.methods)?);
        let main = contract_main(db, &surface.name, &surface.methods, &surface.fallback)?;
        let offset = contract_insert_offset(db, source, contract)?;
        insertions.push((offset, main));
    }

    if top_level.is_empty() && insertions.is_empty() {
        return None;
    }

    let mut augmented = String::new();
    augmented.push_str("// ");
    augmented.push_str(GENERATED_MARKER);
    augmented.push('\n');
    augmented.push_str(&top_level);
    let source_base = augmented.len();
    augmented.push_str(source);
    insertions.sort_by_key(|(offset, _)| *offset);
    for (offset, text) in insertions.into_iter().rev() {
        augmented.insert_str(source_base + offset, &text);
    }

    let augmented_file = SourceFile::new(db, file.url(db).clone(), Some(augmented));
    Some(parse_file_to_hir(db, augmented_file).module(db))
}

fn contract_has_main(db: &dyn Db, contract: ContractDef<'_>) -> bool {
    contract.items(db).iter().any(|item| {
        let ContractItem::FunctionDef(function) = item else {
            return false;
        };
        function.kind(db) == FuncKind::Function && ident_text(db, &function.sig(db).name) == "main"
    })
}

fn contract_insert_offset(db: &dyn Db, source: &str, contract: ContractDef<'_>) -> Option<usize> {
    let span = contract.span(db).resolve_to_absolute(db);
    let start = span.start().as_usize().min(source.len());
    let open = source[start..].find('{')? + start;
    let mut depth = 0usize;
    for (relative, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + relative);
                }
            }
            _ => {}
        }
    }
    None
}

fn dispatch_name_decls(contract: &str, methods: &[DispatchMethod<'_>]) -> Option<String> {
    let mut out = String::new();
    let mut declared = BTreeSet::new();
    for method in methods {
        if !declared.insert(method.name.clone()) {
            continue;
        }
        let name_ty = dispatch_name_ty(contract, &method.name);
        out.push_str("data ");
        out.push_str(&name_ty);
        out.push_str(";\n\n");
        out.push_str("instance ");
        out.push_str(&name_ty);
        out.push_str(":SigString {\n");
        out.push_str("    function sigStr(p: Proxy(");
        out.push_str(&name_ty);
        out.push_str(")) -> string {\n");
        out.push_str("        \"");
        out.push_str(&escape_string(&method.name));
        out.push_str("\"\n");
        out.push_str("    }\n");
        out.push_str("}\n\n");
    }
    Some(out)
}

fn contract_main(
    db: &dyn Db,
    contract: &str,
    methods: &[DispatchMethod<'_>],
    fallback: &DispatchFallback<'_>,
) -> Option<String> {
    let fallback_expr = fallback_expr(db, fallback)?;
    let mut body = String::new();
    body.push_str("\n    function main() -> () {\n");
    body.push_str("        assembly { mstore(0x40, memoryguard(128)) }\n");
    if methods.is_empty() {
        body.push_str("        ExecMethod.exec(");
        body.push_str(&fallback_expr);
        body.push_str(");\n");
    } else {
        body.push_str("        if (dispatch_has_selector()) {\n");
        for method in methods {
            body.push_str("            if (");
            body.push_str(&method_selector_expr(method)?);
            body.push_str(") {\n                ExecMethod.exec(");
            body.push_str(&method_expr(contract, method)?);
            body.push_str(");\n            }\n");
        }
        body.push_str("        }\n");
        body.push_str("        ExecMethod.exec(");
        body.push_str(&fallback_expr);
        body.push_str(");\n");
    }
    body.push_str("    }\n");
    Some(body)
}

fn method_expr(contract: &str, method: &DispatchMethod<'_>) -> Option<String> {
    let name_ty = dispatch_name_ty(contract, &method.name);
    let payability = if method.payable {
        "Payable"
    } else {
        "NonPayable"
    };
    let args_ty = method_args_ty(method)?;
    let rets_ty = method_rets_ty(method)?;
    Some(format!(
        "Method(Proxy : Proxy({name_ty}), Proxy : Proxy({payability}), Proxy : Proxy({args_ty}), Proxy : Proxy({rets_ty}), {})",
        method.name
    ))
}

fn method_args_ty(method: &DispatchMethod<'_>) -> Option<String> {
    Some(tuple_type(
        method
            .inputs
            .iter()
            .map(abi_param_type)
            .collect::<Option<Vec<_>>>()?,
    ))
}

fn method_rets_ty(method: &DispatchMethod<'_>) -> Option<String> {
    Some(tuple_type(
        method
            .outputs
            .iter()
            .map(abi_param_type)
            .collect::<Option<Vec<_>>>()?,
    ))
}

fn method_selector_expr(method: &DispatchMethod<'_>) -> Option<String> {
    Some(format!(
        "selector_matches_const(bytes4(0x{}))",
        selector_hex(method.selector.0)
    ))
}

fn fallback_expr(db: &dyn Db, fallback: &DispatchFallback<'_>) -> Option<String> {
    match fallback {
        DispatchFallback::Default => Some(
            "Fallback(Proxy : Proxy(NonPayable), Proxy : Proxy(()), Proxy : Proxy(()), fallback_default_implementation)"
                .to_owned(),
        ),
        DispatchFallback::Explicit { def, payable, .. } => {
            let payability = if *payable { "Payable" } else { "NonPayable" };
            let name = def.name(db)?;
            Some(format!(
                "Fallback(Proxy : Proxy({payability}), Proxy : Proxy(()), Proxy : Proxy(()), {name})"
            ))
        }
    }
}

fn dispatch_name_ty(contract: &str, method: &str) -> String {
    format!("DispatchNameTy_{contract}_{method}")
}

fn tuple_type(elems: Vec<String>) -> String {
    match elems.as_slice() {
        [] => "()".to_owned(),
        [one] => one.clone(),
        [head, tail @ ..] => format!("({}, {})", head, tuple_type(tail.to_vec())),
    }
}

fn abi_param_type(param: &AbiParam) -> Option<String> {
    match &param.ty {
        AbiType::Uint256 => Some("uint256".to_owned()),
        AbiType::Bool => Some("bool".to_owned()),
        AbiType::String => Some("memory(string)".to_owned()),
        AbiType::Unit => Some("()".to_owned()),
        AbiType::Tuple => Some(tuple_type(
            param
                .components
                .iter()
                .map(abi_param_type)
                .collect::<Option<Vec<_>>>()?,
        )),
        AbiType::Named(name) if name == "bytes" => Some("memory(bytes)".to_owned()),
        AbiType::Named(name) if name == "string" => Some("memory(string)".to_owned()),
        AbiType::Named(name) => Some(name.clone()),
        AbiType::Unsupported => None,
    }
}

fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn selector_hex(selector: [u8; 4]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        selector[0], selector[1], selector[2], selector[3]
    )
}
