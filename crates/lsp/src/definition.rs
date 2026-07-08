//! Go-to-definition support over the wasm-clean LSP core.

use hir::{
    anchor::{DefId, resolve_def_location},
    ast::{
        function::{ExprKind, FuncBody, FuncParam, PatKind, StmtKind},
        item::{AdtDef, ClassDef, ContractDef, ContractItem, FunctionDef, Item, Module},
    },
    diag::AbsoluteSpan,
    nameres::{self as hir_nameres, FieldId, LocalBinding, ParamId, Resolution, TypeVarBinding},
    span::{Span, Spanned},
};
use lsp_types::{GotoDefinitionResponse, Location, Position, Url};

use crate::{
    LineIndexExt,
    resolve::{function_owning_offset, innermost_expr},
    state::{WorldState, uri_to_vfs_path},
};

/// Computes the target definition location for the symbol at a source position.
pub fn handle_definition(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let db = world.db();
    let path = uri_to_vfs_path(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let entry = world.workspace().entry_module()?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, entry);

    let owner = function_owning_offset(db, module, file, offset)?;
    let body_map = body_resolution_map(
        db,
        module,
        owner.function,
        owner.root_body,
        owner.enclosing_contract,
        owner.inherited_type_vars,
        &env,
    );
    let (owning_body, expr_id) = innermost_expr(db, owner.root_body, file, offset)?;
    let resolution = body_map
        .exprs
        .iter()
        .find(|entry| entry.body == owning_body && entry.expr == expr_id)?
        .resolution
        .clone();
    let target = resolution_target_span(db, module, resolution)?;
    let location = location_for_span(world, db, target)?;

    Some(GotoDefinitionResponse::Scalar(location))
}

fn body_resolution_map<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    root_body: FuncBody<'db>,
    enclosing_contract: Option<DefId<'db>>,
    mut type_vars: Vec<TypeVarBinding<'db>>,
    imports: &dyn hir_nameres::ImportedNames<'db>,
) -> hir_nameres::BodyResolutionMap<'db> {
    let sig = function.sig(db);
    type_vars.extend(hir_nameres::type_var_bindings(
        function.def_id_value(db),
        &sig.type_vars,
    ));
    let context = hir_nameres::BodyResolutionContext {
        module,
        enclosing_contract,
        params: hir_nameres::param_bindings(sig.params.atom()),
        type_vars,
    };
    hir_nameres::resolve_body_with_imports_and_policy(
        db,
        root_body,
        &context,
        imports,
        hir_nameres::NameresDiagnosticPolicy::Emit,
    )
}

fn resolution_target_span<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    resolution: Resolution<'db>,
) -> Option<AbsoluteSpan> {
    match resolution {
        Resolution::Def { def, .. } => def_name_span(db, def),
        Resolution::Ctor { ty, index } => ctor_name_span(db, ty, index.as_usize()),
        Resolution::Param(param) => param_name_span(db, module, param),
        Resolution::Local(LocalBinding::Let { body, stmt }) => {
            let stmt = body.stmts(db).get(stmt);
            let span = match &stmt.kind {
                StmtKind::Let { name, .. } => name.span(db),
                _ => stmt.span,
            };
            Some(span.resolve_to_absolute(db))
        }
        Resolution::Local(LocalBinding::Pattern { body, pat }) => {
            let pat = body.pats(db).get(pat);
            let span = match &pat.kind {
                PatKind::Var(name) => name.span(db),
                _ => pat.span,
            };
            Some(span.resolve_to_absolute(db))
        }
        Resolution::Local(LocalBinding::TypeVar(_)) => None,
        Resolution::Field(field) => field_name_span(db, field),
        Resolution::ClassMethod { class, name } => class_method_name_span(db, class, &name),
        Resolution::Module(_)
        | Resolution::DotCtorDeferred
        | Resolution::Builtin(_)
        | Resolution::Err => None,
    }
}

fn def_name_span<'db>(db: &'db dyn hir_ty::Db, def: DefId<'db>) -> Option<AbsoluteSpan> {
    let file = def.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_def_name_span_in_module(db, module, def)
        .map(|span| span.resolve_to_absolute(db))
        .or_else(|| {
            let location = resolve_def_location(db.def_location_table(file), def)?;
            Some(AbsoluteSpan::new(
                location.file,
                location.base_offset,
                location.base_offset,
            ))
        })
}

fn find_def_name_span_in_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<Span<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(function.sig(db).name.span(db));
            }
            Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(alias.name_elem(db).span(db));
            }
            Item::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(adt.name_elem(db).span(db));
            }
            Item::ClassDef(class) if class.def_id_value(db) == def => {
                return Some(class.head(db).kind(db).class.span(db));
            }
            Item::InstanceDef(instance) if instance.def_id_value(db) == def => {
                return Some(instance.head(db).span(db));
            }
            Item::ContractDef(contract) => {
                if contract.def_id_value(db) == def {
                    return Some(contract.name_elem(db).span(db));
                }
                if let Some(span) = find_def_name_span_in_contract(db, contract, def) {
                    return Some(span);
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::InstanceDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    None
}

fn find_def_name_span_in_contract<'db>(
    db: &'db dyn hir_ty::Db,
    contract: ContractDef<'db>,
    def: DefId<'db>,
) -> Option<Span<'db>> {
    for item in contract.items(db) {
        match *item {
            ContractItem::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(function.sig(db).name.span(db));
            }
            ContractItem::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(alias.name_elem(db).span(db));
            }
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(adt.name_elem(db).span(db));
            }
            ContractItem::FunctionDef(_)
            | ContractItem::TypeAlias(_)
            | ContractItem::AdtDef(_)
            | ContractItem::Error { .. } => {}
        }
    }

    None
}

fn ctor_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    ty: DefId<'db>,
    index: usize,
) -> Option<AbsoluteSpan> {
    let file = ty.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_adt(db, module, ty)?
        .ctors(db)
        .get(index)
        .map(|ctor| ctor.name.span(db).resolve_to_absolute(db))
}

fn find_adt<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    module.items(db).iter().find_map(|item| match *item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match *item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
            _ => None,
        }),
        _ => None,
    })
}

fn param_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    param: ParamId<'db>,
) -> Option<AbsoluteSpan> {
    find_param_span_in_module(db, module, param.body, param.index.as_usize())
        .map(|span| span.resolve_to_absolute(db))
}

fn find_param_span_in_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    body: FuncBody<'db>,
    index: usize,
) -> Option<Span<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                if let Some(span) = find_param_span_in_function(db, function, body, index) {
                    return Some(span);
                }
            }
            Item::ContractDef(contract) => {
                for contract_item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = *contract_item
                        && let Some(span) = find_param_span_in_function(db, function, body, index)
                    {
                        return Some(span);
                    }
                }
            }
            Item::InstanceDef(instance) => {
                for function in instance.methods(db) {
                    if let Some(span) = find_param_span_in_function(db, *function, body, index) {
                        return Some(span);
                    }
                }
            }
            Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    None
}

fn find_param_span_in_function<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    body: FuncBody<'db>,
    index: usize,
) -> Option<Span<'db>> {
    if function.body(db) == Some(body) {
        return function
            .sig(db)
            .params
            .atom()
            .get(index)
            .and_then(|param| param_name_or_whole_span(db, param));
    }

    find_lambda_param_span(db, function.body(db)?, body, index)
}

fn find_lambda_param_span<'db>(
    db: &'db dyn hir_ty::Db,
    root: FuncBody<'db>,
    body: FuncBody<'db>,
    index: usize,
) -> Option<Span<'db>> {
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        for (_, expr) in current.exprs(db).iter() {
            if let ExprKind::Lambda {
                params,
                body: lambda_body,
                ..
            } = &expr.kind
            {
                if *lambda_body == body {
                    return params
                        .atom()
                        .get(index)
                        .and_then(|param| param_name_or_whole_span(db, param));
                }
                stack.push(*lambda_body);
            }
        }
    }

    None
}

fn param_name_or_whole_span<'db>(
    db: &'db dyn hir_ty::Db,
    param: &FuncParam<'db>,
) -> Option<Span<'db>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name.span(db)),
        FuncParam::Error { span } if !span.resolve_to_absolute(db).is_empty() => Some(*span),
        FuncParam::Error { .. } => None,
    }
}

fn field_name_span<'db>(db: &'db dyn hir_ty::Db, field: FieldId<'db>) -> Option<AbsoluteSpan> {
    let file = field.contract.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_contract(db, module, field.contract)?
        .fields(db)
        .get(field.index.as_usize())
        .map(|field| field.name().span(db).resolve_to_absolute(db))
}

fn class_method_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    class: DefId<'db>,
    name: &str,
) -> Option<AbsoluteSpan> {
    let file = class.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_class(db, module, class)?
        .methods(db)
        .iter()
        .find(|method| method.name.atom().text(db) == name)
        .map(|method| method.name.span(db).resolve_to_absolute(db))
}

fn find_contract<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ContractDef<'db>> {
    module.items(db).iter().find_map(|item| match *item {
        Item::ContractDef(contract) if contract.def_id_value(db) == def => Some(contract),
        _ => None,
    })
}

fn find_class<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ClassDef<'db>> {
    module.items(db).iter().find_map(|item| match *item {
        Item::ClassDef(class) if class.def_id_value(db) == def => Some(class),
        _ => None,
    })
}

fn location_for_span(
    world: &WorldState,
    db: &vfs::AnalysisHost,
    span: AbsoluteSpan,
) -> Option<Location> {
    let uri = Url::parse(span.file().url(db).as_str()).ok()?;
    let range = if let Some(line_index) = world.line_index(&uri) {
        line_index.range(span.start().as_u32(), span.end().as_u32())
    } else {
        let text = span.file().content(db).as_deref()?;
        LineIndexExt::new(text).range(span.start().as_u32(), span.end().as_u32())
    };

    Some(Location { uri, range })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn definition_of_parameter_use_points_to_parameter_name() {
        let source = "function id(x: word) -> word {\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let use_offset = (source.find("return x").expect("return") + "return ".len()) as u32;
        let param_offset = source.find("x: word").expect("param") as u32;
        let line_index = world.line_index(&uri).expect("line index");
        let position = line_index.byte_to_position(use_offset);

        let response = handle_definition(&world, &uri, position).expect("definition");
        let GotoDefinitionResponse::Scalar(location) = response else {
            panic!("expected scalar definition response");
        };

        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range,
            line_index.range(param_offset, param_offset + 1)
        );
    }
}
