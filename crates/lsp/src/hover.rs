//! Hover support over the wasm-clean LSP core.

use crate::{
    references::{ReferenceTarget, reference_target_at},
    resolve::{function_owning_offset, innermost_expr, module_id_for_uri},
    state::WorldState,
};
use hir::{
    anchor::DefId,
    ast::{
        function::{ExprKind, FuncBody, FuncParam, FuncSig, PatKind, StmtKind},
        item::{
            AdtDef, ClassDef, ContractDef, ContractItem, FuncKind, FunctionDef, InstanceDef, Item,
            Module, SourceComment, TypeAlias,
        },
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    nameres::{LocalBinding, ParamId, Resolution, TypeVarBinding},
    span::SpannedElem,
};
use hir_ty::{ClassId, InferResultExt, InferenceResult, PredKind, Ty, TyCtor, TyKind, TyScheme};
use lsp_types::{Hover, HoverContents, MarkedString, Position, Range, Url};

/// Computes hover information at a source position.
///
/// Named declarations and references are resolved through the same semantic
/// identity as references/rename. Expression inference remains the fallback
/// for literals and other non-name syntax.
pub fn handle_hover(world: &WorldState, uri: &Url, position: Position) -> Option<Hover> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let current_module = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, current_module);

    if let Some(target) = reference_target_at(world, uri, position)
        && let Some(info) = semantic_hover_info(db, module, current_module, &env, &target)
    {
        let range = identifier_range(line_index.text(), offset)
            .map(|(start, end)| line_index.range(start, end));
        return Some(hover_from_info(info, range));
    }

    expression_hover(db, module, current_module, &env, file, offset, line_index)
}

struct HoverInfo {
    code: String,
    documentation: Option<String>,
}

fn hover_from_info(info: HoverInfo, range: Option<Range>) -> Hover {
    let code = MarkedString::from_language_code("solcore".to_owned(), info.code);
    let contents = match info.documentation {
        Some(documentation) => {
            HoverContents::Array(vec![code, MarkedString::String(documentation)])
        }
        None => HoverContents::Scalar(code),
    };
    Hover { contents, range }
}

fn semantic_hover_info<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    current_module: nameres::ModuleId<'db>,
    imports: &dyn hir::nameres::ImportedNames<'db>,
    target: &ReferenceTarget<'db>,
) -> Option<HoverInfo> {
    match target {
        ReferenceTarget::Def(def) => definition_hover(db, *def),
        ReferenceTarget::Ctor { ty, index } => constructor_hover(db, *ty, *index),
        ReferenceTarget::Param(param) => {
            parameter_hover(db, module, current_module, imports, *param)
        }
        ReferenceTarget::Local(local) => local_hover(db, module, current_module, imports, local),
        ReferenceTarget::Field(field) => field_hover(db, *field),
        ReferenceTarget::ClassMethod { class, name } => class_method_hover(db, *class, name),
        ReferenceTarget::Module(module_ref) => Some(HoverInfo {
            code: format!("module {}", module_ref.name),
            documentation: None,
        }),
        ReferenceTarget::ImportAlias { name, .. } => Some(HoverInfo {
            code: format!("import alias {name}"),
            documentation: None,
        }),
        ReferenceTarget::ExportedModuleAlias { name, .. } => Some(HoverInfo {
            code: format!("exported module alias {name}"),
            documentation: None,
        }),
    }
}

fn expression_hover<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    current_module: nameres::ModuleId<'db>,
    imports: &dyn hir::nameres::ImportedNames<'db>,
    file: hir::input::SourceFile,
    offset: u32,
    line_index: &crate::LineIndexExt,
) -> Option<Hover> {
    let owner = function_owning_offset(db, module, file, offset)?;
    let analysis = infer_function(
        db,
        module,
        current_module,
        imports,
        &FunctionOwner {
            function: owner.function,
            root_body: owner.root_body,
            enclosing_contract: owner.enclosing_contract,
            inherited_type_vars: owner.inherited_type_vars,
        },
    );
    let (owning_body, expr_id) = innermost_expr(db, owner.root_body, file, offset)?;
    let ty = analysis.inference.expr_ty(owning_body, expr_id)?;
    let expr = owning_body.exprs(db).get(expr_id);
    let absolute = expr.span.resolve_to_absolute(db);

    Some(hover_from_info(
        HoverInfo {
            code: display_ty(db, ty, &analysis.type_var_names),
            documentation: None,
        },
        Some(line_index.range(absolute.start().as_u32(), absolute.end().as_u32())),
    ))
}

fn identifier_range(text: &str, offset: u32) -> Option<(u32, u32)> {
    let offset = usize::try_from(offset).ok()?;
    let suffix = text.get(offset..)?;
    let (cursor, current) = match suffix.chars().next() {
        Some(ch) if is_identifier_char(ch) => (offset, ch),
        _ => text
            .get(..offset)?
            .char_indices()
            .next_back()
            .filter(|(_, ch)| is_identifier_char(*ch))?,
    };

    let mut start = cursor;
    for (index, ch) in text.get(..cursor)?.char_indices().rev() {
        if !is_identifier_char(ch) {
            break;
        }
        start = index;
    }

    let first_end = cursor + current.len_utf8();
    let mut end = first_end;
    for (relative, ch) in text.get(first_end..)?.char_indices() {
        if !is_identifier_char(ch) {
            break;
        }
        end = first_end + relative + ch.len_utf8();
    }
    Some((start as u32, end as u32))
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

enum Definition<'db> {
    Function(FoundFunction<'db>),
    TypeAlias(TypeAlias<'db>),
    Adt(FoundAdt<'db>),
    Class(ClassDef<'db>),
    Instance(InstanceDef<'db>),
    Contract(ContractDef<'db>),
}

struct FoundFunction<'db> {
    function: FunctionDef<'db>,
    type_var_names: Vec<String>,
}

struct FoundAdt<'db> {
    adt: AdtDef<'db>,
    type_var_names: Vec<String>,
}

fn definition_hover<'db>(db: &'db vfs::AnalysisHost, def: DefId<'db>) -> Option<HoverInfo> {
    let module = parser::parse_file_to_hir(db, def.file(db)).module(db);
    match find_definition(db, module, def)? {
        Definition::Function(found) => {
            let function = found.function;
            let sig = function.sig(db);
            let module_id = nameres::module_id_for_source_file(db, def.file(db));
            let scheme = module_id.and_then(|module| hir_ty::function_scheme(db, module, def));
            let callable = scheme.map_or_else(
                || format_source_function_signature(db, sig),
                |scheme| {
                    format_callable_scheme(
                        db,
                        sig.name.atom().text(db),
                        &function_param_names(db, sig),
                        &found.type_var_names,
                        scheme,
                    )
                },
            );
            let keyword = match function.kind(db) {
                FuncKind::Function => "function",
                FuncKind::Constructor => "constructor",
                FuncKind::Fallback => "fallback",
            };
            Some(HoverInfo {
                code: format!("{keyword} {callable}"),
                documentation: comments_markdown(function.leading_comments(db)),
            })
        }
        Definition::TypeAlias(alias) => {
            let name = alias.name_elem(db).atom().text(db);
            let params = type_parameter_list(db, alias.ty_param_elems(db));
            Some(HoverInfo {
                code: format!(
                    "type {name}{params} = {}",
                    display_type_ref(db, alias.ty(db))
                ),
                documentation: comments_markdown(alias.leading_comments(db)),
            })
        }
        Definition::Adt(found) => Some(HoverInfo {
            code: format_adt_declaration(db, found.adt),
            documentation: comments_markdown(found.adt.leading_comments(db)),
        }),
        Definition::Class(class) => Some(HoverInfo {
            code: format!("class {}", display_pred_ref(db, class.head(db))),
            documentation: comments_markdown(class.leading_comments(db)),
        }),
        Definition::Instance(instance) => Some(HoverInfo {
            code: format!("instance {}", display_pred_ref(db, instance.head(db))),
            documentation: comments_markdown(instance.leading_comments(db)),
        }),
        Definition::Contract(contract) => {
            let name = contract.name_elem(db).atom().text(db);
            Some(HoverInfo {
                code: format!(
                    "contract {name}{}",
                    type_parameter_list(db, contract.ty_param_elems(db))
                ),
                documentation: comments_markdown(contract.leading_comments(db)),
            })
        }
    }
}

fn find_definition<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<Definition<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(Definition::Function(FoundFunction {
                    function,
                    type_var_names: function_type_var_names(db, &[], function),
                }));
            }
            Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(Definition::TypeAlias(alias));
            }
            Item::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(Definition::Adt(FoundAdt {
                    adt,
                    type_var_names: ident_names(db, adt.ty_param_elems(db)),
                }));
            }
            Item::ClassDef(class) if class.def_id_value(db) == def => {
                return Some(Definition::Class(class));
            }
            Item::InstanceDef(instance) => {
                if instance.def_id_value(db) == def {
                    return Some(Definition::Instance(instance));
                }
                let inherited = ident_names(db, instance.type_var_elems(db));
                if let Some(function) = instance
                    .methods(db)
                    .iter()
                    .copied()
                    .find(|function| function.def_id_value(db) == def)
                {
                    return Some(Definition::Function(FoundFunction {
                        function,
                        type_var_names: function_type_var_names(db, &inherited, function),
                    }));
                }
            }
            Item::ContractDef(contract) => {
                if contract.def_id_value(db) == def {
                    return Some(Definition::Contract(contract));
                }
                let inherited = ident_names(db, contract.ty_param_elems(db));
                for item in contract.items(db) {
                    match *item {
                        ContractItem::FunctionDef(function) if function.def_id_value(db) == def => {
                            return Some(Definition::Function(FoundFunction {
                                function,
                                type_var_names: function_type_var_names(db, &inherited, function),
                            }));
                        }
                        ContractItem::TypeAlias(alias) if alias.def_id_value(db) == def => {
                            return Some(Definition::TypeAlias(alias));
                        }
                        ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => {
                            let mut type_var_names = inherited.clone();
                            type_var_names.extend(ident_names(db, adt.ty_param_elems(db)));
                            return Some(Definition::Adt(FoundAdt {
                                adt,
                                type_var_names,
                            }));
                        }
                        ContractItem::FunctionDef(_)
                        | ContractItem::TypeAlias(_)
                        | ContractItem::AdtDef(_)
                        | ContractItem::Error { .. } => {}
                    }
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
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

fn function_type_var_names<'db>(
    db: &'db dyn hir_ty::Db,
    inherited: &[String],
    function: FunctionDef<'db>,
) -> Vec<String> {
    let mut names = inherited.to_vec();
    names.extend(ident_names(db, &function.sig(db).type_vars));
    names
}

fn ident_names<'db>(
    db: &'db dyn hir_ty::Db,
    idents: &[SpannedElem<'db, hir::ast::Ident<'db>>],
) -> Vec<String> {
    idents
        .iter()
        .map(|ident| ident.atom().text(db).to_owned())
        .collect()
}

fn function_param_names<'db>(db: &'db dyn hir_ty::Db, sig: &FuncSig<'db>) -> Vec<String> {
    sig.params
        .atom()
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(name.atom().text(db).to_owned())
            }
            FuncParam::Error { .. } => None,
        })
        .collect()
}

fn format_source_function_signature<'db>(db: &'db dyn hir_ty::Db, sig: &FuncSig<'db>) -> String {
    let params = sig
        .params
        .atom()
        .iter()
        .map(|param| format_source_param(db, param))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = sig
        .ret
        .map(|ret| display_type_ref(db, ret))
        .unwrap_or_else(|| "_".to_owned());
    format!("{}({params}) -> {ret}", sig.name.atom().text(db))
}

fn format_source_param<'db>(db: &'db dyn hir_ty::Db, param: &FuncParam<'db>) -> String {
    match param {
        FuncParam::Typed { comptime, name, ty } => {
            let prefix = if comptime.is_some() { "comptime " } else { "" };
            format!(
                "{prefix}{}: {}",
                name.atom().text(db),
                display_type_ref(db, *ty)
            )
        }
        FuncParam::Untyped { comptime, name } => {
            let prefix = if comptime.is_some() { "comptime " } else { "" };
            format!("{prefix}{}: _", name.atom().text(db))
        }
        FuncParam::Error { .. } => "<error>: <error>".to_owned(),
    }
}

fn format_adt_declaration<'db>(db: &'db dyn hir_ty::Db, adt: AdtDef<'db>) -> String {
    let name = adt.name_elem(db).atom().text(db);
    let params = type_parameter_list(db, adt.ty_param_elems(db));
    let ctors = adt
        .ctors(db)
        .iter()
        .map(|ctor| {
            let name = ctor.name.atom().text(db);
            if ctor.field_count == 0 {
                name.to_owned()
            } else {
                let fields = display_type_ref(db, *ctor.fields.atom());
                if ctor.field_count == 1 {
                    format!("{name}({fields})")
                } else {
                    format!("{name}{fields}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");
    if ctors.is_empty() {
        format!("data {name}{params}")
    } else {
        format!("data {name}{params} = {ctors}")
    }
}

fn type_parameter_list<'db>(
    db: &'db dyn hir_ty::Db,
    params: &[SpannedElem<'db, hir::ast::Ident<'db>>],
) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("({})", ident_names(db, params).join(", "))
    }
}

fn comments_markdown(comments: &[SourceComment]) -> Option<String> {
    let text = comments
        .iter()
        .map(|comment| {
            comment
                .text
                .lines()
                .map(|line| line.trim().trim_start_matches('*').trim_start())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

fn constructor_hover<'db>(
    db: &'db vfs::AnalysisHost,
    ty: DefId<'db>,
    index: hir::nameres::CtorIndex,
) -> Option<HoverInfo> {
    let module = parser::parse_file_to_hir(db, ty.file(db)).module(db);
    let Definition::Adt(found) = find_definition(db, module, ty)? else {
        return None;
    };
    let ctor = found.adt.ctors(db).get(index.as_usize())?;
    let name = ctor.name.atom().text(db);
    let module_id = nameres::module_id_for_source_file(db, ty.file(db))?;
    let scheme = hir_ty::infer::adt_ctor_scheme(db, module_id, ty, index)?;
    Some(HoverInfo {
        code: format!(
            "constructor {}",
            format_callable_scheme(db, name, &[], &found.type_var_names, scheme)
        ),
        documentation: comments_markdown(found.adt.ctor_leading_comments(db, index.as_usize())?),
    })
}

fn field_hover<'db>(
    db: &'db vfs::AnalysisHost,
    field: hir::nameres::FieldId<'db>,
) -> Option<HoverInfo> {
    let module = parser::parse_file_to_hir(db, field.contract.file(db)).module(db);
    let Definition::Contract(contract) = find_definition(db, module, field.contract)? else {
        return None;
    };
    let field_def = contract.fields(db).get(field.index.as_usize())?;
    let module_id = nameres::module_id_for_source_file(db, field.contract.file(db))?;
    let scheme = hir_ty::infer::field_scheme(db, module_id, field)?;
    let type_var_names = ident_names(db, contract.ty_param_elems(db));
    Some(HoverInfo {
        code: format!(
            "{}: {}",
            field_def.name().atom().text(db),
            display_scheme_type(db, scheme, &type_var_names)
        ),
        documentation: comments_markdown(
            contract.field_leading_comments(db, field.index.as_usize())?,
        ),
    })
}

fn class_method_hover<'db>(
    db: &'db vfs::AnalysisHost,
    class: DefId<'db>,
    name: &str,
) -> Option<HoverInfo> {
    let module = parser::parse_file_to_hir(db, class.file(db)).module(db);
    let Definition::Class(class_def) = find_definition(db, module, class)? else {
        return None;
    };
    let (index, sig) = class_def
        .methods(db)
        .iter()
        .enumerate()
        .find(|(_, method)| method.name.atom().text(db) == name)?;
    let module_id = nameres::module_id_for_source_file(db, class.file(db))?;
    let scheme = hir_ty::infer::class_method_scheme(db, module_id, class, name.to_owned())?;
    let type_var_names = ident_names(db, class_def.type_var_elems(db));
    Some(HoverInfo {
        code: format!(
            "function {}",
            format_callable_scheme(
                db,
                name,
                &function_param_names(db, sig),
                &type_var_names,
                scheme,
            )
        ),
        documentation: comments_markdown(class_def.method_leading_comments(db, index)?),
    })
}

struct FunctionOwner<'db> {
    function: FunctionDef<'db>,
    root_body: FuncBody<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
}

struct BodyAnalysis<'db> {
    inference: InferenceResult<'db>,
    resolutions: hir::nameres::BodyResolutionMap<'db>,
    type_var_names: Vec<String>,
}

fn parameter_hover<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    current_module: nameres::ModuleId<'db>,
    imports: &dyn hir::nameres::ImportedNames<'db>,
    param: ParamId<'db>,
) -> Option<HoverInfo> {
    let owner = function_owner_for_body(db, module, param.body)?;
    let parameter = parameter_for_body(db, &owner, param.body, param.index.as_usize())?;
    let analysis = infer_function(db, module, current_module, imports, &owner);
    let inferred = if param.body == owner.root_body {
        root_parameter_ty(db, &analysis.inference, param.index.as_usize())
    } else {
        parameter_reference_ty(&analysis, param)
    };
    let (name, annotated, comptime) = match &parameter {
        FuncParam::Typed { comptime, name, ty } => (
            name.atom().text(db).to_owned(),
            Some(display_type_ref(db, *ty)),
            comptime.is_some(),
        ),
        FuncParam::Untyped { comptime, name } => {
            (name.atom().text(db).to_owned(), None, comptime.is_some())
        }
        FuncParam::Error { .. } => return None,
    };
    let ty = inferred
        .map(|ty| display_ty(db, ty, &analysis.type_var_names))
        .or(annotated)
        .unwrap_or_else(|| "_".to_owned());
    let prefix = if comptime { "comptime " } else { "" };
    Some(HoverInfo {
        code: format!("{prefix}{name}: {ty}"),
        documentation: None,
    })
}

fn local_hover<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    current_module: nameres::ModuleId<'db>,
    imports: &dyn hir::nameres::ImportedNames<'db>,
    local: &LocalBinding<'db>,
) -> Option<HoverInfo> {
    let body = match local {
        LocalBinding::Let { body, .. } | LocalBinding::Pattern { body, .. } => *body,
        LocalBinding::TypeVar(type_var) => {
            return Some(HoverInfo {
                code: format!("type parameter {}", type_var.name),
                documentation: None,
            });
        }
    };
    let owner = function_owner_for_body(db, module, body)?;
    let analysis = infer_function(db, module, current_module, imports, &owner);

    match local {
        LocalBinding::Let { body, stmt } => {
            let statement = body.stmts(db).get(*stmt);
            let StmtKind::Let {
                comptime,
                name,
                ty: annotation,
                ..
            } = &statement.kind
            else {
                return None;
            };
            let ty = analysis
                .inference
                .let_ty(*body, *stmt)
                .map(|ty| display_ty(db, ty, &analysis.type_var_names))
                .or_else(|| annotation.map(|ty| display_type_ref(db, ty)))
                .unwrap_or_else(|| "_".to_owned());
            let prefix = if comptime.is_some() { "comptime " } else { "" };
            Some(HoverInfo {
                code: format!("{prefix}let {}: {ty}", name.atom().text(db)),
                documentation: None,
            })
        }
        LocalBinding::Pattern { body, pat } => {
            let pattern = body.pats(db).get(*pat);
            let PatKind::Var(name) = &pattern.kind else {
                return None;
            };
            let ty = analysis
                .inference
                .pat_ty(*body, *pat)
                .map(|ty| display_ty(db, ty, &analysis.type_var_names))
                .unwrap_or_else(|| "_".to_owned());
            Some(HoverInfo {
                code: format!("{}: {ty}", name.atom().text(db)),
                documentation: None,
            })
        }
        LocalBinding::TypeVar(_) => unreachable!("handled above"),
    }
}

fn root_parameter_ty<'db>(
    db: &'db dyn hir_ty::Db,
    inference: &InferenceResult<'db>,
    index: usize,
) -> Option<Ty<'db>> {
    let TyKind::Function { params, .. } = inference.root_scheme.body(db).ty(db).kind(db) else {
        return None;
    };
    params.get(index).copied()
}

fn parameter_reference_ty<'db>(
    analysis: &BodyAnalysis<'db>,
    param: ParamId<'db>,
) -> Option<Ty<'db>> {
    analysis.resolutions.exprs.iter().find_map(|entry| {
        if matches!(&entry.resolution, Resolution::Param(candidate) if *candidate == param) {
            analysis.inference.expr_ty(entry.body, entry.expr)
        } else {
            None
        }
    })
}

fn parameter_for_body<'db>(
    db: &'db dyn hir_ty::Db,
    owner: &FunctionOwner<'db>,
    target: FuncBody<'db>,
    index: usize,
) -> Option<FuncParam<'db>> {
    if owner.root_body == target {
        return owner.function.sig(db).params.atom().get(index).cloned();
    }

    let mut stack = vec![owner.root_body];
    while let Some(body) = stack.pop() {
        for (_, expr) in body.exprs(db).iter() {
            if let ExprKind::Lambda {
                params,
                body: lambda_body,
                ..
            } = &expr.kind
            {
                if *lambda_body == target {
                    return params.atom().get(index).cloned();
                }
                stack.push(*lambda_body);
            }
        }
    }
    None
}

fn function_owner_for_body<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    target: FuncBody<'db>,
) -> Option<FunctionOwner<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                if let Some(owner) = make_owner_if_contains(db, function, None, Vec::new(), target)
                {
                    return Some(owner);
                }
            }
            Item::ContractDef(contract) => {
                let inherited_type_vars = hir::nameres::type_var_bindings(
                    contract.def_id_value(db),
                    contract.ty_param_elems(db),
                );
                for item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = *item
                        && let Some(owner) = make_owner_if_contains(
                            db,
                            function,
                            Some(contract.def_id_value(db)),
                            inherited_type_vars.clone(),
                            target,
                        )
                    {
                        return Some(owner);
                    }
                }
            }
            Item::InstanceDef(instance) => {
                let inherited_type_vars = hir::nameres::type_var_bindings(
                    instance.def_id_value(db),
                    instance.type_var_elems(db),
                );
                for function in instance.methods(db) {
                    if let Some(owner) = make_owner_if_contains(
                        db,
                        *function,
                        None,
                        inherited_type_vars.clone(),
                        target,
                    ) {
                        return Some(owner);
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

fn make_owner_if_contains<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
    target: FuncBody<'db>,
) -> Option<FunctionOwner<'db>> {
    let root_body = function.body(db)?;
    body_tree_contains(db, root_body, target).then_some(FunctionOwner {
        function,
        root_body,
        enclosing_contract,
        inherited_type_vars,
    })
}

fn body_tree_contains<'db>(
    db: &'db dyn hir_ty::Db,
    root: FuncBody<'db>,
    target: FuncBody<'db>,
) -> bool {
    let mut stack = vec![root];
    while let Some(body) = stack.pop() {
        if body == target {
            return true;
        }
        for (_, expr) in body.exprs(db).iter() {
            if let ExprKind::Lambda {
                body: lambda_body, ..
            } = &expr.kind
            {
                stack.push(*lambda_body);
            }
        }
    }
    false
}

fn infer_function<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    current_module: nameres::ModuleId<'db>,
    imports: &dyn hir::nameres::ImportedNames<'db>,
    owner: &FunctionOwner<'db>,
) -> BodyAnalysis<'db> {
    let scope = hir::nameres::item_scope_facts(db, module);
    let item_facts =
        hir::nameres::resolve_item_type_facts_with_imports(db, module, &scope, imports);
    let sig = owner.function.sig(db);
    let mut type_vars = owner.inherited_type_vars.clone();
    type_vars.extend(hir::nameres::type_var_bindings(
        owner.function.def_id_value(db),
        &sig.type_vars,
    ));
    let type_var_names = type_vars
        .iter()
        .map(|var| var.name.atom().text(db).to_owned())
        .collect::<Vec<_>>();
    let body_context = hir::nameres::BodyResolutionContext {
        module,
        enclosing_contract: owner.enclosing_contract,
        params: hir::nameres::param_bindings(sig.params.atom()),
        type_vars: type_vars.clone(),
    };
    let resolutions = hir::nameres::resolve_body_with_imports_and_policy(
        db,
        owner.root_body,
        &body_context,
        imports,
        hir::nameres::NameresDiagnosticPolicy::Emit,
    );
    let lowered = hir_ty::lower_normalized_function_with_inferred_signature(
        db,
        module,
        &item_facts,
        owner.function,
        &type_vars,
        Some(&resolutions),
        Some(current_module),
    );
    let param_names = function_param_names(db, sig);
    let ty_context = hir_ty::BodyTyContext::new(
        module,
        resolutions.clone(),
        type_vars,
        lowered.params.clone(),
        Some(lowered.ret),
    )
    .with_param_names(param_names)
    .with_entry_module(current_module)
    .with_pre_typeck_desugar(hir_ty::pre_typeck_desugar_body_tree(db, owner.root_body));
    let inference = hir_ty::infer_body(db, owner.root_body, ty_context);

    BodyAnalysis {
        inference,
        resolutions,
        type_var_names,
    }
}

fn format_callable_scheme<'db>(
    db: &'db dyn hir_ty::Db,
    name: &str,
    param_names: &[String],
    type_var_names: &[String],
    scheme: TyScheme<'db>,
) -> String {
    let ty = scheme.body(db).ty(db);
    let (params, ret) = match ty.kind(db) {
        TyKind::Function { params, ret } => (params.as_slice(), *ret),
        _ => (&[][..], ty),
    };
    let params = params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let ty = display_ty(db, *param, type_var_names);
            param_names
                .get(index)
                .map(|name| format!("{name}: {ty}"))
                .unwrap_or(ty)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut signature = format!(
        "{name}({params}) -> {}",
        display_ty(db, ret, type_var_names)
    );
    let predicates = scheme
        .body(db)
        .preds(db)
        .iter()
        .map(|pred| display_pred(db, *pred, type_var_names))
        .collect::<Vec<_>>();
    if !predicates.is_empty() {
        signature.push_str(" where ");
        signature.push_str(&predicates.join(", "));
    }
    signature
}

fn display_scheme_type<'db>(
    db: &'db dyn hir_ty::Db,
    scheme: TyScheme<'db>,
    type_var_names: &[String],
) -> String {
    display_ty(db, scheme.body(db).ty(db), type_var_names)
}

fn display_ty<'db>(db: &'db dyn hir_ty::Db, ty: Ty<'db>, names: &[String]) -> String {
    match ty.kind(db) {
        TyKind::Error => "<error>".to_owned(),
        TyKind::Unknown => "_".to_owned(),
        TyKind::BoundVar(var) => names
            .get(var.index as usize)
            .cloned()
            .unwrap_or_else(|| "_".to_owned()),
        TyKind::Named { ctor, args } => {
            let name = match ctor {
                TyCtor::Builtin(ctor) => ctor.name().to_owned(),
                TyCtor::User(user) => user
                    .def
                    .name(db)
                    .unwrap_or_else(|| format!("{:?}", user.def.kind(db))),
            };
            if args.is_empty() {
                name
            } else {
                format!(
                    "{name}({})",
                    args.iter()
                        .map(|arg| display_ty(db, *arg, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Function { params, ret } => format!(
            "({}) -> {}",
            params
                .iter()
                .map(|param| display_ty(db, *param, names))
                .collect::<Vec<_>>()
                .join(", "),
            display_ty(db, *ret, names)
        ),
        TyKind::Tuple(elems) => {
            if elems.is_empty() {
                "()".to_owned()
            } else {
                format!(
                    "({})",
                    elems
                        .iter()
                        .map(|elem| display_ty(db, *elem, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Comptime(inner) => format!("comptime {}", display_ty(db, *inner, names)),
    }
}

fn display_pred<'db>(db: &'db dyn hir_ty::Db, pred: hir_ty::Pred<'db>, names: &[String]) -> String {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => {
            let class = match class {
                ClassId::Builtin(class) => class.name().to_owned(),
                ClassId::User(def) => def
                    .name(db)
                    .unwrap_or_else(|| format!("{:?}", def.kind(db))),
            };
            if args.is_empty() {
                format!("{}: {class}", display_ty(db, *main, names))
            } else {
                format!(
                    "{}: {class}({})",
                    display_ty(db, *main, names),
                    args.iter()
                        .map(|arg| display_ty(db, *arg, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        PredKind::Eq { lhs, rhs } => format!(
            "{} ~ {}",
            display_ty(db, *lhs, names),
            display_ty(db, *rhs, names)
        ),
        PredKind::Error => "<error predicate>".to_owned(),
    }
}

fn display_type_ref<'db>(db: &'db dyn hir_ty::Db, ty: TypeRef<'db>) -> String {
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier,
            name,
            args,
        } => {
            let mut out = String::new();
            if let Some(qualifier) = qualifier {
                out.push_str(qualifier.atom().text(db));
                out.push('.');
            }
            out.push_str(name.atom().text(db));
            if !args.atom().is_empty() {
                out.push('(');
                out.push_str(
                    &args
                        .atom()
                        .iter()
                        .map(|arg| display_type_ref(db, *arg))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push(')');
            }
            out
        }
        TypeRefKind::Fn { params, ret } => format!(
            "({}) -> {}",
            params
                .atom()
                .iter()
                .map(|param| display_type_ref(db, *param))
                .collect::<Vec<_>>()
                .join(", "),
            display_type_ref(db, *ret)
        ),
        TypeRefKind::Comptime { inner, .. } => {
            format!("comptime {}", display_type_ref(db, *inner))
        }
        TypeRefKind::Tuple { elems } => format!(
            "({})",
            elems
                .atom()
                .iter()
                .map(|elem| display_type_ref(db, *elem))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRefKind::Error { .. } => "<error type>".to_owned(),
    }
}

fn display_pred_ref<'db>(db: &'db dyn hir_ty::Db, pred: PredRef<'db>) -> String {
    let kind = pred.kind(db);
    let ty = display_type_ref(db, kind.ty);
    let class = kind.class.atom().text(db);
    if kind.args.atom().is_empty() {
        format!("{ty}: {class}")
    } else {
        format!(
            "{ty}: {class}({})",
            kind.args
                .atom()
                .iter()
                .map(|arg| display_type_ref(db, *arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::{HoverContents, MarkedString};

    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    fn hover_at(source: &str, world: &WorldState, uri: &Url, offset: usize) -> Hover {
        let position = world
            .line_index(uri)
            .expect("line index")
            .byte_to_position(offset as u32);
        handle_hover(world, uri, position).unwrap_or_else(|| {
            panic!(
                "hover at byte {offset} (`{}`)",
                &source[offset..source.len().min(offset + 12)]
            )
        })
    }

    fn hover_code(hover: &Hover) -> &str {
        let marked = match &hover.contents {
            HoverContents::Scalar(marked) => marked,
            HoverContents::Array(marked) => marked.first().expect("hover code"),
            HoverContents::Markup(markup) => return &markup.value,
        };
        match marked {
            MarkedString::LanguageString(value) => &value.value,
            MarkedString::String(value) => value,
        }
    }

    #[test]
    fn identifier_range_supports_unicode_and_internal_hyphens() {
        let text = "prefix λ-value suffix";
        let start = text.find('λ').expect("unicode identifier") as u32;
        let end = start + "λ-value".len() as u32;

        assert_eq!(identifier_range(text, start), Some((start, end)));
        assert_eq!(
            identifier_range(text, start + "λ-".len() as u32),
            Some((start, end))
        );
        assert_eq!(identifier_range(text, start + 1), None);
    }

    #[test]
    fn hovers_integer_literal_type() {
        let source = "function main() -> word {\n  return 42;\n}\n";
        let (world, uri) = world_with_main(source);
        let literal_offset = source.find("42").expect("literal");

        let hover = hover_at(source, &world, &uri, literal_offset);
        let display = hover_code(&hover);

        assert!(
            display.contains("word"),
            "expected word type in hover display, got {display}"
        );
        assert_eq!(
            hover.range,
            Some(
                world
                    .line_index(&uri)
                    .expect("line index")
                    .range(literal_offset as u32, literal_offset as u32 + 2)
            )
        );
    }

    #[test]
    fn function_and_parameter_references_show_signatures_and_identifier_ranges() {
        let source = "\
// Returns its input.
function id(x: word) -> word {
  return x;
}

function main() -> word {
  return id(42);
}
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");

        let call = source.rfind("id(42)").expect("call");
        let function_hover = hover_at(source, &world, &uri, call);
        assert!(
            hover_code(&function_hover).contains("id(x: word) -> word"),
            "unexpected function hover: {:?}",
            function_hover.contents
        );
        assert_eq!(
            function_hover.range,
            Some(line_index.range(call as u32, call as u32 + 2))
        );
        assert!(
            matches!(function_hover.contents, HoverContents::Array(_)),
            "leading documentation should be included"
        );

        let parameter = source.find("return x").expect("parameter use") + "return ".len();
        let parameter_hover = hover_at(source, &world, &uri, parameter);
        assert_eq!(hover_code(&parameter_hover), "x: word");
        assert_eq!(
            parameter_hover.range,
            Some(line_index.range(parameter as u32, parameter as u32 + 1))
        );
    }

    #[test]
    fn inferred_local_reference_hover_uses_local_name_range() {
        let source = "\
function main() -> word {
  let result = 42;
  return result;
}
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let reference = source.rfind("result").expect("local reference");

        let hover = hover_at(source, &world, &uri, reference);

        assert!(
            hover_code(&hover).contains("let result: word"),
            "unexpected local hover: {:?}",
            hover.contents
        );
        assert_eq!(
            hover.range,
            Some(line_index.range(reference as u32, reference as u32 + "result".len() as u32))
        );
    }

    #[test]
    fn type_and_constructor_references_have_rich_hover_and_leaf_ranges() {
        let source = "\
data Maybe = None | Some(word);

function main() -> Maybe {
  return Maybe.Some(42);
}
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");

        let ty_reference = source.rfind("Maybe").expect("type reference");
        let ty_hover = hover_at(source, &world, &uri, ty_reference);
        assert!(
            hover_code(&ty_hover).contains("data Maybe = None | Some(word)"),
            "unexpected type hover: {:?}",
            ty_hover.contents
        );
        assert_eq!(
            ty_hover.range,
            Some(line_index.range(ty_reference as u32, ty_reference as u32 + 5))
        );

        let ctor_reference = source.rfind("Some(42)").expect("constructor reference");
        let ctor_hover = hover_at(source, &world, &uri, ctor_reference);
        let ctor_code = hover_code(&ctor_hover);
        assert!(
            ctor_code.contains("Some(word) -> Maybe"),
            "unexpected constructor hover: {ctor_code}"
        );
        assert_eq!(
            ctor_hover.range,
            Some(line_index.range(ctor_reference as u32, ctor_reference as u32 + 4))
        );
    }
}
