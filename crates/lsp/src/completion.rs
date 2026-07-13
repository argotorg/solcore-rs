//! Completion support over the wasm-clean LSP core.

use hir::{
    anchor::DefId,
    ast::{
        function::{FuncParam, PatKind, StmtKind},
        item::{ContractItem, Item, Module},
    },
    input::SourceFile,
    nameres::{
        BuiltinKind, DefResolutionKind, ImportedNames, LocalBinding, Namespace, Resolution,
        ScopeEntry, TypeVarBinding,
    },
    span::{Span, Spanned, SpannedElem},
};
use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position, Url};

use crate::{
    resolve::{function_owning_offset, module_id_for_uri},
    state::WorldState,
};

const KEYWORDS: &[&str] = &[
    "contract",
    "import",
    "export",
    "as",
    "let",
    "data",
    "class",
    "forall",
    "instance",
    "if",
    "else",
    "for",
    "switch",
    "type",
    "case",
    "default",
    "match",
    "public",
    "payable",
    "function",
    "constructor",
    "fallback",
    "return",
    "leave",
    "continue",
    "break",
    "lam",
    "assembly",
    "pragma",
    "true",
    "false",
];

/// Computes completion items at a source position.
pub fn handle_completion(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<CompletionResponse> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let current_module = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, current_module);
    let scope = hir::nameres::item_scope_facts(db, module);
    let owner = function_owning_offset(db, module, file, offset);
    let enclosing_contract = owner
        .as_ref()
        .and_then(|owner| owner.enclosing_contract)
        .or_else(|| contract_at_offset(db, module, file, offset));

    if let Some(context) = qualified_completion_context(line_index.text(), offset) {
        let mut completions = CompletionAccumulator::default();
        add_qualified_completions(
            db,
            &scope,
            enclosing_contract,
            &env,
            &context,
            &mut completions,
        );
        return Some(CompletionResponse::Array(completions.finish()));
    }

    let mut completions = CompletionAccumulator::default();
    add_keyword_completions(&mut completions);
    add_item_scope_completions(&scope, &mut completions);
    add_imported_completions(db, &env, &mut completions);

    if let Some(owner) = owner {
        add_body_completions(
            db,
            module,
            &scope,
            file,
            offset,
            owner.function,
            owner.root_body,
            owner.enclosing_contract,
            owner.inherited_type_vars,
            &env,
            &mut completions,
        );
    }

    Some(CompletionResponse::Array(completions.finish()))
}

#[derive(Debug, PartialEq, Eq)]
struct QualifiedCompletionContext {
    qualifier: String,
    member_prefix: String,
}

fn qualified_completion_context(text: &str, offset: u32) -> Option<QualifiedCompletionContext> {
    let before_cursor = text.get(..usize::try_from(offset).ok()?)?;
    let path_start = before_cursor
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!is_qualified_path_char(ch)).then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let path = &before_cursor[path_start..];
    let dot = path.rfind('.')?;
    let qualifier = &path[..dot];
    let member_prefix = &path[dot + 1..];

    if qualifier.is_empty()
        || qualifier.split('.').any(|segment| !is_identifier(segment))
        || (!member_prefix.is_empty() && !is_identifier(member_prefix))
    {
        return None;
    }

    Some(QualifiedCompletionContext {
        qualifier: qualifier.to_owned(),
        member_prefix: member_prefix.to_owned(),
    })
}

fn is_qualified_path_char(ch: char) -> bool {
    ch == '.' || ch == '-' || ch == '_' || ch.is_alphanumeric()
}

fn is_identifier(text: &str) -> bool {
    text.split('-').all(is_identifier_chunk)
}

fn is_identifier_chunk(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(char::is_alphabetic)
        && chars.all(|ch| ch == '_' || ch.is_alphanumeric())
}

fn add_qualified_completions<'db>(
    db: &'db vfs::AnalysisHost,
    scope: &hir::nameres::ItemScopeFacts<'db>,
    enclosing_contract: Option<DefId<'db>>,
    imports: &dyn ImportedNames<'db>,
    context: &QualifiedCompletionContext,
    completions: &mut CompletionAccumulator,
) {
    add_qualified_scope_entries(&scope.terms, context, completions);
    add_qualified_scope_entries(&scope.types, context, completions);
    add_qualified_scope_entries(&scope.modules, context, completions);

    if let Some(contract) = enclosing_contract.and_then(|contract| scope.contract_scope(contract)) {
        add_qualified_scope_entries(&contract.terms, context, completions);
        add_qualified_scope_entries(&contract.types, context, completions);
    }

    for namespace in [Namespace::Term, Namespace::Type, Namespace::Module] {
        for name in imports.candidate_names(db, namespace) {
            let Some(member) = direct_qualified_member(&name, context) else {
                continue;
            };
            let resolution = imports.imported(db, namespace, &name);
            let kind = resolution.as_ref().map_or_else(
                || completion_kind_for_namespace(namespace),
                completion_kind_for_resolution,
            );
            let detail = resolution
                .as_ref()
                .map_or_else(|| detail_for_namespace(namespace), detail_for_resolution);
            completions.push(member.to_owned(), kind, Some(detail));
        }
    }
}

fn add_qualified_scope_entries(
    entries: &hir::nameres::NamespaceTable<'_>,
    context: &QualifiedCompletionContext,
    completions: &mut CompletionAccumulator,
) {
    for entry in entries {
        let Some(member) = direct_qualified_member(&entry.name, context) else {
            continue;
        };
        completions.push(
            member.to_owned(),
            completion_kind_for_resolution(&entry.resolution),
            Some(detail_for_resolution(&entry.resolution)),
        );
    }
}

fn direct_qualified_member<'a>(
    name: &'a str,
    context: &QualifiedCompletionContext,
) -> Option<&'a str> {
    let rest = name.strip_prefix(&context.qualifier)?.strip_prefix('.')?;
    (!rest.is_empty() && !rest.contains('.') && rest.starts_with(&context.member_prefix))
        .then_some(rest)
}

fn contract_at_offset<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<DefId<'db>> {
    module.items(db).iter().find_map(|item| {
        let Item::ContractDef(contract) = *item else {
            return None;
        };
        contract.items(db).iter().find_map(|item| {
            let ContractItem::FunctionDef(function) = *item else {
                return None;
            };
            let body = function.body(db)?;
            let span = body.span(db).resolve_to_absolute(db);
            (span.file() == file
                && span.start().as_u32() <= offset
                && offset <= span.end().as_u32())
            .then_some(contract.def_id_value(db))
        })
    })
}

#[derive(Default)]
struct CompletionAccumulator {
    items: Vec<CompletionItem>,
}

impl CompletionAccumulator {
    fn push(&mut self, label: String, kind: CompletionItemKind, detail: Option<&'static str>) {
        if self
            .items
            .iter()
            .any(|item| item.label == label && item.kind == Some(kind))
        {
            return;
        }

        self.items.push(CompletionItem {
            label,
            kind: Some(kind),
            detail: detail.map(str::to_owned),
            ..CompletionItem::default()
        });
    }

    fn finish(mut self) -> Vec<CompletionItem> {
        self.items.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| kind_rank(left.kind).cmp(&kind_rank(right.kind)))
        });
        self.items
    }
}

fn add_keyword_completions(completions: &mut CompletionAccumulator) {
    for keyword in KEYWORDS {
        completions.push(
            (*keyword).to_owned(),
            CompletionItemKind::KEYWORD,
            Some("keyword"),
        );
    }
}

fn add_item_scope_completions<'db>(
    scope: &hir::nameres::ItemScopeFacts<'db>,
    completions: &mut CompletionAccumulator,
) {
    for entry in &scope.terms {
        add_scope_entry_completion(entry, completions);
    }
    for entry in &scope.types {
        add_scope_entry_completion(entry, completions);
    }
    for entry in &scope.modules {
        add_scope_entry_completion(entry, completions);
    }
}

fn add_scope_entry_completion(entry: &ScopeEntry<'_>, completions: &mut CompletionAccumulator) {
    completions.push(
        entry.name.clone(),
        completion_kind_for_resolution(&entry.resolution),
        Some(detail_for_resolution(&entry.resolution)),
    );
}

fn add_imported_completions<'db>(
    db: &'db vfs::AnalysisHost,
    imports: &dyn ImportedNames<'db>,
    completions: &mut CompletionAccumulator,
) {
    for namespace in [Namespace::Term, Namespace::Type, Namespace::Module] {
        let mut names = imports.candidate_names(db, namespace);
        names.sort();
        names.dedup();
        for name in names {
            let resolution = imports.imported(db, namespace, &name);
            let kind = resolution.as_ref().map_or_else(
                || completion_kind_for_namespace(namespace),
                completion_kind_for_resolution,
            );
            let detail = resolution
                .as_ref()
                .map_or_else(|| detail_for_namespace(namespace), detail_for_resolution);
            completions.push(name, kind, Some(detail));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_body_completions<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    scope: &hir::nameres::ItemScopeFacts<'db>,
    file: SourceFile,
    offset: u32,
    function: hir::ast::item::FunctionDef<'db>,
    root_body: hir::ast::function::FuncBody<'db>,
    enclosing_contract: Option<hir::anchor::DefId<'db>>,
    mut type_vars: Vec<TypeVarBinding<'db>>,
    imports: &dyn ImportedNames<'db>,
    completions: &mut CompletionAccumulator,
) {
    let sig = function.sig(db);
    type_vars.extend(hir::nameres::type_var_bindings(
        function.def_id_value(db),
        &sig.type_vars,
    ));

    for param in sig.params.atom() {
        if let Some(name) = param_name(param) {
            completions.push(
                name.atom().text(db).to_owned(),
                CompletionItemKind::VARIABLE,
                Some("parameter"),
            );
        }
    }
    for type_var in &type_vars {
        completions.push(
            type_var.name.atom().text(db).to_owned(),
            CompletionItemKind::TYPE_PARAMETER,
            Some("type parameter"),
        );
    }
    if let Some(contract) = enclosing_contract.and_then(|contract| scope.contract_scope(contract)) {
        for entry in &contract.terms {
            add_scope_entry_completion(entry, completions);
        }
        for entry in &contract.types {
            add_scope_entry_completion(entry, completions);
        }
        for field in &contract.fields {
            completions.push(field.name.clone(), CompletionItemKind::FIELD, Some("field"));
        }
    }

    let context = hir::nameres::BodyResolutionContext {
        module,
        enclosing_contract,
        params: hir::nameres::param_bindings(sig.params.atom()),
        type_vars,
    };
    let body_map = hir::nameres::resolve_body_with_imports_and_policy(
        db,
        root_body,
        &context,
        imports,
        hir::nameres::NameresDiagnosticPolicy::Emit,
    );

    for binding in &body_map.stmt_bindings {
        let Resolution::Local(LocalBinding::Let { .. }) = &binding.resolution else {
            continue;
        };
        let stmt = binding.body.stmts(db).get(binding.stmt);
        if let StmtKind::Let { name, .. } = &stmt.kind {
            add_local_if_visible(db, file, offset, name, completions);
        }
    }
    for binding in &body_map.pats {
        let Resolution::Local(LocalBinding::Pattern { .. }) = &binding.resolution else {
            continue;
        };
        let pat = binding.body.pats(db).get(binding.pat);
        if let PatKind::Var(name) = &pat.kind {
            add_local_if_visible(db, file, offset, name, completions);
        }
    }
}

fn param_name<'a, 'db>(
    param: &'a FuncParam<'db>,
) -> Option<&'a SpannedElem<'db, hir::ast::Ident<'db>>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name),
        FuncParam::Error { .. } => None,
    }
}

fn add_local_if_visible<'db>(
    db: &'db vfs::AnalysisHost,
    file: SourceFile,
    offset: u32,
    name: &SpannedElem<'db, hir::ast::Ident<'db>>,
    completions: &mut CompletionAccumulator,
) {
    if span_ends_before_offset(db, name.span(db), file, offset) {
        completions.push(
            name.atom().text(db).to_owned(),
            CompletionItemKind::VARIABLE,
            Some("local"),
        );
    }
}

fn span_ends_before_offset<'db>(
    db: &'db vfs::AnalysisHost,
    span: Span<'db>,
    file: SourceFile,
    offset: u32,
) -> bool {
    let absolute = span.resolve_to_absolute(db);
    absolute.file() == file && absolute.end().as_u32() <= offset
}

fn completion_kind_for_namespace(namespace: Namespace) -> CompletionItemKind {
    match namespace {
        Namespace::Type => CompletionItemKind::STRUCT,
        Namespace::Term => CompletionItemKind::VALUE,
        Namespace::Field => CompletionItemKind::FIELD,
        Namespace::Module => CompletionItemKind::MODULE,
    }
}

fn detail_for_namespace(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::Type => "type",
        Namespace::Term => "term",
        Namespace::Field => "field",
        Namespace::Module => "module",
    }
}

fn completion_kind_for_resolution(resolution: &Resolution<'_>) -> CompletionItemKind {
    match resolution {
        Resolution::Def {
            kind: DefResolutionKind::Function,
            ..
        } => CompletionItemKind::FUNCTION,
        Resolution::Def {
            kind: DefResolutionKind::Contract,
            ..
        } => CompletionItemKind::CLASS,
        Resolution::Def {
            kind: DefResolutionKind::Adt,
            ..
        } => CompletionItemKind::ENUM,
        Resolution::Def {
            kind: DefResolutionKind::TypeAlias,
            ..
        } => CompletionItemKind::STRUCT,
        Resolution::Def {
            kind: DefResolutionKind::Class,
            ..
        } => CompletionItemKind::INTERFACE,
        Resolution::Def {
            kind: DefResolutionKind::Instance,
            ..
        } => CompletionItemKind::CLASS,
        Resolution::Ctor { .. } => CompletionItemKind::CONSTRUCTOR,
        Resolution::Local(LocalBinding::TypeVar(_)) => CompletionItemKind::TYPE_PARAMETER,
        Resolution::Local(_) | Resolution::Param(_) => CompletionItemKind::VARIABLE,
        Resolution::Field(_) => CompletionItemKind::FIELD,
        Resolution::ClassMethod { .. } => CompletionItemKind::METHOD,
        Resolution::Module(_) => CompletionItemKind::MODULE,
        Resolution::Builtin(BuiltinKind::Type(_)) => CompletionItemKind::STRUCT,
        Resolution::Builtin(BuiltinKind::Class(_)) => CompletionItemKind::INTERFACE,
        Resolution::Builtin(BuiltinKind::Constructor(_)) => CompletionItemKind::CONSTRUCTOR,
        Resolution::Builtin(BuiltinKind::Function(_)) => CompletionItemKind::FUNCTION,
        Resolution::Builtin(BuiltinKind::ClassMethod(_)) => CompletionItemKind::METHOD,
        Resolution::DotCtorDeferred | Resolution::Err => CompletionItemKind::TEXT,
    }
}

fn detail_for_resolution(resolution: &Resolution<'_>) -> &'static str {
    match resolution {
        Resolution::Def {
            kind: DefResolutionKind::Function,
            ..
        } => "function",
        Resolution::Def {
            kind: DefResolutionKind::Contract,
            ..
        } => "contract",
        Resolution::Def {
            kind: DefResolutionKind::Adt,
            ..
        } => "data",
        Resolution::Def {
            kind: DefResolutionKind::TypeAlias,
            ..
        } => "type alias",
        Resolution::Def {
            kind: DefResolutionKind::Class,
            ..
        } => "class",
        Resolution::Def {
            kind: DefResolutionKind::Instance,
            ..
        } => "instance",
        Resolution::Ctor { .. } => "constructor",
        Resolution::Local(LocalBinding::TypeVar(_)) => "type parameter",
        Resolution::Local(_) => "local",
        Resolution::Param(_) => "parameter",
        Resolution::Field(_) => "field",
        Resolution::ClassMethod { .. } => "class method",
        Resolution::Module(_) => "module",
        Resolution::Builtin(BuiltinKind::Type(_)) => "builtin type",
        Resolution::Builtin(BuiltinKind::Class(_)) => "builtin class",
        Resolution::Builtin(BuiltinKind::Constructor(_)) => "builtin constructor",
        Resolution::Builtin(BuiltinKind::Function(_)) => "builtin function",
        Resolution::Builtin(BuiltinKind::ClassMethod(_)) => "builtin class method",
        Resolution::DotCtorDeferred => "constructor",
        Resolution::Err => "unresolved",
    }
}

fn kind_rank(kind: Option<CompletionItemKind>) -> u8 {
    let Some(kind) = kind else {
        return u8::MAX;
    };
    if kind == CompletionItemKind::KEYWORD {
        0
    } else if kind == CompletionItemKind::FUNCTION {
        1
    } else if kind == CompletionItemKind::METHOD {
        2
    } else if kind == CompletionItemKind::CONSTRUCTOR {
        3
    } else if kind == CompletionItemKind::VARIABLE {
        4
    } else if kind == CompletionItemKind::FIELD {
        5
    } else if kind == CompletionItemKind::ENUM {
        6
    } else if kind == CompletionItemKind::STRUCT {
        7
    } else if kind == CompletionItemKind::CLASS {
        8
    } else if kind == CompletionItemKind::INTERFACE {
        9
    } else if kind == CompletionItemKind::MODULE {
        10
    } else if kind == CompletionItemKind::TYPE_PARAMETER {
        11
    } else {
        100
    }
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
    fn function_body_completion_includes_params_locals_and_top_level_items() {
        let source = "\
function helper() -> word {
  return 1;
}

function main(input: word) -> word {
  let local = input;
  return local;
}
";
        let (world, uri) = world_with_main(source);
        let offset = (source.find("return local").expect("return local") + "return ".len()) as u32;
        let position = world
            .line_index(&uri)
            .expect("line index")
            .byte_to_position(offset);

        let items =
            completion_items(handle_completion(&world, &uri, position).expect("completion"));

        assert_completion(&items, "input", CompletionItemKind::VARIABLE);
        assert_completion(&items, "local", CompletionItemKind::VARIABLE);
        assert_completion(&items, "helper", CompletionItemKind::FUNCTION);
    }

    #[test]
    fn completion_includes_language_keywords() {
        let source = "function main() -> word {\n  return 1;\n}\n";
        let (world, uri) = world_with_main(source);
        let offset = source.find('1').expect("literal") as u32;
        let position = world
            .line_index(&uri)
            .expect("line index")
            .byte_to_position(offset);

        let items =
            completion_items(handle_completion(&world, &uri, position).expect("completion"));

        assert_completion(&items, "function", CompletionItemKind::KEYWORD);
    }

    #[test]
    fn completion_uses_requested_module_when_unrelated_document_opened_first() {
        let unrelated = "function unrelated() -> word { return 0; }\n";
        let math =
            "function combine(a: word, b: word) -> word { return a + b; }\n\nexport { combine };\n";
        let main =
            "import math.{combine};\n\nfunction main() -> word {\n  return combine(1, 2);\n}\n";
        let unrelated_uri = Url::parse("file:///main/unrelated.solc").expect("unrelated uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let mut world = WorldState::new();
        assert!(world.open_document(unrelated_uri, unrelated.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let offset = (main.find("return combine").expect("return call") + "return ".len()) as u32;
        let position = world
            .line_index(&main_uri)
            .expect("line index")
            .byte_to_position(offset);

        let items = completion_items(
            handle_completion(&world, &main_uri, position).expect("completion response"),
        );

        assert_completion(&items, "combine", CompletionItemKind::FUNCTION);
    }

    #[test]
    fn trailing_dot_module_completion_is_member_only_and_respects_exports() {
        let math = "\
function visible() -> word { return 1; }
function hidden() -> word { return 2; }
data Color = Red | Green;
export { visible, Color(Red, Green) };
";
        let main = "\
import math;
function main() -> word {
  return math.;
}
";
        let (world, main_uri) = world_with_module(main, "math.solc", math);
        let items = completion_at(&world, &main_uri, main, "math.");

        assert_completion(&items, "visible", CompletionItemKind::FUNCTION);
        assert_completion(&items, "Color", CompletionItemKind::ENUM);
        assert_no_completion(&items, "hidden");
        assert_no_completion(&items, "Red");
        assert_no_completion(&items, "function");
        assert!(
            items.iter().all(|item| !item.label.contains('.')),
            "expected direct member labels only, got {items:#?}"
        );
    }

    #[test]
    fn qualified_completion_filters_a_typed_member_prefix() {
        let math = "\
function visible() -> word { return 1; }
function value() -> word { return 2; }
export { visible, value };
";
        let main = "\
import math;
function main() -> word {
  return math.vis;
}
";
        let (world, main_uri) = world_with_module(main, "math.solc", math);
        let items = completion_at(&world, &main_uri, main, "math.vis");

        assert_completion(&items, "visible", CompletionItemKind::FUNCTION);
        assert_no_completion(&items, "value");
    }

    #[test]
    fn qualified_completion_includes_contract_local_adt_constructors() {
        let source = "\
contract Palette {
  data Color = Red | Green;

  function main() -> word {
    return Color.;
  }
}
";
        let (world, uri) = world_with_main(source);
        let items = completion_at(&world, &uri, source, "Color.");

        assert_completion(&items, "Red", CompletionItemKind::CONSTRUCTOR);
        assert_completion(&items, "Green", CompletionItemKind::CONSTRUCTOR);
        assert_no_completion(&items, "Color.Red");
    }

    #[test]
    fn qualified_completion_includes_imported_class_methods() {
        let classes = "\
forall a . class a : Eq {
  function eq(x: a, y: a) -> bool;
  function unequal(x: a, y: a) -> bool;
}
export { Eq };
";
        let main = "\
import classes.{Eq};
function main() -> word {
  return Eq.;
}
";
        let (world, main_uri) = world_with_module(main, "classes.solc", classes);
        let items = completion_at(&world, &main_uri, main, "Eq.");

        assert_completion(&items, "eq", CompletionItemKind::METHOD);
        assert_completion(&items, "unequal", CompletionItemKind::METHOD);
        assert_no_completion(&items, "Eq.eq");
    }

    #[test]
    fn qualified_context_accepts_only_lexer_shaped_hyphenated_identifiers() {
        let valid = "foo-bar.member-prefix";
        assert_eq!(
            qualified_completion_context(valid, valid.len() as u32),
            Some(QualifiedCompletionContext {
                qualifier: "foo-bar".to_owned(),
                member_prefix: "member-prefix".to_owned(),
            })
        );

        for invalid in ["foo-.member", "foo--bar.member", "foo.-member"] {
            assert_eq!(
                qualified_completion_context(invalid, invalid.len() as u32),
                None,
                "unexpectedly accepted {invalid:?}"
            );
        }
    }

    fn world_with_module(main: &str, module_path: &str, module_source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let module_uri = Url::parse(&format!("file:///main/{module_path}")).expect("module uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(module_uri, module_source.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        (world, main_uri)
    }

    fn completion_at(
        world: &WorldState,
        uri: &Url,
        source: &str,
        cursor_after: &str,
    ) -> Vec<CompletionItem> {
        let offset =
            (source.find(cursor_after).expect("completion marker") + cursor_after.len()) as u32;
        let position = world
            .line_index(uri)
            .expect("line index")
            .byte_to_position(offset);
        completion_items(handle_completion(world, uri, position).expect("completion response"))
    }

    fn completion_items(response: CompletionResponse) -> Vec<CompletionItem> {
        match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        }
    }

    fn assert_completion(items: &[CompletionItem], label: &str, kind: CompletionItemKind) {
        assert!(
            items
                .iter()
                .any(|item| item.label == label && item.kind == Some(kind)),
            "expected completion {label:?} with kind {kind:?}, got {items:#?}"
        );
    }

    fn assert_no_completion(items: &[CompletionItem], label: &str) {
        assert!(
            items.iter().all(|item| item.label != label),
            "did not expect completion {label:?}, got {items:#?}"
        );
    }
}
