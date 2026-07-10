use hir_ty::AbiType;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AbiWordKind {
    Plain,
    Address,
    Bool,
}

pub(super) fn constructor_inputs_are_static_word(contract: &MonoContract<'_>) -> bool {
    contract
        .constructor
        .inputs
        .iter()
        .all(abi_param_is_static_word)
}

fn abi_param_is_static_word(param: &MonoAbiParam) -> bool {
    param.components.is_empty()
        && (matches!(&param.ty, AbiType::Uint256 | AbiType::Bool)
            || matches!(
                &param.ty,
                AbiType::Named(name)
                    if matches!(
                        name.as_str(),
                        "uint256" | "uint" | "word" | "bytes32" | "address" | "bool"
                    )
            ))
}

fn abi_param_is_address(param: &MonoAbiParam) -> bool {
    param.components.is_empty() && matches!(&param.ty, AbiType::Named(name) if name == "address")
}

fn abi_param_is_bool(param: &MonoAbiParam) -> bool {
    param.components.is_empty()
        && (matches!(&param.ty, AbiType::Bool)
            || matches!(&param.ty, AbiType::Named(name) if name == "bool"))
}

pub(super) fn abi_word_kind(param: &MonoAbiParam) -> AbiWordKind {
    if abi_param_is_address(param) {
        AbiWordKind::Address
    } else if abi_param_is_bool(param) {
        AbiWordKind::Bool
    } else {
        AbiWordKind::Plain
    }
}

pub(super) fn abi_word_to_bool_expr<'db>(
    span: Span<'db>,
    word: Expr<'db>,
    target: Ty<'db>,
) -> Expr<'db> {
    Expr {
        span,
        ty: target.clone(),
        kind: ExprKind::If {
            target: target.clone(),
            cond: Box::new(Expr {
                span,
                ty: bool_sum_ty(span),
                kind: ExprKind::Call {
                    callee: "primEqWord".into(),
                    args: vec![word, Expr::word(span, "0")],
                },
            }),
            then_expr: Box::new(bool_expr(span, target.clone(), false)),
            else_expr: Box::new(bool_expr(span, target, true)),
        },
    }
}
