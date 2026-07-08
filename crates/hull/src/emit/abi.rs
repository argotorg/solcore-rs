use super::*;
use hir_ty::AbiType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AbiWordKind {
    Plain,
    Address,
    Bool,
}

#[derive(Debug, Clone)]
pub(super) struct StaticAbiLayout<'db> {
    ty: Ty<'db>,
    pub(super) slots: usize,
    kind: StaticAbiLayoutKind<'db>,
}

#[derive(Debug, Clone)]
enum StaticAbiLayoutKind<'db> {
    Unit,
    Word(AbiWordKind),
    Product(Vec<StaticAbiLayout<'db>>),
    Sum {
        lhs: Box<StaticAbiLayout<'db>>,
        rhs: Box<StaticAbiLayout<'db>>,
    },
}

pub(super) fn constructor_inputs_are_static_word(contract: &MonoContract<'_>) -> bool {
    contract
        .constructor
        .inputs
        .iter()
        .all(abi_param_is_static_word)
}

pub(super) fn dispatcher_input_layouts<'db>(
    function: &Function<'db>,
    inputs: &[MonoAbiParam],
) -> Option<Vec<StaticAbiLayout<'db>>> {
    function
        .args
        .iter()
        .zip(inputs)
        .map(|(arg, param)| static_abi_layout_for_param(&arg.ty, param))
        .collect()
}

pub(super) fn dispatcher_return_layout<'db>(
    ret: &Ty<'db>,
    outputs: &[MonoAbiParam],
) -> Option<StaticAbiLayout<'db>> {
    match outputs.len() {
        0 if matches!(ret.strip_named().kind, TyKind::Unit) => Some(StaticAbiLayout {
            ty: ret.clone(),
            slots: 0,
            kind: StaticAbiLayoutKind::Unit,
        }),
        0 => None,
        1 => static_abi_layout_for_param(ret, &outputs[0]),
        count => {
            let components = product_component_tys(ret.clone(), count)?;
            let layouts = components
                .iter()
                .zip(outputs)
                .map(|(component, output)| static_abi_layout_for_param(component, output))
                .collect::<Option<Vec<_>>>()?;
            Some(static_abi_product_layout(ret.clone(), layouts))
        }
    }
}

fn static_abi_layout_for_param<'db>(
    ty: &Ty<'db>,
    param: &MonoAbiParam,
) -> Option<StaticAbiLayout<'db>> {
    if abi_param_is_dynamic(param) {
        return None;
    }
    if matches!(&param.ty, AbiType::Tuple) {
        return static_abi_tuple_layout(ty, &param.components);
    }
    if !param.components.is_empty() {
        return None;
    }
    if abi_param_is_bool(param) {
        if hull_ty_is_bool_word(ty) {
            return Some(StaticAbiLayout {
                ty: ty.clone(),
                slots: 1,
                kind: StaticAbiLayoutKind::Word(AbiWordKind::Bool),
            });
        }
        return None;
    }
    if abi_param_is_address(param) {
        if hull_ty_word_slots(ty) == Some(1) && !hull_ty_is_bool_word(ty) {
            return Some(StaticAbiLayout {
                ty: ty.clone(),
                slots: 1,
                kind: StaticAbiLayoutKind::Word(AbiWordKind::Address),
            });
        }
        return None;
    }
    static_abi_layout_from_ty(ty)
}

fn static_abi_tuple_layout<'db>(
    ty: &Ty<'db>,
    components: &[MonoAbiParam],
) -> Option<StaticAbiLayout<'db>> {
    let component_tys = product_component_tys(ty.clone(), components.len())?;
    let layouts = component_tys
        .iter()
        .zip(components)
        .map(|(component, param)| static_abi_layout_for_param(component, param))
        .collect::<Option<Vec<_>>>()?;
    Some(static_abi_product_layout(ty.clone(), layouts))
}

fn static_abi_layout_from_ty<'db>(ty: &Ty<'db>) -> Option<StaticAbiLayout<'db>> {
    match &ty.strip_named().kind {
        TyKind::Unit => Some(StaticAbiLayout {
            ty: ty.clone(),
            slots: 0,
            kind: StaticAbiLayoutKind::Unit,
        }),
        TyKind::Word => Some(StaticAbiLayout {
            ty: ty.clone(),
            slots: 1,
            kind: StaticAbiLayoutKind::Word(AbiWordKind::Plain),
        }),
        TyKind::Bool => Some(StaticAbiLayout {
            ty: ty.clone(),
            slots: 1,
            kind: StaticAbiLayoutKind::Word(AbiWordKind::Bool),
        }),
        TyKind::Product(_, _) => {
            let mut layouts = Vec::new();
            collect_static_abi_product_layouts(ty, &mut layouts)?;
            Some(static_abi_product_layout(ty.clone(), layouts))
        }
        TyKind::Sum(lhs, rhs) => {
            let lhs = static_abi_layout_from_ty(lhs)?;
            let rhs = static_abi_layout_from_ty(rhs)?;
            let slots = 1 + lhs.slots.max(rhs.slots);
            Some(StaticAbiLayout {
                ty: ty.clone(),
                slots,
                kind: StaticAbiLayoutKind::Sum {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            })
        }
        TyKind::Named { inner, .. } => static_abi_layout_from_ty(inner),
        TyKind::NamedRef { .. } => None,
        TyKind::Function { .. } => None,
    }
}

fn collect_static_abi_product_layouts<'db>(
    ty: &Ty<'db>,
    out: &mut Vec<StaticAbiLayout<'db>>,
) -> Option<()> {
    match &ty.strip_named().kind {
        TyKind::Product(lhs, rhs) => {
            out.push(static_abi_layout_from_ty(lhs)?);
            collect_static_abi_product_layouts(rhs, out)?;
        }
        _ => out.push(static_abi_layout_from_ty(ty)?),
    }
    Some(())
}

fn static_abi_product_layout<'db>(
    ty: Ty<'db>,
    layouts: Vec<StaticAbiLayout<'db>>,
) -> StaticAbiLayout<'db> {
    let slots = layouts.iter().map(|layout| layout.slots).sum();
    StaticAbiLayout {
        ty,
        slots,
        kind: StaticAbiLayoutKind::Product(layouts),
    }
}

fn abi_param_is_dynamic(param: &MonoAbiParam) -> bool {
    matches!(&param.ty, AbiType::String)
        || matches!(&param.ty, AbiType::Named(name) if matches!(name.as_str(), "string" | "bytes"))
        || param.components.iter().any(abi_param_is_dynamic)
}

fn abi_param_is_static_word(param: &specialize::MonoAbiParam) -> bool {
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

pub(super) fn selector_hex(selector: [u8; 4]) -> String {
    format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        selector[0], selector[1], selector[2], selector[3]
    )
}

pub(super) fn abi_words_to_expr<'db>(
    span: Span<'db>,
    layout: &StaticAbiLayout<'db>,
    names: &[String],
) -> Expr<'db> {
    match &layout.kind {
        StaticAbiLayoutKind::Unit => {
            let mut expr = Expr::unit(span);
            expr.ty = layout.ty.clone();
            expr
        }
        StaticAbiLayoutKind::Word(kind) => {
            let word = Expr::var(span, names[0].clone(), Ty::word(span));
            match kind {
                AbiWordKind::Bool => abi_word_to_bool_expr(span, word, layout.ty.clone()),
                AbiWordKind::Plain | AbiWordKind::Address => {
                    let mut expr = word;
                    expr.ty = layout.ty.clone();
                    expr
                }
            }
        }
        StaticAbiLayoutKind::Product(layouts) => {
            let mut offset = 0;
            let mut elems = Vec::new();
            for component in layouts {
                let end = offset + component.slots;
                elems.push(abi_words_to_expr(span, component, &names[offset..end]));
                offset = end;
            }
            product_expr(span, layout.ty.clone(), elems)
        }
        StaticAbiLayoutKind::Sum { lhs, rhs } => {
            let tag = Expr::var(span, names[0].clone(), Ty::word(span));
            let payload = &names[1..];
            let lhs_expr = abi_words_to_expr(span, lhs, &payload[..lhs.slots]);
            let rhs_expr = abi_words_to_expr(span, rhs, &payload[..rhs.slots]);
            Expr {
                span,
                ty: layout.ty.clone(),
                kind: ExprKind::If {
                    target: layout.ty.clone(),
                    cond: Box::new(Expr {
                        span,
                        ty: bool_sum_ty(span),
                        kind: ExprKind::Call {
                            callee: "primEqWord".to_owned(),
                            args: vec![tag, Expr::word(span, "0")],
                        },
                    }),
                    then_expr: Box::new(Expr {
                        span,
                        ty: layout.ty.clone(),
                        kind: ExprKind::Inl {
                            target: layout.ty.clone(),
                            value: Box::new(lhs_expr),
                        },
                    }),
                    else_expr: Box::new(Expr {
                        span,
                        ty: layout.ty.clone(),
                        kind: ExprKind::Inr {
                            target: layout.ty.clone(),
                            value: Box::new(rhs_expr),
                        },
                    }),
                },
            }
        }
    }
}

pub(super) fn write_expr_to_abi_slots<'db>(
    span: Span<'db>,
    value: Expr<'db>,
    layout: &StaticAbiLayout<'db>,
    names: &[String],
    body: &mut Vec<Stmt<'db>>,
) {
    match &layout.kind {
        StaticAbiLayoutKind::Unit => {}
        StaticAbiLayoutKind::Word(kind) => {
            let rhs = match kind {
                AbiWordKind::Bool if hull_ty_is_bool_word(&value.ty) => {
                    abi_bool_to_word_expr(span, value)
                }
                AbiWordKind::Plain | AbiWordKind::Address | AbiWordKind::Bool => {
                    let mut value = value;
                    value.ty = Ty::word(span);
                    value
                }
            };
            body.push(assign_abi_word_slot(span, &names[0], rhs));
        }
        StaticAbiLayoutKind::Product(layouts) => {
            let fields = layouts
                .iter()
                .map(|layout| layout.ty.clone())
                .collect::<Vec<_>>();
            let components = product_field_exprs(value, &fields);
            let mut offset = 0;
            for (component, layout) in components.into_iter().zip(layouts) {
                let end = offset + layout.slots;
                write_expr_to_abi_slots(span, component, layout, &names[offset..end], body);
                offset = end;
            }
        }
        StaticAbiLayoutKind::Sum { lhs, rhs } => {
            let tag_name = names[0].clone();
            let payload_names = &names[1..];
            let lhs_binder = format!("{tag_name}_inl");
            let rhs_binder = format!("{tag_name}_inr");

            let mut lhs_body = vec![assign_abi_word_slot(span, &tag_name, Expr::word(span, "0"))];
            write_expr_to_abi_slots(
                span,
                Expr::var(span, lhs_binder.clone(), lhs.ty.clone()),
                lhs,
                &payload_names[..lhs.slots],
                &mut lhs_body,
            );

            let mut rhs_body = vec![assign_abi_word_slot(span, &tag_name, Expr::word(span, "1"))];
            write_expr_to_abi_slots(
                span,
                Expr::var(span, rhs_binder.clone(), rhs.ty.clone()),
                rhs,
                &payload_names[..rhs.slots],
                &mut rhs_body,
            );

            body.push(Stmt {
                span,
                kind: StmtKind::Match {
                    target: layout.ty.clone(),
                    scrutinee: value,
                    alts: vec![
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inl),
                            },
                            binder: lhs_binder,
                            body: lhs_body,
                        },
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inr),
                            },
                            binder: rhs_binder,
                            body: rhs_body,
                        },
                    ],
                },
            });
        }
    }
}

fn assign_abi_word_slot<'db>(span: Span<'db>, name: &str, rhs: Expr<'db>) -> Stmt<'db> {
    Stmt {
        span,
        kind: StmtKind::Assign {
            lhs: Expr::var(span, name.to_owned(), Ty::word(span)),
            rhs,
        },
    }
}

pub(super) fn abi_layout_slot_kinds(layout: &StaticAbiLayout<'_>) -> Vec<AbiWordKind> {
    match &layout.kind {
        StaticAbiLayoutKind::Unit => Vec::new(),
        StaticAbiLayoutKind::Word(kind) => vec![*kind],
        StaticAbiLayoutKind::Product(layouts) => {
            layouts.iter().flat_map(abi_layout_slot_kinds).collect()
        }
        StaticAbiLayoutKind::Sum { lhs, rhs } => {
            let mut kinds = vec![AbiWordKind::Plain];
            kinds.extend((0..lhs.slots.max(rhs.slots)).map(|_| AbiWordKind::Plain));
            kinds
        }
    }
}

pub(super) fn numbered_name(prefix: &str, index: usize, count: usize) -> String {
    if count == 1 {
        prefix.to_owned()
    } else {
        format!("{prefix}_{index}")
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
                    callee: "primEqWord".to_owned(),
                    args: vec![word, Expr::word(span, "0")],
                },
            }),
            then_expr: Box::new(bool_expr(span, target.clone(), false)),
            else_expr: Box::new(bool_expr(span, target, true)),
        },
    }
}

fn abi_bool_to_word_expr<'db>(span: Span<'db>, value: Expr<'db>) -> Expr<'db> {
    Expr {
        span,
        ty: Ty::word(span),
        kind: ExprKind::If {
            target: Ty::word(span),
            cond: Box::new(value),
            then_expr: Box::new(Expr::word(span, "1")),
            else_expr: Box::new(Expr::word(span, "0")),
        },
    }
}

fn product_component_tys<'db>(ty: Ty<'db>, count: usize) -> Option<Vec<Ty<'db>>> {
    if count <= 1 {
        return Some(vec![ty]);
    }
    match ty.strip_named().kind.clone() {
        TyKind::Product(lhs, rhs) => {
            let mut out = vec![*lhs];
            out.extend(product_component_tys(*rhs, count - 1)?);
            Some(out)
        }
        _ => None,
    }
}
