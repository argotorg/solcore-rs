//! Go-to-definition support over the wasm-clean LSP core.

use hir::{
    anchor::{DefId, resolve_def_location},
    ast::{
        function::{ExprKind, FuncBody, FuncParam, PatKind, StmtKind},
        item::{AdtDef, ClassDef, ContractDef, ContractItem, FunctionDef, Item, Module},
    },
    diag::{AbsoluteSpan, Offset},
    nameres::{self as hir_nameres, FieldId, LocalBinding, ParamId, Resolution, TypeVarBinding},
    span::{Span, Spanned},
};
use lsp_types::{GotoDefinitionResponse, Location, Position, Url};

use crate::{
    references::{import_export_target_at, reference_target_at, target_declaration_span},
    resolve::{function_owning_offset, innermost_expr, module_id_for_uri},
    state::WorldState,
};

/// Computes the target definition location for the symbol at a source position.
pub fn handle_definition(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let current_module = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, current_module);

    if let Some(location) = (|| {
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
        let target = resolution_target_span(db, module, &env, resolution)?;
        location_for_span(world, db, target)
    })() {
        return Some(GotoDefinitionResponse::Scalar(location));
    }

    if let Some(location) =
        import_module_location_at(world, db, module, current_module, file, offset)
    {
        return Some(GotoDefinitionResponse::Scalar(location));
    }

    if let Some(location) = (|| {
        let target = reference_target_at(world, uri, position)?;
        let span = target_declaration_span(db, &target)?;
        location_for_span(world, db, span)
    })() {
        return Some(GotoDefinitionResponse::Scalar(location));
    }

    let target = import_export_target_at(world, uri, position)?;
    let span = target_declaration_span(db, &target)?;
    let location = location_for_span(world, db, span)?;

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
    imports: &nameres::ModuleEnv<'db>,
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
        Resolution::Module(module_ref) => imports
            .modules
            .get(&module_ref.name)
            .copied()
            .or_else(|| {
                imports
                    .module_origins
                    .get(&module_ref.name)
                    .copied()
                    .flatten()
            })
            .and_then(|module| module_start_span(db, module)),
        Resolution::DotCtorDeferred | Resolution::Builtin(_) | Resolution::Err => None,
    }
}

fn import_module_location_at<'db>(
    world: &WorldState,
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    current_module: nameres::ModuleId<'db>,
    file: hir::input::SourceFile,
    offset: u32,
) -> Option<Location> {
    for item in module.items(db) {
        let Item::Import(import) = *item else {
            continue;
        };
        let on_path = import
            .path_elems(db)
            .iter()
            .any(|segment| span_contains_offset(db, segment.span(db), file, offset));
        let on_alias = import
            .alias_elem(db)
            .is_some_and(|alias| span_contains_offset(db, alias.span(db), file, offset));
        if !on_path && !on_alias {
            continue;
        }

        let target = nameres::resolve_direct_import_target(db, current_module, import).ok()?;
        let span = module_start_span(db, target)?;
        return location_for_span(world, db, span);
    }

    None
}

fn module_start_span<'db>(
    db: &'db dyn hir_ty::Db,
    module: nameres::ModuleId<'db>,
) -> Option<AbsoluteSpan> {
    let file = db.module_file(module)?;
    Some(AbsoluteSpan::new(file, Offset::new(0), Offset::new(0)))
}

fn span_contains_offset<'db>(
    db: &'db dyn hir_ty::Db,
    span: Span<'db>,
    file: hir::input::SourceFile,
    offset: u32,
) -> bool {
    let absolute = span.resolve_to_absolute(db);
    absolute.file() == file
        && absolute.start().as_u32() <= offset
        && offset < absolute.end().as_u32()
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
    let uri = world.client_uri_for_vfs_url(span.file().url(db).as_str())?;
    let line_index = world.line_index(&uri)?;
    let range = line_index.range(span.start().as_u32(), span.end().as_u32());

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

    fn world_with_main_and_math(main: &str, math: &str) -> (WorldState, Url, Url) {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri.clone(), math.to_owned()));
        (world, main_uri, math_uri)
    }

    fn world_with_main_and_nested(
        main: &str,
        nested_path: &str,
        nested: &str,
    ) -> (WorldState, Url, Url) {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let nested_uri =
            Url::parse(&format!("file:///main/{nested_path}.solc")).expect("nested uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(nested_uri.clone(), nested.to_owned()));
        (world, main_uri, nested_uri)
    }

    fn scalar_definition(world: &WorldState, uri: &Url, offset: u32) -> Location {
        let line_index = world.line_index(uri).expect("line index");
        let response =
            handle_definition(world, uri, line_index.byte_to_position(offset)).expect("definition");
        let GotoDefinitionResponse::Scalar(location) = response else {
            panic!("expected scalar definition response");
        };
        location
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

    #[test]
    fn definition_of_import_selector_name_points_to_imported_declaration() {
        let main = "import math.{double};\nfunction main() -> word { return double(21); }\n";
        let math = "function double(x: word) -> word { return x + x; }\nexport { double };\n";
        let (world, main_uri, math_uri) = world_with_main_and_math(main, math);
        let main_index = world.line_index(&main_uri).expect("main line index");
        let math_index = world.line_index(&math_uri).expect("math line index");
        let import = main.find("double").expect("import") as u32;
        let declaration = math.find("double").expect("declaration") as u32;

        let response = handle_definition(&world, &main_uri, main_index.byte_to_position(import))
            .expect("definition");
        let GotoDefinitionResponse::Scalar(location) = response else {
            panic!("expected scalar definition response");
        };

        assert_eq!(location.uri, math_uri);
        assert_eq!(
            location.range,
            math_index.range(declaration, declaration + "double".len() as u32)
        );
    }

    #[test]
    fn definition_in_embedded_std_is_not_returned_as_an_unopenable_uri() {
        let source = "import std.{addWord};\nfunction main() -> word { return addWord(1, 2); }\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let call = source.rfind("addWord").expect("call") as u32;

        assert_eq!(
            handle_definition(&world, &uri, line_index.byte_to_position(call)),
            None
        );
    }

    #[test]
    fn definition_of_cross_file_type_ref_points_to_type_declaration() {
        let main = "\
import models.{Box};
function wrap(value: word) -> Box {
  let boxed: Box = Box(value);
  return boxed;
}
";
        let models = "data Box = Box(word);\nexport { Box };\n";
        let (world, main_uri, models_uri) = world_with_main_and_nested(main, "models", models);
        let models_index = world.line_index(&models_uri).expect("models line index");
        let type_ref = (main.find("boxed: Box").expect("local type") + "boxed: ".len()) as u32;
        let declaration = models.find("Box").expect("type declaration") as u32;

        let location = scalar_definition(&world, &main_uri, type_ref);

        assert_eq!(location.uri, models_uri);
        assert_eq!(
            location.range,
            models_index.range(declaration, declaration + "Box".len() as u32)
        );
    }

    #[test]
    fn definition_on_type_declaration_points_to_itself() {
        let source = "data Choice = Left | Right;\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let declaration = source.find("Choice").expect("declaration") as u32;

        let location = scalar_definition(&world, &uri, declaration);

        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range,
            line_index.range(declaration, declaration + "Choice".len() as u32)
        );
    }

    #[test]
    fn definition_of_predicate_points_to_class_declaration() {
        let source = "\
forall a. class a:Comparable {
  function compare(x: a, y: a) -> word;
}

forall a. a:Comparable =>
function keep(x: a) -> a { return x; }
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let declaration = source.find("Comparable").expect("class declaration") as u32;
        let predicate = source.rfind("Comparable").expect("predicate") as u32;

        let location = scalar_definition(&world, &uri, predicate);

        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range,
            line_index.range(declaration, declaration + "Comparable".len() as u32)
        );
    }

    #[test]
    fn definition_of_constructor_pattern_points_to_constructor_declaration() {
        let source = "\
data Choice = Left(word) | Right;

function unwrap(value: Choice) -> word {
  match value {
  | Choice.Left(x) => return x;
  | Choice.Right => return 0;
  }
}
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let declaration = source.find("Left").expect("constructor declaration") as u32;
        let pattern = source.rfind("Left").expect("constructor pattern") as u32;

        let location = scalar_definition(&world, &uri, pattern);

        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range,
            line_index.range(declaration, declaration + "Left".len() as u32)
        );
    }

    #[test]
    fn definition_of_import_path_and_module_qualifier_points_to_module_start() {
        let main = "import foo.bar;\nfunction main() -> word { return foo.bar.value(); }\n";
        let bar = "export { value };\nfunction value() -> word { return 7; }\n";
        let (world, main_uri, bar_uri) = world_with_main_and_nested(main, "foo/bar", bar);
        let bar_index = world.line_index(&bar_uri).expect("bar line index");
        let expected = bar_index.range(0, 0);
        let import_foo = main.find("foo.bar").expect("import path") as u32;
        let import_bar = import_foo + "foo.".len() as u32;
        let qualifier_foo = main.rfind("foo.bar").expect("module qualifier") as u32;
        let qualifier_bar = qualifier_foo + "foo.".len() as u32;

        for offset in [import_foo, import_bar, qualifier_foo, qualifier_bar] {
            let location = scalar_definition(&world, &main_uri, offset);
            assert_eq!(location.uri, bar_uri, "offset {offset}");
            assert_eq!(location.range, expected, "offset {offset}");
        }
    }

    #[test]
    fn definition_of_exact_module_qualifier_wins_over_shared_navigation_origin() {
        let main =
            "import foo.bar;\nimport foo;\nfunction main() -> word { return foo.value(); }\n";
        let foo = "export { value };\nfunction value() -> word { return 1; }\n";
        let bar = "export { value };\nfunction value() -> word { return 2; }\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let foo_uri = Url::parse("file:///main/foo.solc").expect("foo uri");
        let bar_uri = Url::parse("file:///main/foo/bar.solc").expect("bar uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(foo_uri.clone(), foo.to_owned()));
        assert!(world.open_document(bar_uri, bar.to_owned()));
        let foo_index = world.line_index(&foo_uri).expect("foo line index");
        let qualifier = main.rfind("foo.value").expect("exact module qualifier") as u32;

        let location = scalar_definition(&world, &main_uri, qualifier);

        assert_eq!(location.uri, foo_uri);
        assert_eq!(location.range, foo_index.range(0, 0));
    }
}
