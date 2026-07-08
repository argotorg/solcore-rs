use hull::{Con, Ty as HullTy, TyKind};

use crate::ast::{Expr, Literal, Stmt};

use super::{
    TranslationError,
    names::{stack_name, yul_var_name},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Location {
    Word(String),
    Bool(bool),
    Stack(usize),
    Named(String),
    Seq(Vec<Location>),
    Empty(usize),
}

pub(super) fn is_word_type(ty: &HullTy<'_>) -> bool {
    matches!(ty.strip_named().kind, TyKind::Word)
}

pub(super) fn zero_sized_type(ty: &HullTy<'_>) -> bool {
    size_of_ty(ty).is_ok_and(|size| size == 0)
}

pub(super) fn lower_in_k_loc(
    target: &HullTy<'_>,
    index: usize,
    payload: Location,
) -> Result<Location, TranslationError> {
    match &target.strip_named().kind {
        TyKind::Named { inner, .. } => lower_in_k_loc(inner, index, payload),
        TyKind::Sum(lhs, rhs) if index == 0 => {
            let padded = pad_to_size(payload, size_of_ty(lhs)?.max(size_of_ty(rhs)?));
            Ok(Location::Seq(vec![Location::Bool(false), padded]))
        }
        TyKind::Sum(lhs, rhs) => {
            let nested = lower_in_k_loc(rhs, index - 1, payload)?;
            let padded = pad_to_size(nested, size_of_ty(lhs)?.max(size_of_ty(rhs)?));
            Ok(Location::Seq(vec![Location::Bool(true), padded]))
        }
        _ if index == 0 => Ok(payload),
        _ => Err(TranslationError::new(format!(
            "bad injection index {index} for non-sum target"
        ))),
    }
}

pub(super) fn size_of_ty(ty: &HullTy<'_>) -> Result<usize, TranslationError> {
    match &ty.strip_named().kind {
        TyKind::Word | TyKind::Bool | TyKind::NamedRef { .. } | TyKind::Function { .. } => Ok(1),
        TyKind::Unit => Ok(0),
        TyKind::Product(lhs, rhs) => Ok(size_of_ty(lhs)? + size_of_ty(rhs)?),
        TyKind::Sum(lhs, rhs) => Ok(1 + size_of_ty(lhs)?.max(size_of_ty(rhs)?)),
        TyKind::Named { inner, .. } => size_of_ty(inner),
    }
}

pub(super) fn size_of_loc(loc: &Location) -> usize {
    match loc {
        Location::Empty(size) => *size,
        Location::Seq(locs) => locs.iter().map(size_of_loc).sum(),
        _ => 1,
    }
}

pub(super) fn alloc_loc(loc: &Location) -> Vec<Stmt> {
    stack_slots(loc)
        .into_iter()
        .map(|index| Stmt::Let {
            names: vec![stack_name(index)],
            init: None,
        })
        .collect()
}

fn stack_slots(loc: &Location) -> Vec<usize> {
    match loc {
        Location::Stack(index) => vec![*index],
        Location::Seq(locs) => locs.iter().flat_map(stack_slots).collect(),
        _ => Vec::new(),
    }
}

pub(super) fn flatten_rhs(loc: &Location) -> Vec<Expr> {
    match loc {
        Location::Word(value) => vec![Expr::number(value.clone())],
        Location::Bool(value) => vec![Expr::bool(*value)],
        Location::Stack(index) => vec![Expr::ident(stack_name(*index))],
        Location::Named(name) => vec![Expr::ident(yul_var_name(name))],
        Location::Seq(locs) => locs.iter().flat_map(flatten_rhs).collect(),
        Location::Empty(size) => (0..*size).map(|_| Expr::number("911")).collect(),
    }
}

pub(super) fn flatten_lhs(loc: &Location) -> Result<Vec<String>, TranslationError> {
    match loc {
        Location::Stack(index) => Ok(vec![stack_name(*index)]),
        Location::Named(name) => Ok(vec![yul_var_name(name)]),
        Location::Seq(locs) => locs
            .iter()
            .map(flatten_lhs)
            .collect::<Result<Vec<_>, _>>()
            .map(|chunks| chunks.into_iter().flatten().collect()),
        other => Err(TranslationError::new(format!(
            "cannot use location as assignment target: {other:?}"
        ))),
    }
}

pub(super) fn load_loc(loc: &Location) -> Result<Expr, TranslationError> {
    match loc {
        Location::Word(value) => Ok(Expr::number(value.clone())),
        Location::Bool(value) => Ok(Expr::bool(*value)),
        Location::Stack(index) => Ok(Expr::ident(stack_name(*index))),
        Location::Named(name) => Ok(Expr::ident(yul_var_name(name))),
        Location::Empty(_) => Ok(Expr::number("911")),
        Location::Seq(_) => Err(TranslationError::new(format!(
            "cannot load location: {loc:?}"
        ))),
    }
}

pub(super) fn copy_locs(lhs: &Location, rhs: &Location) -> Result<Vec<Stmt>, TranslationError> {
    if matches!(lhs, Location::Seq(_)) || matches!(rhs, Location::Seq(_)) {
        let lhs = flatten_locs(lhs);
        let rhs = flatten_locs(rhs);
        if lhs.len() != rhs.len() {
            return Err(TranslationError::new(format!(
                "location copy arity mismatch: lhs={} rhs={}",
                lhs.len(),
                rhs.len()
            )));
        }
        return lhs
            .into_iter()
            .zip(rhs)
            .map(|(lhs, rhs)| copy_locs(&lhs, &rhs))
            .collect::<Result<Vec<_>, _>>()
            .map(|chunks| chunks.into_iter().flatten().collect());
    }

    match (lhs, rhs) {
        (Location::Stack(_), Location::Empty(_)) | (Location::Named(_), Location::Empty(_)) => {
            Ok(Vec::new())
        }
        (Location::Stack(index), rhs) => Ok(vec![Stmt::Assign {
            names: vec![stack_name(*index)],
            value: load_loc(rhs)?,
        }]),
        (Location::Named(name), rhs) => Ok(vec![Stmt::Assign {
            names: vec![yul_var_name(name)],
            value: load_loc(rhs)?,
        }]),
        _ => Err(TranslationError::new(format!(
            "location copy mismatch: lhs={lhs:?} rhs={rhs:?}"
        ))),
    }
}

fn flatten_locs(loc: &Location) -> Vec<Location> {
    match loc {
        Location::Empty(size) => (0..*size).map(|_| Location::Empty(1)).collect(),
        Location::Seq(locs) => locs.iter().flat_map(flatten_locs).collect(),
        loc => vec![loc.clone()],
    }
}

pub(super) fn normalize_loc(loc: Location) -> Location {
    match loc {
        Location::Seq(_) => {
            let flattened = flatten_locs(&loc);
            match flattened.as_slice() {
                [one] => one.clone(),
                _ => Location::Seq(flattened),
            }
        }
        loc => loc,
    }
}

pub(super) fn pair_locs(loc: Location) -> Result<(Location, Location), TranslationError> {
    match loc {
        Location::Seq(locs) => match <[Location; 2]>::try_from(locs) {
            Ok([lhs, rhs]) => Ok((lhs, rhs)),
            Err(locs) => Err(TranslationError::new(format!(
                "expected product location, got {:?}",
                Location::Seq(locs)
            ))),
        },
        loc => Err(TranslationError::new(format!(
            "expected product location, got {loc:?}"
        ))),
    }
}

pub(super) fn pad_to_size(loc: Location, size: usize) -> Location {
    let padding = size.saturating_sub(size_of_loc(&loc));
    if padding == 0 {
        loc
    } else {
        Location::Seq(vec![loc, Location::Empty(padding)])
    }
}

fn reshape_loc<'db>(ty: &HullTy<'db>, loc: &Location) -> Result<Location, TranslationError> {
    fn go<'db>(
        ty: &HullTy<'db>,
        slots: &[Location],
    ) -> Result<(Location, usize), TranslationError> {
        match &ty.strip_named().kind {
            TyKind::Named { inner, .. } => go(inner, slots),
            TyKind::Unit => Ok((Location::Seq(Vec::new()), 0)),
            TyKind::Product(lhs, rhs) => {
                let (lhs_loc, lhs_used) = go(lhs, slots)?;
                let (rhs_loc, rhs_used) = go(rhs, &slots[lhs_used..])?;
                Ok((Location::Seq(vec![lhs_loc, rhs_loc]), lhs_used + rhs_used))
            }
            _ => {
                let size = size_of_ty(ty)?;
                let here = slots.iter().take(size).cloned().collect::<Vec<_>>();
                let loc = match here.as_slice() {
                    [one] => one.clone(),
                    _ => Location::Seq(here),
                };
                Ok((loc, size))
            }
        }
    }

    let slots = flatten_locs(loc);
    let (loc, _) = go(ty, &slots)?;
    Ok(loc)
}

pub(super) fn con_payload<'db>(
    target: &HullTy<'db>,
    con: Con,
    payload: &Location,
) -> Result<Location, TranslationError> {
    match (&target.strip_named().kind, con) {
        (TyKind::Named { inner, .. }, con) => con_payload(inner, con, payload),
        (TyKind::Sum(lhs, _), Con::Inl) => reshape_loc(lhs, payload),
        (TyKind::Sum(_, rhs), Con::Inr) => reshape_loc(rhs, payload),
        (_, Con::InK(index)) => {
            let Some(ty) = nth_sum_payload(target, index) else {
                return Ok(payload.clone());
            };
            reshape_loc(&ty, payload)
        }
        _ => Ok(payload.clone()),
    }
}

fn nth_sum_payload<'db>(target: &HullTy<'db>, index: usize) -> Option<HullTy<'db>> {
    let mut current = target.strip_named();
    let mut remaining = index;
    loop {
        match &current.strip_named().kind {
            TyKind::Sum(lhs, _) if remaining == 0 => return Some((**lhs).clone()),
            TyKind::Sum(_, rhs) => {
                current = rhs.strip_named();
                remaining -= 1;
            }
            _ if remaining == 0 => return Some(current.clone()),
            _ => return None,
        }
    }
}

pub(super) fn con_lit(target: &HullTy<'_>, con: Con) -> Result<Literal, TranslationError> {
    match con {
        Con::Inl => Ok(Literal::Bool(false)),
        Con::Inr => Ok(Literal::Bool(true)),
        Con::InK(index) if matches!(target.strip_named().kind, TyKind::Sum(_, _)) => {
            Err(TranslationError::new(format!(
                "in({index}) patterns require nested binary inl/inr matches"
            )))
        }
        Con::InK(index) => Ok(Literal::Number(index.to_string())),
    }
}

pub(super) fn partition_allocs(stmts: Vec<Stmt>) -> (Vec<Stmt>, Vec<Stmt>) {
    stmts
        .into_iter()
        .partition(|stmt| matches!(stmt, Stmt::Let { init: None, .. }))
}

pub(super) fn is_unit_loc(loc: &Location) -> bool {
    matches!(loc, Location::Seq(locs) if locs.is_empty())
}
