use hir::diag::Diagnostic;

use crate::{BuiltinTyCtor, Db, Ty, TyCtor, TyKind};

/// ABI parameter or tuple component.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AbiParam {
    /// Parameter name. Outputs and tuple components use the empty name,
    /// matching the reference ABI emitter.
    pub name: String,
    /// Canonical ABI type string.
    pub ty: String,
    /// Tuple components, if `ty == "tuple"`.
    pub components: Vec<AbiParam>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct AbiSelector(pub [u8; 4]);

impl AbiSelector {
    pub fn to_hex(self) -> String {
        format!(
            "0x{:02x}{:02x}{:02x}{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

/// Interned ABI signature preimage used as the selector query key.
#[salsa::interned(debug)]
pub struct AbiSignature<'db> {
    /// Canonical signature, e.g. `transfer(address,uint256)`.
    #[returns(ref)]
    pub text: String,
}

/// Computes the ABI selector for a canonical signature.
#[salsa::tracked]
pub fn abi_selector<'db>(db: &'db dyn Db, signature: AbiSignature<'db>) -> AbiSelector {
    let hash = hir::keccak::keccak256(signature.text(db).as_bytes());
    AbiSelector([hash[0], hash[1], hash[2], hash[3]])
}

pub(super) fn method_signature_string<'db>(
    db: &'db dyn Db,
    name: &str,
    params: &[Ty<'db>],
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(name);
    out.push('(');
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&signature_type_string(db, *param)?);
    }
    out.push(')');
    Ok(out)
}

fn signature_type_string<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Result<String, String> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty() => Ok("uint256".to_owned()),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
            args,
        } if args.is_empty() => Ok("bool".to_owned()),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::String),
            args,
        } if args.is_empty() => Ok("string".to_owned()),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Ok(String::new()),
        TyKind::Tuple(elems) => tuple_signature_string(db, elems),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => tuple_signature_string(db, args),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if user
            .def
            .name(db)
            .as_deref()
            .is_some_and(is_transparent_abi_location)
            && args.len() == 1 =>
        {
            signature_type_string(db, args[0])
        }
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if args.is_empty() => Ok(user
            .def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", user.kind))),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => Err(ty.display(db)),
        TyKind::Named { .. } | TyKind::Function { .. } | TyKind::Comptime(_) => Err(ty.display(db)),
    }
}

fn tuple_signature_string<'db>(db: &'db dyn Db, elems: &[Ty<'db>]) -> Result<String, String> {
    let mut parts = Vec::new();
    for elem in flatten_tuple(db, elems) {
        parts.push(signature_type_string(db, elem)?);
    }
    Ok(parts.join(","))
}

pub(super) fn abi_params<'db>(
    db: &'db dyn Db,
    names: &[String],
    tys: &[Ty<'db>],
    diagnostics: &mut Vec<Diagnostic>,
    span: hir::span::Span<'db>,
) -> Vec<AbiParam> {
    tys.iter()
        .enumerate()
        .map(|(index, ty)| {
            match abi_param(db, names.get(index).cloned().unwrap_or_default(), *ty) {
                Ok(param) => param,
                Err(err) => {
                    diagnostics.push(contract_diag_unsupported_abi_type(
                        db,
                        span,
                        "ABI parameter",
                        &err,
                    ));
                    AbiParam {
                        name: names.get(index).cloned().unwrap_or_default(),
                        ty: "<unsupported>".to_owned(),
                        components: Vec::new(),
                    }
                }
            }
        })
        .collect()
}

pub(super) fn abi_outputs<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    diagnostics: &mut Vec<Diagnostic>,
    span: hir::span::Span<'db>,
) -> Vec<AbiParam> {
    if is_unit_ty(db, ty) {
        return Vec::new();
    }
    flatten_output_ty(db, ty)
        .into_iter()
        .map(|ty| match abi_param(db, String::new(), ty) {
            Ok(param) => param,
            Err(err) => {
                diagnostics.push(contract_diag_unsupported_abi_type(
                    db,
                    span,
                    "ABI output",
                    &err,
                ));
                AbiParam {
                    name: String::new(),
                    ty: "<unsupported>".to_owned(),
                    components: Vec::new(),
                }
            }
        })
        .collect()
}

fn abi_param<'db>(db: &'db dyn Db, name: String, ty: Ty<'db>) -> Result<AbiParam, String> {
    let (ty, components) = abi_type_of(db, ty)?;
    Ok(AbiParam {
        name,
        ty,
        components,
    })
}

fn abi_type_of<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Result<(String, Vec<AbiParam>), String> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty() => Ok(("uint256".to_owned(), Vec::new())),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
            args,
        } if args.is_empty() => Ok(("bool".to_owned(), Vec::new())),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::String),
            args,
        } if args.is_empty() => Ok(("string".to_owned(), Vec::new())),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Ok(("".to_owned(), Vec::new())),
        TyKind::Tuple(elems) if elems.is_empty() => Ok(("".to_owned(), Vec::new())),
        TyKind::Tuple(elems) => Ok((
            "tuple".to_owned(),
            flatten_tuple(db, elems)
                .into_iter()
                .map(|elem| abi_param(db, String::new(), elem))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => Ok((
            "tuple".to_owned(),
            flatten_tuple(db, args)
                .into_iter()
                .map(|elem| abi_param(db, String::new(), elem))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if user
            .def
            .name(db)
            .as_deref()
            .is_some_and(is_transparent_abi_location)
            && args.len() == 1 =>
        {
            abi_type_of(db, args[0])
        }
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if args.is_empty() => Ok((
            user.def
                .name(db)
                .unwrap_or_else(|| format!("{:?}", user.kind)),
            Vec::new(),
        )),
        _ => Err(ty.display(db)),
    }
}

fn flatten_output_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Vec<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Tuple(elems) => flatten_tuple(db, elems),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => flatten_tuple(db, args),
        _ => vec![ty],
    }
}

fn flatten_tuple<'db>(db: &'db dyn Db, elems: &[Ty<'db>]) -> Vec<Ty<'db>> {
    let mut out = Vec::new();
    for elem in elems {
        match elem.kind(db) {
            TyKind::Tuple(nested) => out.extend(flatten_tuple(db, nested)),
            TyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => out.extend(flatten_tuple(db, args)),
            _ => out.push(*elem),
        }
    }
    out
}

fn is_unit_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Tuple(elems) if elems.is_empty()
    ) || matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty()
    )
}

fn is_transparent_abi_location(name: &str) -> bool {
    matches!(name, "memory" | "calldata")
}

pub(super) fn contract_diag_unsupported_abi_type<'db>(
    db: &'db dyn Db,
    span: hir::span::Span<'db>,
    context: &str,
    ty: &str,
) -> Diagnostic {
    Diagnostic::error(format!("{context} cannot be represented in the ABI: {ty}"))
        .with_code("SC0231")
        .with_primary_label(db, span, Some("unsupported ABI type"))
}
