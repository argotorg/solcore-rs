use std::fmt::Write as _;

use hir::ast::{
    function::FunctionMutability,
    item::{ContractDef, Module},
};

use super::{
    abi::{AbiParam, AbiType, abi_params_contain_unsupported},
    dispatch::{DispatchConstructor, DispatchFallback, contract_dispatch_surface},
};
use crate::Db;

/// Renders an ABI JSON document mirroring the reference `contractAbiJson`
/// behavior: explicit constructors and user-defined fallbacks are included,
/// while the implicit runtime defaults remain a dispatch-surface detail.
pub fn contract_abi_json<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    contract: ContractDef<'db>,
) -> Result<String, String> {
    let surface = contract_dispatch_surface(db, module, contract);
    if surface.methods.iter().any(|method| {
        abi_params_contain_unsupported(&method.inputs)
            || abi_params_contain_unsupported(&method.outputs)
    }) || matches!(
        &surface.constructor,
        DispatchConstructor::Explicit { inputs, .. }
            if abi_params_contain_unsupported(inputs)
    ) || matches!(
        &surface.fallback,
        DispatchFallback::Explicit {
            inputs, outputs, ..
        } if abi_params_contain_unsupported(inputs)
            || abi_params_contain_unsupported(outputs)
    ) {
        return Err("cannot represent unsupported type in ABI".to_owned());
    }
    let mut entries = Vec::new();
    if let DispatchConstructor::Explicit {
        source_index,
        inputs,
        payable,
    } = surface.constructor
    {
        entries.push((source_index, AbiJsonEntry::Constructor { inputs, payable }));
    }
    for method in surface.methods {
        entries.push((
            method.source_index,
            AbiJsonEntry::Function {
                name: method.name,
                inputs: method.inputs,
                outputs: method.outputs,
                mutability: method.mutability,
            },
        ));
    }
    if let DispatchFallback::Explicit {
        source_index,
        payable,
        ..
    } = surface.fallback
    {
        entries.push((source_index, AbiJsonEntry::Fallback { payable }));
    }
    entries.sort_by_key(|(source_index, _)| *source_index);
    let entries = entries
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    render_abi_json(&entries)
}

enum AbiJsonEntry {
    Function {
        name: String,
        inputs: Vec<AbiParam>,
        outputs: Vec<AbiParam>,
        mutability: Option<FunctionMutability>,
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
            mutability,
        } => {
            line(out, ind, "{");
            render_named_params(out, ind + 1, "inputs", inputs, true)?;
            line(out, ind + 1, &format!("\"name\": {},", json_string(name)));
            render_named_params(out, ind + 1, "outputs", outputs, true)?;
            line(
                out,
                ind + 1,
                &format!(
                    "\"stateMutability\": \"{}\",",
                    function_state_mutability(*mutability)
                ),
            );
            line(out, ind + 1, "\"type\": \"function\"");
            push_close_brace(out, ind);
        }
        AbiJsonEntry::Constructor { inputs, payable } => {
            line(out, ind, "{");
            render_named_params(out, ind + 1, "inputs", inputs, true)?;
            line(
                out,
                ind + 1,
                &format!("\"stateMutability\": \"{}\",", payability_state(*payable)),
            );
            line(out, ind + 1, "\"type\": \"constructor\"");
            push_close_brace(out, ind);
        }
        AbiJsonEntry::Fallback { payable } => {
            line(out, ind, "{");
            line(
                out,
                ind + 1,
                &format!("\"stateMutability\": \"{}\",", payability_state(*payable)),
            );
            line(out, ind + 1, "\"type\": \"fallback\"");
            push_close_brace(out, ind);
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
    if params.iter().any(abi_param_is_unsupported) {
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
        render_abi_param(out, ind + 1, param)?;
    }
    out.push('\n');
    line(
        out,
        ind,
        &format!("]{}", if trailing_comma { "," } else { "" }),
    );
    Ok(())
}

fn render_abi_param(out: &mut String, ind: usize, param: &AbiParam) -> Result<(), String> {
    let ty = param.ty.to_string();
    line(out, ind, "{");
    line(
        out,
        ind + 1,
        &format!("\"internalType\": {},", json_string(&ty)),
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
            json_string(&ty),
            if param.components.is_empty() { "" } else { "," }
        ),
    );
    if !param.components.is_empty() {
        render_named_params(out, ind + 1, "components", &param.components, false)?;
    }
    push_close_brace(out, ind);
    Ok(())
}

fn abi_param_is_unsupported(param: &AbiParam) -> bool {
    matches!(&param.ty, AbiType::Unsupported)
        || param.components.iter().any(abi_param_is_unsupported)
}

fn payability_state(payable: bool) -> &'static str {
    if payable { "payable" } else { "nonpayable" }
}

fn function_state_mutability(mutability: Option<FunctionMutability>) -> &'static str {
    match mutability {
        None => "nonpayable",
        Some(FunctionMutability::Pure) => "pure",
        Some(FunctionMutability::View) => "view",
        Some(FunctionMutability::Payable) => "payable",
    }
}

fn line(out: &mut String, ind: usize, text: &str) {
    push_indent(out, ind);
    out.push_str(text);
    out.push('\n');
}

fn push_close_brace(out: &mut String, ind: usize) {
    push_indent(out, ind);
    out.push('}');
}

fn push_indent(out: &mut String, ind: usize) {
    for _ in 0..ind {
        out.push_str("  ");
    }
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
