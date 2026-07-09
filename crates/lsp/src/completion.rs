//! Completion support over the wasm-clean LSP core.

use hir::{
    ast::{
        function::{FuncParam, PatKind, StmtKind},
        item::Module,
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
    resolve::function_owning_offset,
    state::{WorldState, uri_to_vfs_path},
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
    let path = uri_to_vfs_path(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let entry = world.workspace().entry_module()?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, entry);
    let scope = hir::nameres::item_scope_facts(db, module);

    let mut completions = CompletionAccumulator::default();
    add_keyword_completions(&mut completions);
    add_item_scope_completions(&scope, &mut completions);
    add_imported_completions(db, &env, &mut completions);

    if let Some(owner) = function_owning_offset(db, module, file, offset) {
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

    // NOTE(codex): Member completion after `.` is advertised as a trigger, but
    // this first cut returns the same in-scope set until field/method lookup is
    // exposed as a reusable compiler query.
    Some(CompletionResponse::Array(completions.finish()))
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
}
