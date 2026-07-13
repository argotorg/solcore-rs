//! Semantic token support over the wasm-clean LSP core.

use hir::{
    anchor::DefId,
    ast::{
        function::{ExprKind, FuncBody, FuncParam, FuncSig, PatKind, StmtKind},
        item::{ContractItem, FunctionDef, Item, Module},
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    input::SourceFile,
    nameres::{
        self as hir_nameres, BuiltinKind, DefResolutionKind, LocalBinding, Resolution,
        TypeVarBinding,
    },
    span::{Span, Spanned},
};
use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensResult,
    Url,
};

use crate::{analysis::with_analysis_stack, resolve::module_id_for_uri, state::WorldState};

/// Semantic token types advertised by the server and used by the encoder.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::TYPE,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::ENUM_MEMBER,
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::NUMBER,
    SemanticTokenType::STRING,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::COMMENT,
];

/// Semantic token modifiers advertised by the server and used by the encoder.
pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::READONLY,
];

/// Computes full-document semantic tokens for one open source document.
pub fn handle_semantic_tokens_full(world: &WorldState, uri: &Url) -> Option<SemanticTokensResult> {
    with_analysis_stack(|| handle_semantic_tokens_full_inner(world, uri))
}

fn handle_semantic_tokens_full_inner(
    world: &WorldState,
    uri: &Url,
) -> Option<SemanticTokensResult> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let current_module = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, current_module);
    let scope = hir_nameres::item_scope_facts(db, module);
    let item_facts = hir_nameres::resolve_item_type_facts_with_imports(db, module, &scope, &env);

    let mut collector = TokenCollector::new(db, file);
    collect_declaration_tokens(db, module, &mut collector);
    collect_item_type_tokens(db, &item_facts, &mut collector);
    collect_body_tokens(db, module, &env, &mut collector);

    // NOTE(codex): This first pass emits HIR-derived name tokens only. Keywords,
    // literals, operators, and comments remain covered by client syntax
    // highlighting until the LSP core has a parser-token stream to reuse.
    Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: encode_tokens(line_index, collector.finish()),
    }))
}

#[derive(Clone, Copy)]
enum TokenKind {
    Function,
    Type,
    Variable,
    Parameter,
    Property,
    EnumMember,
    Namespace,
}

#[derive(Clone, Copy)]
struct RawToken {
    start: u32,
    end: u32,
    token_type: u32,
    token_modifiers_bitset: u32,
}

struct TokenCollector<'db> {
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    tokens: Vec<RawToken>,
}

impl<'db> TokenCollector<'db> {
    fn new(db: &'db dyn hir_ty::Db, file: SourceFile) -> Self {
        Self {
            db,
            file,
            tokens: Vec::new(),
        }
    }

    fn add_span(&mut self, span: Span<'db>, kind: TokenKind, modifiers: u32) {
        let absolute = span.resolve_to_absolute(self.db);
        if absolute.file() != self.file || absolute.is_empty() {
            return;
        }

        self.tokens.push(RawToken {
            start: absolute.start().as_u32(),
            end: absolute.end().as_u32(),
            token_type: token_type_index(kind),
            token_modifiers_bitset: modifiers,
        });
    }

    fn finish(self) -> Vec<RawToken> {
        self.tokens
    }
}

fn collect_declaration_tokens<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    collector: &mut TokenCollector<'db>,
) {
    for item in module.items(db) {
        collect_item_declarations(db, *item, collector);
    }
}

fn collect_item_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    item: Item<'db>,
    collector: &mut TokenCollector<'db>,
) {
    match item {
        Item::FunctionDef(function) => collect_function_declarations(db, function, collector),
        Item::TypeAlias(alias) => {
            collector.add_span(
                alias.name_elem(db).span(db),
                TokenKind::Type,
                declaration_bitset(),
            );
            collect_type_var_declarations(db, alias.ty_param_elems(db), collector);
        }
        Item::AdtDef(adt) => {
            collector.add_span(
                adt.name_elem(db).span(db),
                TokenKind::Type,
                declaration_bitset(),
            );
            collect_type_var_declarations(db, adt.ty_param_elems(db), collector);
            for ctor in adt.ctors(db) {
                collector.add_span(
                    ctor.name.span(db),
                    TokenKind::EnumMember,
                    declaration_bitset(),
                );
            }
        }
        Item::ClassDef(class) => {
            collector.add_span(
                class.head(db).kind(db).class.span(db),
                TokenKind::Type,
                declaration_bitset(),
            );
            collect_type_var_declarations(db, class.type_var_elems(db), collector);
            for method in class.methods(db) {
                collect_signature_declarations(db, method, collector);
            }
        }
        Item::InstanceDef(instance) => {
            collect_type_var_declarations(db, instance.type_var_elems(db), collector);
            for method in instance.methods(db) {
                collect_function_declarations(db, *method, collector);
            }
        }
        Item::ContractDef(contract) => {
            collector.add_span(
                contract.name_elem(db).span(db),
                TokenKind::Type,
                declaration_bitset(),
            );
            collect_type_var_declarations(db, contract.ty_param_elems(db), collector);
            for field in contract.fields(db) {
                collector.add_span(
                    field.name().span(db),
                    TokenKind::Property,
                    declaration_bitset(),
                );
            }
            for item in contract.items(db) {
                collect_contract_item_declarations(db, *item, collector);
            }
        }
        Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
    }
}

fn collect_contract_item_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    item: ContractItem<'db>,
    collector: &mut TokenCollector<'db>,
) {
    match item {
        ContractItem::FunctionDef(function) => {
            collect_function_declarations(db, function, collector);
        }
        ContractItem::TypeAlias(alias) => {
            collector.add_span(
                alias.name_elem(db).span(db),
                TokenKind::Type,
                declaration_bitset(),
            );
            collect_type_var_declarations(db, alias.ty_param_elems(db), collector);
        }
        ContractItem::AdtDef(adt) => {
            collector.add_span(
                adt.name_elem(db).span(db),
                TokenKind::Type,
                declaration_bitset(),
            );
            collect_type_var_declarations(db, adt.ty_param_elems(db), collector);
            for ctor in adt.ctors(db) {
                collector.add_span(
                    ctor.name.span(db),
                    TokenKind::EnumMember,
                    declaration_bitset(),
                );
            }
        }
        ContractItem::Error { .. } => {}
    }
}

fn collect_function_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    collector: &mut TokenCollector<'db>,
) {
    collect_signature_declarations(db, function.sig(db), collector);
}

fn collect_signature_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    sig: &FuncSig<'db>,
    collector: &mut TokenCollector<'db>,
) {
    collector.add_span(sig.name.span(db), TokenKind::Function, declaration_bitset());
    collect_type_var_declarations(db, &sig.type_vars, collector);
    collect_param_declarations(db, sig.params.atom(), collector);
}

fn collect_type_var_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    vars: &[hir::span::SpannedElem<'db, hir::ast::Ident<'db>>],
    collector: &mut TokenCollector<'db>,
) {
    for var in vars {
        collector.add_span(var.span(db), TokenKind::Type, declaration_bitset());
    }
}

fn collect_param_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    params: &[FuncParam<'db>],
    collector: &mut TokenCollector<'db>,
) {
    for param in params {
        match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                collector.add_span(name.span(db), TokenKind::Parameter, declaration_bitset());
            }
            FuncParam::Error { .. } => {}
        }
    }
}

fn collect_item_type_tokens<'db>(
    db: &'db dyn hir_ty::Db,
    facts: &hir_nameres::ItemResolutionFacts<'db>,
    collector: &mut TokenCollector<'db>,
) {
    for resolved in &facts.types {
        collect_type_ref_token(db, resolved.ty, &resolved.resolution, collector);
    }
    for resolved in &facts.preds {
        collect_pred_ref_token(db, resolved.pred, &resolved.resolution, collector);
    }
}

fn collect_body_tokens<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    imports: &dyn hir_nameres::ImportedNames<'db>,
    collector: &mut TokenCollector<'db>,
) {
    for item in module.items(db) {
        collect_item_body_tokens(db, module, *item, None, &[], imports, collector);
    }
}

fn collect_item_body_tokens<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    item: Item<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn hir_nameres::ImportedNames<'db>,
    collector: &mut TokenCollector<'db>,
) {
    match item {
        Item::FunctionDef(function) => collect_function_body_tokens(
            db,
            module,
            function,
            enclosing_contract,
            inherited_type_vars,
            imports,
            collector,
        ),
        Item::InstanceDef(instance) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(hir_nameres::type_var_bindings(
                instance.def_id_value(db),
                instance.type_var_elems(db),
            ));
            for method in instance.methods(db) {
                collect_function_body_tokens(
                    db,
                    module,
                    *method,
                    enclosing_contract,
                    &inherited,
                    imports,
                    collector,
                );
            }
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(hir_nameres::type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                if let ContractItem::FunctionDef(function) = *item {
                    collect_function_body_tokens(
                        db,
                        module,
                        function,
                        Some(contract.def_id_value(db)),
                        &inherited,
                        imports,
                        collector,
                    );
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

fn collect_function_body_tokens<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn hir_nameres::ImportedNames<'db>,
    collector: &mut TokenCollector<'db>,
) {
    let Some(body) = function.body(db) else {
        return;
    };

    let sig = function.sig(db);
    let mut type_vars = inherited_type_vars.to_vec();
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
    let body_map = hir_nameres::resolve_body_with_imports_and_policy(
        db,
        body,
        &context,
        imports,
        hir_nameres::NameresDiagnosticPolicy::Emit,
    );

    collect_body_map_tokens(db, &body_map, collector);
    collect_lambda_param_declarations(db, body, collector);
}

fn collect_body_map_tokens<'db>(
    db: &'db dyn hir_ty::Db,
    body_map: &hir_nameres::BodyResolutionMap<'db>,
    collector: &mut TokenCollector<'db>,
) {
    for entry in &body_map.exprs {
        let expr = entry.body.exprs(db).get(entry.expr);
        if let Some(span) = expr_token_span(db, expr)
            && let Some(kind) = token_kind_for_resolution(&entry.resolution)
        {
            collector.add_span(span, kind, 0);
        }
    }

    for entry in &body_map.stmt_bindings {
        let stmt = entry.body.stmts(db).get(entry.stmt);
        if let StmtKind::Let { name, .. } = &stmt.kind {
            collector.add_span(name.span(db), TokenKind::Variable, declaration_bitset());
        }
    }

    for entry in &body_map.pats {
        let pat = entry.body.pats(db).get(entry.pat);
        let Some(span) = pat_token_span(db, pat) else {
            continue;
        };
        match (&pat.kind, &entry.resolution) {
            (PatKind::Var(_), Resolution::Local(LocalBinding::Pattern { .. })) => {
                collector.add_span(span, TokenKind::Variable, declaration_bitset());
            }
            (PatKind::Ctor { .. }, Resolution::Local(LocalBinding::Pattern { .. })) => {
                collector.add_span(span, TokenKind::Variable, declaration_bitset());
            }
            _ => {
                if let Some(kind) = token_kind_for_resolution(&entry.resolution) {
                    collector.add_span(span, kind, 0);
                }
            }
        }
    }

    for entry in &body_map.types {
        collect_type_ref_token(db, entry.ty, &entry.resolution, collector);
    }
    for entry in &body_map.preds {
        collect_pred_ref_token(db, entry.pred, &entry.resolution, collector);
    }
}

fn collect_lambda_param_declarations<'db>(
    db: &'db dyn hir_ty::Db,
    body: FuncBody<'db>,
    collector: &mut TokenCollector<'db>,
) {
    for (_, expr) in body.exprs(db).iter() {
        if let ExprKind::Lambda {
            params,
            body: lambda_body,
            ..
        } = &expr.kind
        {
            collect_param_declarations(db, params.atom(), collector);
            collect_lambda_param_declarations(db, *lambda_body, collector);
        }
    }
}

fn collect_type_ref_token<'db>(
    db: &'db dyn hir_ty::Db,
    ty: TypeRef<'db>,
    resolution: &Resolution<'db>,
    collector: &mut TokenCollector<'db>,
) {
    let Some(kind) = token_kind_for_resolution(resolution) else {
        return;
    };
    if matches!(
        kind,
        TokenKind::Type | TokenKind::Namespace | TokenKind::EnumMember
    ) && let TypeRefKind::Named { name, .. } = ty.kind(db)
    {
        collector.add_span(name.span(db), kind, 0);
    }
}

fn collect_pred_ref_token<'db>(
    db: &'db dyn hir_ty::Db,
    pred: PredRef<'db>,
    resolution: &Resolution<'db>,
    collector: &mut TokenCollector<'db>,
) {
    if let Some(kind @ TokenKind::Type) = token_kind_for_resolution(resolution) {
        collector.add_span(pred.kind(db).class.span(db), kind, 0);
    }
}

fn expr_token_span<'db>(
    db: &'db dyn hir_ty::Db,
    expr: &hir::ast::function::Expr<'db>,
) -> Option<Span<'db>> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(name.span(db)),
        ExprKind::DotCtor { name, .. } => Some(name.span(db)),
        ExprKind::Field { field, .. } => Some(field.span(db)),
        ExprKind::Lit(_)
        | ExprKind::Proxy { .. }
        | ExprKind::Lambda { .. }
        | ExprKind::BinOp { .. }
        | ExprKind::Index { .. }
        | ExprKind::Call { .. }
        | ExprKind::TypeAnnot { .. }
        | ExprKind::UnaryOp { .. }
        | ExprKind::If { .. }
        | ExprKind::Tuple(_)
        | ExprKind::Error => None,
    }
}

fn pat_token_span<'db>(
    db: &'db dyn hir_ty::Db,
    pat: &hir::ast::function::Pat<'db>,
) -> Option<Span<'db>> {
    match &pat.kind {
        PatKind::Var(name) => Some(name.span(db)),
        PatKind::Ctor { head, .. } => Some(head.name().span(db)),
        PatKind::Wildcard
        | PatKind::Lit(_)
        | PatKind::ComptimeLabel { .. }
        | PatKind::Tuple { .. }
        | PatKind::Error => None,
    }
}

fn token_kind_for_resolution(resolution: &Resolution<'_>) -> Option<TokenKind> {
    match resolution {
        Resolution::Def { kind, .. } => match kind {
            DefResolutionKind::Function => Some(TokenKind::Function),
            DefResolutionKind::Contract
            | DefResolutionKind::Adt
            | DefResolutionKind::TypeAlias
            | DefResolutionKind::Class
            | DefResolutionKind::Instance => Some(TokenKind::Type),
        },
        Resolution::Local(LocalBinding::Let { .. } | LocalBinding::Pattern { .. }) => {
            Some(TokenKind::Variable)
        }
        Resolution::Local(LocalBinding::TypeVar(_)) => Some(TokenKind::Type),
        Resolution::Param(_) => Some(TokenKind::Parameter),
        Resolution::Field(_) => Some(TokenKind::Property),
        Resolution::Ctor { .. } | Resolution::DotCtorDeferred => Some(TokenKind::EnumMember),
        Resolution::ClassMethod { .. } => Some(TokenKind::Function),
        Resolution::Module(_) => Some(TokenKind::Namespace),
        Resolution::Builtin(kind) => match kind {
            BuiltinKind::Type(_) | BuiltinKind::Class(_) => Some(TokenKind::Type),
            BuiltinKind::Constructor(_) => Some(TokenKind::EnumMember),
            BuiltinKind::Function(_) | BuiltinKind::ClassMethod(_) => Some(TokenKind::Function),
        },
        Resolution::Err => None,
    }
}

#[derive(Clone, Copy)]
struct PositionedToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    token_modifiers_bitset: u32,
}

fn encode_tokens(
    line_index: &crate::LineIndexExt,
    raw_tokens: Vec<RawToken>,
) -> Vec<SemanticToken> {
    let mut positioned = raw_tokens
        .into_iter()
        .filter_map(|token| {
            let start = line_index.byte_to_position(token.start);
            let end = line_index.byte_to_position(token.end);
            if start.line != end.line || start.character >= end.character {
                return None;
            }
            Some(PositionedToken {
                line: start.line,
                start: start.character,
                length: end.character - start.character,
                token_type: token.token_type,
                token_modifiers_bitset: token.token_modifiers_bitset,
            })
        })
        .collect::<Vec<_>>();

    positioned.sort_by_key(|token| (token.line, token.start, token.length, token.token_type));

    let mut filtered = Vec::new();
    let mut previous_end = None::<(u32, u32)>;
    for token in positioned {
        let overlaps_previous =
            previous_end.is_some_and(|(line, end)| token.line == line && token.start < end);
        if overlaps_previous {
            continue;
        }
        previous_end = Some((token.line, token.start + token.length));
        filtered.push(token);
    }

    let mut data = Vec::with_capacity(filtered.len());
    let mut previous_line = 0;
    let mut previous_start = 0;
    for token in filtered {
        let delta_line = token.line - previous_line;
        let delta_start = if delta_line == 0 {
            token.start - previous_start
        } else {
            token.start
        };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.token_modifiers_bitset,
        });
        previous_line = token.line;
        previous_start = token.start;
    }

    data
}

fn token_type_index(kind: TokenKind) -> u32 {
    let token_type = match kind {
        TokenKind::Function => &SemanticTokenType::FUNCTION,
        TokenKind::Type => &SemanticTokenType::TYPE,
        TokenKind::Variable => &SemanticTokenType::VARIABLE,
        TokenKind::Parameter => &SemanticTokenType::PARAMETER,
        TokenKind::Property => &SemanticTokenType::PROPERTY,
        TokenKind::EnumMember => &SemanticTokenType::ENUM_MEMBER,
        TokenKind::Namespace => &SemanticTokenType::NAMESPACE,
    };

    TOKEN_TYPES
        .iter()
        .position(|candidate| candidate == token_type)
        .expect("semantic token type must be present in TOKEN_TYPES") as u32
}

fn declaration_bitset() -> u32 {
    modifier_bitset(&SemanticTokenModifier::DECLARATION)
}

fn modifier_bitset(modifier: &SemanticTokenModifier) -> u32 {
    let index = TOKEN_MODIFIERS
        .iter()
        .position(|candidate| candidate == modifier)
        .expect("semantic token modifier must be present in TOKEN_MODIFIERS");
    1_u32 << index
}

#[cfg(test)]
mod tests {
    use lsp_types::{SemanticToken, SemanticTokensResult};

    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn semantic_tokens_are_non_empty_ordered_and_start_at_first_named_entity() {
        let source = "function main(x: word) -> word {\n  let y = x;\n  return y;\n}\n";
        let (world, uri) = world_with_main(source);

        let result = handle_semantic_tokens_full(&world, &uri).expect("semantic tokens");
        let SemanticTokensResult::Tokens(tokens) = result else {
            panic!("expected full semantic tokens");
        };
        assert!(!tokens.data.is_empty(), "expected at least one token");

        let first = tokens.data[0];
        assert_eq!(first.delta_line, 0);
        assert_eq!(first.delta_start, source.find("main").expect("main") as u32);
        assert_eq!(first.length, "main".len() as u32);
        assert_eq!(first.token_type, token_type_index(TokenKind::Function));
        assert_eq!(first.token_modifiers_bitset, declaration_bitset());

        assert_strictly_ordered(&tokens.data);
    }

    #[test]
    fn emitted_token_type_indexes_are_covered_by_the_legend() {
        let source = "\
data Maybe = None | Some(word);

contract Box {
  value: word;
  function get(x: word) -> word {
    let current = value;
    return current + x;
  }
}
";
        let (world, uri) = world_with_main(source);

        let result = handle_semantic_tokens_full(&world, &uri).expect("semantic tokens");
        let SemanticTokensResult::Tokens(tokens) = result else {
            panic!("expected full semantic tokens");
        };
        assert!(
            tokens
                .data
                .iter()
                .all(|token| token.token_type < TOKEN_TYPES.len() as u32),
            "emitted token type outside legend: {:?}",
            tokens.data
        );
        assert!(
            tokens
                .data
                .iter()
                .all(|token| token.token_modifiers_bitset < (1_u32 << TOKEN_MODIFIERS.len())),
            "emitted token modifier outside legend: {:?}",
            tokens.data
        );
    }

    fn assert_strictly_ordered(tokens: &[SemanticToken]) {
        let mut line = 0;
        let mut start = 0;
        let mut previous = None::<(u32, u32)>;
        for token in tokens {
            line += token.delta_line;
            if token.delta_line == 0 {
                start += token.delta_start;
            } else {
                start = token.delta_start;
            }
            if let Some((previous_line, previous_end)) = previous {
                assert!(
                    line > previous_line || (line == previous_line && start >= previous_end),
                    "tokens are not strictly ordered: {tokens:?}"
                );
            }
            previous = Some((line, start + token.length));
        }
    }
}
