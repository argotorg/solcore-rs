//! Syntax facts used to lay out the original token stream without reprinting
//! the parsed tree, which already contains several surface desugarings.

use std::{collections::BTreeSet, ops::Range};

use hir::ast::item::FuncKind;
use logos::Logos;

use crate::{
    lexer::Token,
    parse::{parse_body_statements, parse_supported_items},
    types::*,
};

#[derive(Default)]
pub(super) struct Layout {
    /// Token offsets that begin declarations, statements, or enum variants.
    pub line_starts: BTreeSet<usize>,
    /// Both opening and closing angle brackets belonging to type syntax.
    pub generic_angles: BTreeSet<usize>,
    /// Conditional-expression colons, as distinct from type annotations.
    pub ternary_colons: BTreeSet<usize>,
    /// Leading constructor dots, as distinct from field/path separators.
    pub leading_dots: BTreeSet<usize>,
    /// Both braces of blocks; import and export selectors are excluded.
    pub block_braces: BTreeSet<usize>,
    /// Hyphens joining the segments of a pragma name, rather than subtraction.
    pub pragma_hyphens: BTreeSet<usize>,
}

pub(super) fn layout(source: &str) -> Result<Layout, Vec<(Range<usize>, String)>> {
    let tokens = Token::lexer(source)
        .spanned()
        .filter_map(|(token, span)| token.ok().map(|token| (token, span)))
        .collect::<Vec<_>>();
    let parsed = parse_supported_items(source);
    let mut collector = Collector {
        source,
        tokens,
        layout: Layout::default(),
        errors: Vec::new(),
        pending: parsed.output.into_iter().map(Node::Top).collect(),
    };
    collector.errors(parsed.errors);
    for (token, span) in &collector.tokens {
        match token {
            Token::LBrace | Token::RBrace => {
                collector.layout.block_braces.insert(span.start);
            }
            Token::Case | Token::Default => {
                collector.layout.line_starts.insert(span.start);
            }
            _ => {}
        }
    }
    // Consume the tree iteratively: binary expressions and postfix chains are
    // not bounded by delimiter nesting and may be too deep to walk or drop
    // recursively on an editor worker's stack.
    while let Some(node) = collector.pending.pop() {
        match node {
            Node::Top(item) => collector.top(item),
            Node::Contract(item) => collector.contract_item(item),
            Node::Ty(ty) => collector.ty(ty),
            Node::Expr(expr) => collector.expr(expr),
            Node::Pat(pat) => collector.pat(pat),
            Node::Stmt(stmt, starts_line) => collector.stmt(stmt, starts_line),
            Node::YulStmt(stmt) => collector.yul_stmt(stmt),
            Node::YulExpr(expr) => collector.yul_expr(expr),
            Node::Body(span, implicit_return) => collector.body(span, implicit_return),
        }
    }
    if collector.errors.is_empty() {
        Ok(collector.layout)
    } else {
        collector.errors.sort_by_key(|(range, _)| range.start);
        collector.errors.dedup();
        Err(collector.errors)
    }
}

enum Node<'src> {
    Top(ParsedTopItem<'src>),
    Contract(ParsedContractItem<'src>),
    Ty(ParsedTy<'src>),
    Expr(ParsedExpr<'src>),
    Pat(ParsedPat<'src>),
    Stmt(ParsedStmt<'src>, bool),
    YulStmt(ParsedYulStmt<'src>),
    YulExpr(ParsedYulExpr<'src>),
    Body(LexSpan, bool),
}

struct Collector<'src> {
    source: &'src str,
    tokens: Vec<(Token<'src>, Range<usize>)>,
    layout: Layout,
    errors: Vec<(Range<usize>, String)>,
    pending: Vec<Node<'src>>,
}

impl<'src> Collector<'src> {
    fn errors(&mut self, errors: Vec<ParsedError>) {
        self.errors.extend(
            errors
                .into_iter()
                .map(|error| (error.span.start..error.span.end, error.message)),
        );
    }

    fn recovered(&mut self, span: LexSpan) {
        self.errors.push((
            span.start..span.end,
            "cannot format invalid syntax".to_owned(),
        ));
    }

    fn angles(&mut self, span: LexSpan) {
        let start = self
            .tokens
            .partition_point(|(_, range)| range.end <= span.start);
        for (token, range) in &self.tokens[start..] {
            if range.start >= span.end {
                break;
            }
            if matches!(token, Token::Less | Token::Greater) {
                self.layout.generic_angles.insert(range.start);
            }
        }
    }

    fn header_angles(&mut self, span: LexSpan) {
        let start = self
            .tokens
            .partition_point(|(_, range)| range.end <= span.start);
        let end = self.tokens[start..]
            .iter()
            .find(|(token, range)| range.start < span.end && matches!(token, Token::LBrace))
            .map_or(span.end, |(_, range)| range.start);
        self.angles((span.start..end).into());
    }

    fn start(&mut self, span: LexSpan) {
        self.layout.line_starts.insert(span.start);
    }

    fn top(&mut self, item: ParsedTopItem<'src>) {
        match item {
            ParsedTopItem::Import { span, .. } | ParsedTopItem::Export { span, .. } => {
                self.start(span);
                self.layout
                    .block_braces
                    .retain(|offset| *offset < span.start || *offset >= span.end);
            }
            ParsedTopItem::Pragma { span, name, .. } => {
                self.start(span);
                let name_span = name.1;
                // Keyword-shaped segments such as `default` are part of the
                // pragma name, not statement or match-arm boundaries.
                self.layout
                    .line_starts
                    .retain(|offset| *offset < name_span.start || *offset >= name_span.end);
                let start = self
                    .tokens
                    .partition_point(|(_, range)| range.end <= name_span.start);
                for (token, range) in &self.tokens[start..] {
                    if range.start >= name_span.end {
                        break;
                    }
                    if matches!(token, Token::Minus) {
                        self.layout.pragma_hyphens.insert(range.start);
                    }
                }
            }
            ParsedTopItem::TypeAlias { span, ty, .. } => {
                self.start(span);
                self.angles(span);
                self.pending.push(Node::Ty(ty));
            }
            ParsedTopItem::Adt {
                span,
                derive_attr,
                ctors,
                ..
            } => self.adt(span, derive_attr, ctors),
            ParsedTopItem::Class {
                span,
                super_preds,
                head,
                methods,
                ..
            } => {
                self.start(span);
                self.header_angles(span);
                self.pred(head);
                for pred in super_preds {
                    self.pred(pred);
                }
                for method in methods {
                    self.start(method.sig.span);
                    self.sig(method.sig);
                }
            }
            ParsedTopItem::Instance {
                span,
                preds,
                head,
                methods,
                ..
            } => {
                self.start(span);
                self.header_angles(span);
                self.pred(head);
                for pred in preds {
                    self.pred(pred);
                }
                for method in methods {
                    self.function(method);
                }
            }
            ParsedTopItem::Contract {
                span,
                fields,
                items,
                ..
            } => {
                self.start(span);
                self.header_angles(span);
                for field in fields {
                    self.start(field.span);
                    self.pending.push(Node::Ty(field.ty));
                    self.pending.extend(field.init.map(Node::Expr));
                }
                self.pending.extend(items.into_iter().map(Node::Contract));
            }
            ParsedTopItem::Function {
                span,
                sig,
                body_span,
                ..
            } => {
                self.start(span);
                self.sig(sig);
                self.pending.push(Node::Body(body_span, true));
            }
            ParsedTopItem::Error { span, .. } => self.recovered(span),
        }
    }

    fn contract_item(&mut self, item: ParsedContractItem<'src>) {
        match item {
            ParsedContractItem::Function(function) => self.function(function),
            ParsedContractItem::TypeAlias { span, ty, .. } => {
                self.start(span);
                self.angles(span);
                self.pending.push(Node::Ty(ty));
            }
            ParsedContractItem::Adt {
                span,
                derive_attr,
                ctors,
                ..
            } => self.adt(span, derive_attr, ctors),
            ParsedContractItem::Error { span, .. } => self.recovered(span),
        }
    }

    fn adt(
        &mut self,
        span: LexSpan,
        derive_attr: Option<ParsedDeriveAttr<'src>>,
        ctors: Vec<ParsedAdtCtor<'src>>,
    ) {
        self.start(span);
        self.angles(span);
        if let Some(attribute) = derive_attr
            && let Some((_, range)) = self
                .tokens
                .iter()
                .find(|(_, range)| range.start >= attribute.span.end)
        {
            self.layout.line_starts.insert(range.start);
        }
        for ctor in ctors {
            if let Some(fields) = ctor.field_names {
                for (_, span) in fields {
                    self.start(span);
                }
            } else {
                self.start(ctor.span);
            }
            self.pending.extend(ctor.fields.into_iter().map(Node::Ty));
        }
    }

    fn function(&mut self, function: ParsedFunctionDef<'src>) {
        self.start(function.span);
        self.sig(function.sig);
        self.pending.push(Node::Body(
            function.body_span,
            function.kind == FuncKind::Function,
        ));
    }

    fn sig(&mut self, sig: ParsedFuncSig<'src>) {
        self.angles(sig.span);
        self.params(sig.params);
        self.pending.extend(sig.ret.map(Node::Ty));
        for pred in sig.preds {
            self.pred(pred);
        }
    }

    fn params(&mut self, params: Vec<ParsedFuncParam<'src>>) {
        for param in params {
            match param {
                ParsedFuncParam::Typed { ty, .. } => self.pending.push(Node::Ty(ty)),
                ParsedFuncParam::Untyped { .. } => {}
                ParsedFuncParam::Error { span } => self.recovered(span),
            }
        }
    }

    fn pred(&mut self, pred: ParsedPred<'src>) {
        if let Some(span) = pred.args_span {
            self.angles(span);
        }
        self.pending.push(Node::Ty(pred.ty));
        self.pending.extend(pred.args.into_iter().map(Node::Ty));
    }

    fn ty(&mut self, ty: ParsedTy<'src>) {
        self.angles(ty.span);
        match ty.kind {
            ParsedTyKind::Named { args, .. } => {
                self.pending.extend(args.into_iter().map(Node::Ty));
            }
            ParsedTyKind::Proxy { inner, .. }
            | ParsedTyKind::Comptime { inner, .. }
            | ParsedTyKind::Array { elem: inner, .. } => {
                self.pending.push(Node::Ty(*inner));
            }
            ParsedTyKind::Fn { params, ret, .. } => {
                self.pending.extend(params.into_iter().map(Node::Ty));
                self.pending.push(Node::Ty(*ret));
            }
            ParsedTyKind::Tuple { elems } => {
                self.pending.extend(elems.into_iter().map(Node::Ty));
            }
            ParsedTyKind::Error => self.recovered(ty.span),
        }
    }

    fn body(&mut self, span: LexSpan, implicit_return: bool) {
        let parsed = parse_body_statements(self.source, span);
        self.errors(parsed.errors);
        if !implicit_return
            && let Some(ParsedStmt {
                span,
                kind:
                    ParsedStmtKind::Expr {
                        trailing_semi: false,
                        ..
                    },
            }) = parsed.output.last()
        {
            self.errors.push((
                span.start..span.end,
                "expression statement requires trailing `;`".to_owned(),
            ));
        }
        self.statements(parsed.output, true);
    }

    fn statements(&mut self, statements: Vec<ParsedStmt<'src>>, starts_line: bool) {
        self.pending.extend(
            statements
                .into_iter()
                .map(|statement| Node::Stmt(statement, starts_line)),
        );
    }

    fn stmt(&mut self, stmt: ParsedStmt<'src>, starts_line: bool) {
        if starts_line {
            self.start(stmt.span);
        }
        match stmt.kind {
            ParsedStmtKind::Let { ty, init, .. } => {
                self.pending.extend(ty.map(Node::Ty));
                self.pending.extend(init.map(Node::Expr));
            }
            ParsedStmtKind::Return(expr) => self.pending.extend(expr.map(Node::Expr)),
            ParsedStmtKind::Expr { expr, .. } => self.pending.push(Node::Expr(expr)),
            ParsedStmtKind::Assign { lhs, rhs, .. } => {
                self.pending.push(Node::Expr(lhs));
                self.pending.push(Node::Expr(rhs));
            }
            ParsedStmtKind::Match { scrutinees, arms } => {
                self.pending.extend(scrutinees.into_iter().map(Node::Expr));
                for arm in arms {
                    self.start(arm.span);
                    self.pending.extend(arm.pats.into_iter().map(Node::Pat));
                    self.statements(arm.body, true);
                }
            }
            ParsedStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                self.statements(init, false);
                self.pending.push(Node::Expr(cond));
                self.statements(post.stmts, false);
                self.statements(body.stmts, true);
            }
            ParsedStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.pending.push(Node::Expr(cond));
                self.statements(then_body.stmts, true);
                if let Some(body) = else_body {
                    self.statements(body.stmts, true);
                }
            }
            ParsedStmtKind::Block { body } | ParsedStmtKind::Unchecked { body, .. } => {
                self.statements(body, true)
            }
            ParsedStmtKind::Assembly { body } => {
                self.pending.extend(body.into_iter().map(Node::YulStmt));
            }
            ParsedStmtKind::Break | ParsedStmtKind::Continue => {}
            ParsedStmtKind::Error => self.recovered(stmt.span),
        }
    }

    fn expr(&mut self, expr: ParsedExpr<'src>) {
        match expr.kind {
            ParsedExprKind::Lit(_) | ParsedExprKind::Ident(_) => {}
            ParsedExprKind::DotCtor { dot, args, .. } => {
                self.layout.leading_dots.insert(dot.start);
                self.pending.extend(args.into_iter().map(Node::Expr));
            }
            ParsedExprKind::Tuple(args) | ParsedExprKind::Array(args) => {
                self.pending.extend(args.into_iter().map(Node::Expr));
            }
            ParsedExprKind::Proxy { ty, .. } => self.pending.push(Node::Ty(ty)),
            ParsedExprKind::NewArray { ty, len, .. } => {
                self.pending.push(Node::Ty(ty));
                self.pending.push(Node::Expr(*len));
            }
            ParsedExprKind::Lambda {
                params,
                ret,
                body_span,
                ..
            } => {
                self.params(params);
                self.pending.extend(ret.map(Node::Ty));
                self.pending.push(Node::Body(body_span, false));
            }
            ParsedExprKind::BinOp { lhs, rhs, .. } => {
                self.pending.push(Node::Expr(*lhs));
                self.pending.push(Node::Expr(*rhs));
            }
            ParsedExprKind::Index { base, index } => {
                self.pending.push(Node::Expr(*base));
                self.pending.push(Node::Expr(*index));
            }
            ParsedExprKind::Call { callee, args } => {
                self.pending.push(Node::Expr(*callee));
                self.pending.extend(args.into_iter().map(Node::Expr));
            }
            ParsedExprKind::Field { base, .. } => self.pending.push(Node::Expr(*base)),
            ParsedExprKind::UnaryOp { expr, .. } => self.pending.push(Node::Expr(*expr)),
            ParsedExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let start = self
                    .tokens
                    .partition_point(|(_, range)| range.end <= then_expr.span.end);
                if let Some((_, range)) = self.tokens[start..]
                    .iter()
                    .take_while(|(_, range)| range.start < else_expr.span.start)
                    .find(|(token, _)| matches!(token, Token::Colon))
                {
                    self.layout.ternary_colons.insert(range.start);
                }
                self.pending.push(Node::Expr(*cond));
                self.pending.push(Node::Expr(*then_expr));
                self.pending.push(Node::Expr(*else_expr));
            }
            ParsedExprKind::Error => self.recovered(expr.span),
        }
    }

    fn pat(&mut self, pat: ParsedPat<'src>) {
        match pat.kind {
            ParsedPatKind::Wildcard | ParsedPatKind::Var(_) | ParsedPatKind::Lit(_) => {}
            ParsedPatKind::Ctor {
                leading_dot, args, ..
            } => {
                if let Some(dot) = leading_dot {
                    self.layout.leading_dots.insert(dot.start);
                }
                self.pending.extend(args.into_iter().map(Node::Pat));
            }
            ParsedPatKind::Tuple(args) => {
                self.pending.extend(args.into_iter().map(Node::Pat));
            }
            ParsedPatKind::ComptimeLabel { expr, .. } => self.pending.push(Node::Expr(expr)),
            ParsedPatKind::Error => self.recovered(pat.span),
        }
    }

    fn yul_stmt(&mut self, stmt: ParsedYulStmt<'src>) {
        self.start(stmt.span);
        match stmt.kind {
            ParsedYulStmtKind::Block(body) | ParsedYulStmtKind::FunctionDef { body, .. } => {
                self.pending.extend(body.into_iter().map(Node::YulStmt));
            }
            ParsedYulStmtKind::Let { init, .. } => self.pending.extend(init.map(Node::YulExpr)),
            ParsedYulStmtKind::Assign { value, .. } | ParsedYulStmtKind::Expr(value) => {
                self.pending.push(Node::YulExpr(value));
            }
            ParsedYulStmtKind::If { cond, body } => {
                self.pending.push(Node::YulExpr(cond));
                self.pending.extend(body.into_iter().map(Node::YulStmt));
            }
            ParsedYulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                self.pending.extend(init.into_iter().map(Node::YulStmt));
                self.pending.push(Node::YulExpr(cond));
                self.pending.extend(post.into_iter().map(Node::YulStmt));
                self.pending.extend(body.into_iter().map(Node::YulStmt));
            }
            ParsedYulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                self.pending.push(Node::YulExpr(expr));
                for case in cases {
                    self.start(case.span);
                    self.pending
                        .extend(case.body.into_iter().map(Node::YulStmt));
                }
                if let Some(body) = default {
                    self.pending.extend(body.into_iter().map(Node::YulStmt));
                }
            }
            ParsedYulStmtKind::Leave | ParsedYulStmtKind::Break | ParsedYulStmtKind::Continue => {}
            ParsedYulStmtKind::Error => self.recovered(stmt.span),
        }
    }

    fn yul_expr(&mut self, expr: ParsedYulExpr<'src>) {
        match expr.kind {
            ParsedYulExprKind::Lit(_) | ParsedYulExprKind::Ident(_) => {}
            ParsedYulExprKind::Call { args, .. } => {
                self.pending.extend(args.into_iter().map(Node::YulExpr));
            }
            ParsedYulExprKind::Error => self.recovered(expr.span),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::layout;

    #[test]
    fn separates_yul_statements_but_keeps_core_for_headers_together() {
        let source = "pragma solcore; function f() { for (let i = 0; i < 3; i += 1) { assembly { let x := 0 x := add(x, 1) mstore(0, x) } } }";
        let layout = layout(source).unwrap();
        for text in ["for (", "assembly {", "let x", "x := add", "mstore("] {
            assert!(
                layout.line_starts.contains(&source.find(text).unwrap()),
                "{text}"
            );
        }
        for text in ["let i", "i +="] {
            assert!(
                !layout.line_starts.contains(&source.find(text).unwrap()),
                "{text}"
            );
        }
        assert!(layout.generic_angles.is_empty());
    }

    #[test]
    fn distinguishes_generic_brackets_from_comparisons() {
        let source = "pragma solcore; function f<a>(x: Pair<a, word>) returns (word) { let p: Pair<word, word>; if (x < 2) { return 1; } return 0; }";
        let layout = layout(source).unwrap();
        for (offset, character) in source.char_indices() {
            if matches!(character, '<' | '>') {
                assert_eq!(
                    layout.generic_angles.contains(&offset),
                    offset != source.find("< 2").unwrap(),
                    "angle at {offset}",
                );
            }
        }
    }

    #[test]
    fn excludes_import_and_export_selector_braces() {
        let source = "pragma solcore; import { A, B } from lib; export { A, B }; enum E { A, B }";
        let layout = layout(source).unwrap();
        let enum_start = source.find("enum").unwrap();
        for (offset, character) in source.char_indices() {
            if matches!(character, '{' | '}') {
                assert_eq!(layout.block_braces.contains(&offset), offset > enum_start);
            }
        }
    }

    #[test]
    fn validates_nested_lambda_bodies_and_tail_semicolons() {
        assert!(layout("pragma solcore; function f() { let f = lam() { let x = ; }; }").is_err());
        assert!(layout("pragma solcore; function f() { let f = lam() { 1 }; }").is_err());
        assert!(layout("pragma solcore; contract C { constructor() { 1 } }").is_err());
        assert!(layout("pragma solcore; contract C { fallback() { 1 } }").is_err());
        assert!(layout("pragma solcore; function f() returns (word) { 1 }").is_ok());
    }

    #[test]
    fn marks_nested_and_parenthesized_ternaries_separately_from_types() {
        let source = "pragma solcore; function f(x: word) returns (word) { return true ? (false ? x : 0) : (true ? 1 : x); }";
        let layout = layout(source).unwrap();
        let annotation = source.find(':').unwrap();
        for (offset, character) in source.char_indices() {
            if character == ':' {
                assert_eq!(
                    layout.ternary_colons.contains(&offset),
                    offset != annotation
                );
            }
        }
    }

    #[test]
    fn distinguishes_constructor_dots_from_paths_and_fields() {
        let source = "pragma solcore; function f(x: M.T) { match (x) { case comptime .A { return .B(x.field); } case .C { return M.D; } } }";
        let layout = layout(source).unwrap();
        for (offset, character) in source.char_indices() {
            if character == '.' {
                assert_eq!(
                    layout.leading_dots.contains(&offset),
                    [".A", ".B", ".C"]
                        .iter()
                        .any(|text| source[offset..].starts_with(text)),
                );
            }
        }
    }

    #[test]
    fn flat_expression_chains_respect_the_shared_parser_depth_limit() {
        // Walking the formatter tree is iterative, but parsing can clone or
        // discard a partial tree first. Keep those trees bounded on every
        // consumer, including formatting, even when the input is well formed.
        for (depth, accepted) in [(127, true), (128, false), (4096, false)] {
            let source = format!(
                "pragma solcore; function f() {{ return {}0; }}",
                "0 + ".repeat(depth)
            );
            assert_eq!(layout(&source).is_ok(), accepted);
            let source = format!(
                "pragma solcore; function f() {{ return x{}; }}",
                ".field".repeat(depth)
            );
            assert_eq!(layout(&source).is_ok(), accepted);
        }
    }

    #[test]
    fn rejects_excessive_delimiter_nesting() {
        let source = format!(
            "pragma solcore; function f() {{ return {}0{}; }}",
            "(".repeat(129),
            ")".repeat(129),
        );
        assert!(layout(&source).is_err());
    }
}
