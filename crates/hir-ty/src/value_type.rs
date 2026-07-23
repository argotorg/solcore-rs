//! User-defined value-type declaration lookup and representation lowering.
//!
//! Unlike transparent aliases, a value type remains nominal in semantic types.
//! This module is the single boundary where consumers intentionally recover
//! its underlying runtime representation.

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::item::{ContractItem, Item, Module, TypeAlias, TypeAliasKind},
    diag::{LabelSpan, Offset},
    nameres::{self as hir_nameres, type_var_bindings},
    span::{AnchorId, Span, Spanned},
};
use nameres::LibraryId;
use parser::parse_file_to_hir;

use crate::{
    AliasNormalizer, BinderEnv, BuiltinTyCtor, Db, Ty, TyCtor, TyKind, TypeLowering, UserTyCtorKind,
};

/// A malformed or unavailable user-defined value-type declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueTypeError {
    /// Declaration name, when it can be recovered.
    pub name: String,
    /// Declaration or definition span.
    pub span: LabelSpan,
    /// Human-readable rejection reason.
    pub reason: String,
}

struct ValueTypeInfo<'db> {
    declaration: TypeAlias<'db>,
    inherited_type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

/// Resolves a value type through the supplied module context.
///
/// Inference uses this form so standalone databases and compiler-owned HIR
/// overlays do not need a registered inter-module tree. If `def` is external
/// to `module`, lookup falls back to its registered source module.
pub fn value_type_underlying_in_context<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    def: DefId<'db>,
) -> Result<Ty<'db>, ValueTypeError> {
    if let Some(info) = find_value_type_info(db, module, def) {
        return lower_value_type_info(db, module, item_resolutions, info);
    }
    value_type_underlying(db, def)
}

/// Resolves a value type from its registered source module.
///
/// This path deliberately avoids frontend preparation/type-inference queries:
/// downstream layout and ABI code may call it while those queries are already
/// active. The module import surface provides import-aware type resolution
/// directly from parsed HIR.
pub fn value_type_underlying<'db>(
    db: &'db dyn Db,
    def: DefId<'db>,
) -> Result<Ty<'db>, ValueTypeError> {
    if let Some(module_id) = nameres::module_id_for_source_file(db, def.file(db)) {
        let env = nameres::module_import_surface(db, module_id);
        if let Some(scope) = env.item_scope.as_ref() {
            let item_resolutions =
                hir_nameres::resolve_item_type_facts_with_imports(db, scope.module, scope, &env);
            if let Some(info) = find_value_type_info(db, scope.module, def) {
                return lower_value_type_info(db, scope.module, &item_resolutions, info);
            }
        }
    }

    // Standalone analysis databases do not necessarily register a module
    // tree. Local declarations with builtin-only underlyings remain fully
    // resolvable directly from parsed HIR; imported underlyings still fail
    // safely instead of being guessed.
    let module = parse_file_to_hir(db, def.file(db)).module(db);
    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    let Some(info) = find_value_type_info(db, module, def) else {
        return Err(missing_value_type(db, def));
    };
    lower_value_type_info(db, module, &item_resolutions, info)
}

fn lower_value_type_info<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    info: ValueTypeInfo<'db>,
) -> Result<Ty<'db>, ValueTypeError> {
    let declaration = info.declaration;
    let name = declaration_name(db, declaration);
    let span = LabelSpan::from_span(db, declaration.name_elem(db).span(db));
    if !declaration.ty_param_elems(db).is_empty() {
        return Err(ValueTypeError {
            name,
            span,
            reason: "user-defined value types cannot declare type parameters".to_owned(),
        });
    }
    if !info.inherited_type_vars.is_empty() {
        return Err(ValueTypeError {
            name,
            span,
            reason:
                "a user-defined value type cannot be declared inside a generic contract context"
                    .to_owned(),
        });
    }

    let lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.inherited_type_vars),
    );
    let lowered = lowerer.lower_type_alias(declaration).ty;
    if !lowerer.take_diagnostics().is_empty() {
        return Err(ValueTypeError {
            name,
            span,
            reason: "underlying type could not be lowered".to_owned(),
        });
    }
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let underlying = normalizer.normalize_ty(lowered);
    if !normalizer.take_errors().is_empty() {
        return Err(ValueTypeError {
            name,
            span,
            reason: "underlying type contains an invalid transparent alias".to_owned(),
        });
    }
    validate_underlying(db, underlying).map_err(|reason| ValueTypeError { name, span, reason })?;
    Ok(underlying)
}

fn validate_underlying(db: &dyn Db, ty: Ty<'_>) -> Result<(), String> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word | BuiltinTyCtor::Bool),
            args,
        } if args.is_empty() => Ok(()),
        TyKind::Named {
            ctor:
                TyCtor::User(user @ crate::UserTyCtor {
                    kind: UserTyCtorKind::Adt,
                    ..
                }),
            args,
        } if args.is_empty() && is_canonical_std_elementary_value(db, user.def) => {
            Ok(())
        }
        TyKind::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    kind: UserTyCtorKind::ValueType,
                    ..
                }),
            ..
        } => Err(
            "a user-defined value type cannot use another user-defined value type as its underlying type"
                .to_owned(),
        ),
        TyKind::Function { .. } => {
            Err("function types cannot underlie a user-defined value type".to_owned())
        }
        TyKind::Tuple(_) => Err("tuple types cannot underlie a user-defined value type".to_owned()),
        TyKind::Comptime(_) | TyKind::BoundVar(_) => Err(
            "the underlying type must be a concrete runtime elementary value type".to_owned(),
        ),
        TyKind::Error | TyKind::Unknown => {
            Err("the underlying type could not be resolved".to_owned())
        }
        TyKind::Named { .. } => Err(
            "the underlying type must be `word`, `bool`, or a Solidity elementary value type"
                .to_owned(),
        ),
    }
}

/// Returns whether a valid value-type underlying has the one-word storage
/// representation currently supported by the backend.
///
/// `bool` is intentionally excluded: Hull represents it as a tagged sum, so
/// treating it as a raw storage word would cross the backend's I1/I256
/// boundary without the required encoding and validation.
pub fn value_type_underlying_has_word_storage_representation(db: &dyn Db, ty: Ty<'_>) -> bool {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } => args.is_empty(),
        TyKind::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } => args.is_empty() && is_canonical_std_elementary_value(db, *def),
        _ => false,
    }
}

fn is_canonical_std_elementary_value(db: &dyn Db, def: DefId<'_>) -> bool {
    let Some(name) = def.name(db) else {
        return false;
    };
    let Some(module) = nameres::module_id_for_source_file(db, def.file(db)) else {
        return false;
    };
    if module.library(db) != &LibraryId::Std || module.logical_path(db).as_slice() != ["std"] {
        return false;
    }
    is_solidity_elementary_value_name(&name)
}

fn is_solidity_elementary_value_name(name: &str) -> bool {
    if matches!(name, "address" | "byte") {
        return true;
    }
    if let Some(bits) = name
        .strip_prefix("uint")
        .or_else(|| name.strip_prefix("int"))
    {
        return valid_bit_width(bits, 8, 256, 8);
    }
    if let Some(bytes) = name.strip_prefix("bytes") {
        return valid_bit_width(bytes, 1, 32, 1);
    }
    false
}

fn valid_bit_width(text: &str, min: u16, max: u16, step: u16) -> bool {
    text.parse::<u16>()
        .is_ok_and(|value| value >= min && value <= max && value % step == 0)
}

fn find_value_type_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ValueTypeInfo<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_value_type_in_item(db, *item, def))
}

fn find_value_type_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
) -> Option<ValueTypeInfo<'db>> {
    match item {
        Item::TypeAlias(declaration)
            if declaration.def_id_value(db) == def
                && declaration.kind(db) == TypeAliasKind::ValueType =>
        {
            Some(ValueTypeInfo {
                declaration,
                inherited_type_vars: Vec::new(),
            })
        }
        Item::ContractDef(contract) => {
            let inherited_type_vars =
                type_var_bindings(contract.def_id_value(db), contract.ty_param_elems(db));
            contract.items(db).iter().find_map(|item| match *item {
                ContractItem::TypeAlias(declaration)
                    if declaration.def_id_value(db) == def
                        && declaration.kind(db) == TypeAliasKind::ValueType =>
                {
                    Some(ValueTypeInfo {
                        declaration,
                        inherited_type_vars: inherited_type_vars.clone(),
                    })
                }
                _ => None,
            })
        }
        _ => None,
    }
}

fn declaration_name(db: &dyn HirDb, declaration: TypeAlias<'_>) -> String {
    declaration
        .def_id_value(db)
        .name(db)
        .unwrap_or_else(|| "<anonymous value type>".to_owned())
}

fn missing_value_type(db: &dyn HirDb, def: DefId<'_>) -> ValueTypeError {
    let span = Span::new(
        AnchorId::root(db, def.file(db)),
        Offset::new(0),
        Offset::new(0),
    );
    ValueTypeError {
        name: def
            .name(db)
            .unwrap_or_else(|| "<unknown value type>".to_owned()),
        span: LabelSpan::from_span(db, span),
        reason: "value-type declaration is unavailable".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_solidity_elementary_value_name;

    #[test]
    fn static_byte_names_are_elementary_but_dynamic_bytes_is_not() {
        assert!(is_solidity_elementary_value_name("byte"));
        assert!(is_solidity_elementary_value_name("bytes1"));
        assert!(is_solidity_elementary_value_name("bytes32"));
        assert!(!is_solidity_elementary_value_name("bytes"));
        assert!(!is_solidity_elementary_value_name("bytes33"));
    }
}
