use std::fmt::Write as _;

use hir::ast::item::{ContractDef, Module};

use crate::Db;

use super::{abi::AbiParam, dispatch::contract_dispatch_surface};

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
