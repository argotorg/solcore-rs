//! Signature help support over the wasm-clean LSP core.

use hir::{
    anchor::DefId,
    arena::Id,
    ast::{
        function::{Expr, ExprKind, FuncBody, FuncParam, FuncSig},
        item::{AdtDef, ContractItem, FunctionDef, Item, Module},
        ty::{TypeRef, TypeRefKind},
    },
    input::SourceFile,
    nameres::{self as hir_nameres, DefResolutionKind, Resolution, TypeVarBinding},
};
use hir_ty::{TyKind, TyScheme};
use lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation, Url,
};

use crate::{
    resolve::{function_owning_offset, module_id_for_uri},
    state::WorldState,
};

/// Computes signature help for the nearest call argument list at a source
/// position.
pub fn handle_signature_help(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<SignatureHelp> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let current_module = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, current_module);

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
    let call = enclosing_call(db, owner.root_body, file, offset, line_index.text())?;
    let resolution = expr_resolution(&body_map, call.body, call.callee)?;
    let mut signature = callable_signature(db, resolution)?;
    let active_parameter = if signature.parameters.is_empty() {
        0
    } else {
        call.active_parameter
            .min(signature.parameters.len() as u32 - 1)
    };

    let parameters = signature
        .parameters
        .drain(..)
        .map(|label| ParameterInformation {
            label: ParameterLabel::Simple(label),
            documentation: None,
        })
        .collect::<Vec<_>>();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: signature.label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active_parameter),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_parameter),
    })
}

struct CallAtOffset<'db> {
    body: FuncBody<'db>,
    callee: Id<Expr<'db>>,
    active_parameter: u32,
}

struct CallableSignature {
    label: String,
    parameters: Vec<String>,
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

fn enclosing_call<'db>(
    db: &'db dyn hir_ty::Db,
    root_body: FuncBody<'db>,
    file: SourceFile,
    offset: u32,
    text: &str,
) -> Option<CallAtOffset<'db>> {
    let mut best = None;
    let mut stack = vec![root_body];

    while let Some(body) = stack.pop() {
        for (expr_id, expr) in body.exprs(db).iter() {
            if let ExprKind::Lambda {
                body: lambda_body, ..
            } = &expr.kind
            {
                stack.push(*lambda_body);
            }

            let ExprKind::Call { callee, args } = &expr.kind else {
                continue;
            };
            let absolute = expr.span.resolve_to_absolute(db);
            if absolute.file() != file
                || offset < absolute.start().as_u32()
                || absolute.end().as_u32() < offset
            {
                continue;
            }

            let callee_span = body.exprs(db).get(*callee).span.resolve_to_absolute(db);
            let Some((args_start, args_end)) =
                call_argument_range(text, callee_span.end().as_u32(), absolute.end().as_u32())
            else {
                continue;
            };
            if offset < args_start || args_end < offset {
                continue;
            }

            let width = absolute.len();
            let active_parameter = active_parameter(db, body, args, file, offset, text, args_start);
            if best
                .as_ref()
                .is_none_or(|(_, _, best_width)| width < *best_width)
            {
                best = Some((
                    expr_id,
                    CallAtOffset {
                        body,
                        callee: *callee,
                        active_parameter,
                    },
                    width,
                ));
            }
        }
    }

    best.map(|(_, call, _)| call)
}

fn call_argument_range(text: &str, callee_end: u32, call_end: u32) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let search_start = callee_end as usize;
    let search_end = call_end.min(text.len() as u32) as usize;
    let open = bytes
        .get(search_start..search_end)?
        .iter()
        .position(|byte| *byte == b'(')?
        + search_start;

    let mut depth = 0u32;
    for (index, byte) in bytes.iter().enumerate().take(search_end).skip(open) {
        match *byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(((open + 1) as u32, index as u32));
                }
            }
            _ => {}
        }
    }

    None
}

fn active_parameter<'db>(
    db: &'db dyn hir_ty::Db,
    body: FuncBody<'db>,
    args: &[Id<Expr<'db>>],
    file: SourceFile,
    offset: u32,
    text: &str,
    args_start: u32,
) -> u32 {
    for (index, arg) in args.iter().enumerate() {
        let absolute = body.exprs(db).get(*arg).span.resolve_to_absolute(db);
        if absolute.file() == file
            && absolute.start().as_u32() <= offset
            && offset <= absolute.end().as_u32()
        {
            return index as u32;
        }
    }

    count_commas_before(text, args_start, offset)
}

fn count_commas_before(text: &str, start: u32, offset: u32) -> u32 {
    let start = start.min(text.len() as u32) as usize;
    let end = offset.min(text.len() as u32) as usize;
    let mut depth = 0u32;
    let mut commas = 0u32;

    for byte in text.as_bytes()[start..end].iter().copied() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => commas += 1,
            _ => {}
        }
    }

    commas
}

fn expr_resolution<'db>(
    body_map: &hir_nameres::BodyResolutionMap<'db>,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> Option<Resolution<'db>> {
    body_map
        .exprs
        .iter()
        .find(|entry| entry.body == body && entry.expr == expr)
        .map(|entry| entry.resolution.clone())
}

fn callable_signature<'db>(
    db: &'db dyn hir_ty::Db,
    resolution: Resolution<'db>,
) -> Option<CallableSignature> {
    match resolution {
        Resolution::Def {
            def,
            kind: DefResolutionKind::Function,
        } => {
            let function = function_for_def(db, def)?;
            let sig = function.sig(db);
            let name = sig.name.atom().text(db).to_owned();
            let defining_module = nameres::module_id_for_source_file(db, def.file(db))?;
            let scheme = hir_ty::infer::function_scheme(db, defining_module, def)?;
            signature_from_scheme(db, &name, &[], scheme, Some(sig), None)
        }
        Resolution::Ctor { ty, index } => {
            let ctor = adt_ctor_for_def(db, ty, index.as_usize())?;
            let name = ctor.name.atom().text(db).to_owned();
            let defining_module = nameres::module_id_for_source_file(db, ty.file(db))?;
            let scheme = hir_ty::infer::adt_ctor_scheme(db, defining_module, ty, index)?;
            let source_params = adt_ctor_source_params(db, &ctor);
            signature_from_scheme(db, &name, &[], scheme, None, Some(&source_params))
        }
        Resolution::ClassMethod { class, name } => {
            let defining_module = nameres::module_id_for_source_file(db, class.file(db))?;
            let scheme =
                hir_ty::infer::class_method_scheme(db, defining_module, class, name.clone())?;
            let source_sig = class_method_sig_for_def(db, class, &name);
            signature_from_scheme(db, &name, &[], scheme, source_sig, None)
        }
        Resolution::Builtin(kind) => {
            let name = builtin_name(kind)?;
            let scheme = hir_ty::builtin_scheme(db, kind)?;
            signature_from_scheme(db, name, &[], scheme, None, None)
        }
        Resolution::Def { .. }
        | Resolution::Local(_)
        | Resolution::Param(_)
        | Resolution::Field(_)
        | Resolution::Module(_)
        | Resolution::DotCtorDeferred
        | Resolution::Err => None,
    }
}

fn signature_from_scheme<'db>(
    db: &'db dyn hir_ty::Db,
    name: &str,
    param_names: &[String],
    scheme: TyScheme<'db>,
    source_sig: Option<&FuncSig<'db>>,
    source_params: Option<&[TypeRef<'db>]>,
) -> Option<CallableSignature> {
    let ty = scheme.body(db).ty(db);
    let (params, ret) = match ty.kind(db) {
        TyKind::Function { params, ret } => (params.clone(), *ret),
        _ => (Vec::new(), ty),
    };
    let parameters = params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let source_param = source_sig.and_then(|sig| sig.params.atom().get(index));
            let ty = source_param
                .and_then(|param| match param {
                    FuncParam::Typed { ty, .. } => Some(hir_ty::display_type_ref_source(db, *ty)),
                    FuncParam::Untyped { .. } | FuncParam::Error { .. } => None,
                })
                .or_else(|| {
                    source_params
                        .and_then(|params| params.get(index))
                        .map(|ty| hir_ty::display_type_ref_source(db, *ty))
                })
                .unwrap_or_else(|| param.display(db));
            if let Some(
                FuncParam::Typed { comptime, name, .. } | FuncParam::Untyped { comptime, name },
            ) = source_param
            {
                let prefix = if comptime.is_some() { "comptime " } else { "" };
                return format!("{prefix}{}: {ty}", name.atom().text(db));
            }
            param_names
                .get(index)
                .map(|name| format!("{name}: {ty}"))
                .unwrap_or(ty)
        })
        .collect::<Vec<_>>();
    let return_suffix = source_sig
        .and_then(|sig| sig.ret)
        .map(|ret| display_source_return_suffix(db, ret))
        .unwrap_or_else(|| match ret.kind(db) {
            TyKind::Tuple(elements) if elements.is_empty() => String::new(),
            TyKind::Tuple(elements) => format!(
                " returns ({})",
                elements
                    .iter()
                    .map(|element| element.display(db))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => format!(" returns ({})", ret.display(db)),
        });
    let label = format!("{name}({}){return_suffix}", parameters.join(", "));

    Some(CallableSignature { label, parameters })
}

fn function_for_def<'db>(db: &'db dyn hir_ty::Db, def: DefId<'db>) -> Option<FunctionDef<'db>> {
    let file = def.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    module
        .items(db)
        .iter()
        .find_map(|item| function_in_item(db, *item, def))
}

fn function_in_item<'db>(
    db: &'db dyn hir_ty::Db,
    item: Item<'db>,
    def: DefId<'db>,
) -> Option<FunctionDef<'db>> {
    match item {
        Item::FunctionDef(function) if function.def_id_value(db) == def => Some(function),
        Item::InstanceDef(instance) => instance
            .methods(db)
            .iter()
            .copied()
            .find(|method| method.def_id_value(db) == def),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match *item {
            ContractItem::FunctionDef(function) if function.def_id_value(db) == def => {
                Some(function)
            }
            ContractItem::FunctionDef(_)
            | ContractItem::TypeAlias(_)
            | ContractItem::AdtDef(_)
            | ContractItem::Error { .. } => None,
        }),
        Item::FunctionDef(_)
        | Item::TypeAlias(_)
        | Item::AdtDef(_)
        | Item::ClassDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => None,
    }
}

fn class_method_sig_for_def<'db>(
    db: &'db dyn hir_ty::Db,
    def: DefId<'db>,
    name: &str,
) -> Option<&'db FuncSig<'db>> {
    let file = def.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    module.items(db).iter().find_map(|item| match item {
        Item::ClassDef(class) if class.def_id_value(db) == def => class
            .methods(db)
            .iter()
            .find(|method| method.name.atom().text(db) == name),
        _ => None,
    })
}

fn display_source_return_suffix<'db>(db: &'db dyn hir_ty::Db, ret: TypeRef<'db>) -> String {
    match ret.kind(db) {
        TypeRefKind::Tuple { elems } if elems.atom().is_empty() => String::new(),
        TypeRefKind::Tuple { elems } => format!(
            " returns ({})",
            elems
                .atom()
                .iter()
                .map(|elem| hir_ty::display_type_ref_source(db, *elem))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => format!(" returns ({})", hir_ty::display_type_ref_source(db, ret)),
    }
}

fn adt_ctor_for_def<'db>(
    db: &'db dyn hir_ty::Db,
    def: DefId<'db>,
    index: usize,
) -> Option<hir::ast::item::AdtCtor<'db>> {
    let file = def.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    let adt = module
        .items(db)
        .iter()
        .find_map(|item| adt_in_item(db, *item, def))?;
    adt.ctors(db).get(index).cloned()
}

fn adt_ctor_source_params<'db>(
    db: &'db dyn hir_ty::Db,
    ctor: &hir::ast::item::AdtCtor<'db>,
) -> Vec<TypeRef<'db>> {
    let fields = *ctor.fields.atom();
    match ctor.field_count {
        0 => Vec::new(),
        1 => vec![fields],
        _ => match fields.kind(db) {
            TypeRefKind::Tuple { elems } => elems.atom().clone(),
            _ => vec![fields],
        },
    }
}

fn adt_in_item<'db>(
    db: &'db dyn hir_ty::Db,
    item: Item<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    match item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match *item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
            ContractItem::FunctionDef(_)
            | ContractItem::TypeAlias(_)
            | ContractItem::AdtDef(_)
            | ContractItem::Error { .. } => None,
        }),
        Item::FunctionDef(_)
        | Item::TypeAlias(_)
        | Item::AdtDef(_)
        | Item::ClassDef(_)
        | Item::InstanceDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => None,
    }
}

fn builtin_name(kind: hir_nameres::BuiltinKind) -> Option<&'static str> {
    Some(match kind {
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::True) => "true",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::False) => "false",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Unit) => "()",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Pair) => "pair",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Inl) => "inl",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Inr) => "inr",
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::Invoke) => "invoke",
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::PrimAddWord) => {
            "primAddWord"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::PrimEqWord) => {
            "primEqWord"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::WordToInteger) => {
            "wordToInteger"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::WordFromInteger) => {
            "wordFromInteger"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerAdd) => {
            "integerAdd"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerSub) => {
            "integerSub"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerMul) => {
            "integerMul"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerLt) => "integerLt",
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerEq) => "integerEq",
        hir_nameres::BuiltinKind::ClassMethod(hir_nameres::BuiltinClassMethod::InvokableInvoke) => {
            "invoke"
        }
        hir_nameres::BuiltinKind::ClassMethod(hir_nameres::BuiltinClassMethod::IntFromInteger) => {
            "fromInteger"
        }
        hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use lsp_types::Url;

    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    fn position_at(source: &str, world: &WorldState, uri: &Url, needle: &str) -> Position {
        let offset = source.find(needle).expect("needle") as u32;
        world
            .line_index(uri)
            .expect("line index")
            .byte_to_position(offset)
    }

    #[test]
    fn highlights_first_argument() {
        let source = "function f(a: word, b: word) returns (word) {\n  return a;\n}\n\nfunction main() returns (word) {\n  return f(1, 2);\n}\n";
        let (world, uri) = world_with_main(source);
        let position = position_at(source, &world, &uri, "1, 2");

        let help = handle_signature_help(&world, &uri, position).expect("signature help");

        assert_eq!(help.active_signature, Some(0));
        assert_eq!(help.active_parameter, Some(0));
        assert_eq!(help.signatures[0].active_parameter, Some(0));
    }

    #[test]
    fn highlights_second_argument_and_labels_signature() {
        let source = "function f(a: word, b: word) returns (word) {\n  return a;\n}\n\nfunction main() returns (word) {\n  return f(1, 2);\n}\n";
        let (world, uri) = world_with_main(source);
        let comma_offset = source.find(", 2").expect("comma") as u32 + 1;
        let position = world
            .line_index(&uri)
            .expect("line index")
            .byte_to_position(comma_offset);

        let help = handle_signature_help(&world, &uri, position).expect("signature help");
        let signature = &help.signatures[0];

        assert_eq!(help.active_parameter, Some(1));
        assert_eq!(signature.active_parameter, Some(1));
        assert!(
            signature.label.contains("f("),
            "expected function name in label, got {}",
            signature.label
        );
        assert!(
            signature.label.contains("a: word"),
            "expected first parameter in label, got {}",
            signature.label
        );
        assert!(
            signature.label.contains("b: word"),
            "expected second parameter in label, got {}",
            signature.label
        );
        assert!(
            signature.label.contains("returns (word)"),
            "expected return type in label, got {}",
            signature.label
        );
    }

    #[test]
    fn signature_help_uses_requested_module_when_unrelated_document_opened_first() {
        let unrelated = "function unrelated() returns (word) { return 0; }\n";
        let main = "function combine(a: word, b: word) returns (word) { return a; }\n\nfunction main() returns (word) {\n  return combine(1, 2);\n}\n";
        let unrelated_uri = Url::parse("file:///main/unrelated.solc").expect("unrelated uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let mut world = WorldState::new();
        assert!(world.open_document(unrelated_uri, unrelated.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let comma_offset = main.find(", 2").expect("comma") as u32 + 1;
        let position = world
            .line_index(&main_uri)
            .expect("line index")
            .byte_to_position(comma_offset);

        let help =
            handle_signature_help(&world, &main_uri, position).expect("signature help response");
        let signature = &help.signatures[0];

        assert_eq!(help.active_parameter, Some(1));
        assert!(
            signature.label.contains("combine("),
            "expected imported function signature, got {}",
            signature.label
        );
        assert!(signature.label.contains("a: word"));
        assert!(signature.label.contains("b: word"));
    }

    #[test]
    fn signature_help_resolves_imported_function_in_defining_module() {
        let unrelated = "function unrelated() returns (word) { return 0; }\n";
        let math = "function combine(a: word, b: word) returns (word) { return a; }\n\nexport { combine };\n";
        let main = "import {combine} from math;\n\nfunction main() returns (word) {\n  return combine(1, 2);\n}\n";
        let unrelated_uri = Url::parse("file:///main/unrelated.solc").expect("unrelated uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let mut world = WorldState::new();
        assert!(world.open_document(unrelated_uri, unrelated.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let comma_offset = main.find(", 2").expect("comma") as u32 + 1;
        let position = world
            .line_index(&main_uri)
            .expect("line index")
            .byte_to_position(comma_offset);

        let help =
            handle_signature_help(&world, &main_uri, position).expect("signature help response");
        let signature = &help.signatures[0];

        assert_eq!(help.active_parameter, Some(1));
        assert!(signature.label.contains("combine("));
        assert!(signature.label.contains("a: word"));
        assert!(signature.label.contains("b: word"));
    }

    #[test]
    fn signature_help_preserves_function_type_qualifiers() {
        let source = "\
function register(
  comptime callback: function(word) external view returns (bool),
  seed: word
) returns (bool) {
  return true;
}

function main() returns (bool) {
  return register(1, 2);
}
";
        let (world, uri) = world_with_main(source);
        let position = position_at(source, &world, &uri, "1, 2");
        let help = handle_signature_help(&world, &uri, position).expect("signature help");
        let signature = &help.signatures[0];

        assert_eq!(
            signature.label,
            "register(comptime callback: function(word) external view returns (bool), seed: word) returns (bool)"
        );
        assert_eq!(
            signature.parameters.as_ref().expect("parameters")[0].label,
            ParameterLabel::Simple(
                "comptime callback: function(word) external view returns (bool)".to_owned()
            )
        );
    }

    #[test]
    fn constructor_signature_help_preserves_function_type_qualifiers() {
        let source = "\
enum CallbackBox { CallbackBox(function(word) external view returns (bool)) }

function main() returns (CallbackBox) {
  return CallbackBox.CallbackBox(1);
}
";
        let (world, uri) = world_with_main(source);
        let position = position_at(source, &world, &uri, "1);");
        let help = handle_signature_help(&world, &uri, position).expect("signature help");
        let signature = &help.signatures[0];

        assert_eq!(
            signature.label,
            "CallbackBox(function(word) external view returns (bool)) returns (adt:CallbackBox)"
        );
        assert_eq!(
            signature.parameters.as_ref().expect("parameters")[0].label,
            ParameterLabel::Simple("function(word) external view returns (bool)".to_owned())
        );
    }
}
