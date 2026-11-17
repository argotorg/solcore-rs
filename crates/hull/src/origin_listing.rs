//! Listing of what each Hull function and statement will be attributed to.
//!
//! One node per line, two spaces of indent per depth:
//!
//! ```text
//! fn <name> <origin>
//!   S <kind> <origin>
//!     E <kind> <start>-<end> <path>
//! # stmts=<n> source=<n> generated=<n> none=<n> escaping_literals=<n>
//! ```
//!
//! `<origin>` is `src <start>-<end> <path>`, `gen <reason> <start>-<end> <path>`
//! or `none` for a zero-width span. `<path>` is the last two segments of the
//! file URL. Object-level code is listed under an `object <name>` line.
//! Expressions show their range only, plus the text of word and bool literals.
//!
//! `escaping_literals` counts word and bool literal expressions whose range is
//! not inside the range of their enclosing statement in the same file. Inline
//! Yul statements and expressions are not listed.

use std::fmt::Write as _;

use hir::{Db as HirDb, diag::AbsoluteSpan};

use crate::{
    ir::{Alt, CodeBlock, Expr, ExprKind, Function, Object, Program, Stmt, StmtKind},
    origin::{ResolvedOrigin, function_origin, stmt_origin},
};

/// Renders the origin listing of `program`, ending with a summary line.
pub fn origin_listing<'db>(db: &'db dyn HirDb, program: &Program<'db>) -> String {
    let mut listing = Listing {
        db,
        out: String::new(),
        counts: Counts::default(),
    };
    for function in &program.functions {
        listing.function(function, 0);
    }
    for object in &program.objects {
        listing.object(object, 0);
    }
    let Counts {
        stmts,
        source,
        generated,
        none,
        escaping_literals,
    } = listing.counts;
    let _ = writeln!(
        listing.out,
        "# stmts={stmts} source={source} generated={generated} none={none} escaping_literals={escaping_literals}"
    );
    listing.out
}

#[derive(Default)]
struct Counts {
    stmts: usize,
    source: usize,
    generated: usize,
    none: usize,
    escaping_literals: usize,
}

struct Listing<'db> {
    db: &'db dyn HirDb,
    out: String,
    counts: Counts,
}

impl<'db> Listing<'db> {
    fn object(&mut self, object: &Object<'db>, depth: usize) {
        let _ = writeln!(self.out, "{}object {}", indent(depth), object.name);
        self.code_block(&object.code, depth + 1);
        for inner in &object.inners {
            self.object(inner, depth + 1);
        }
    }

    fn code_block(&mut self, code: &CodeBlock<'db>, depth: usize) {
        for function in &code.functions {
            self.function(function, depth);
        }
        self.stmts(None, &code.stmts, depth);
    }

    fn function(&mut self, function: &Function<'db>, depth: usize) {
        let origin = function_origin(function).resolve(self.db);
        let _ = writeln!(
            self.out,
            "{}fn {} {}",
            indent(depth),
            function.name,
            origin_text(origin.as_ref())
        );
        self.stmts(Some(function), &function.body, depth + 1);
    }

    fn stmts(&mut self, function: Option<&Function<'db>>, stmts: &[Stmt<'db>], depth: usize) {
        for stmt in stmts {
            self.stmt(function, stmt, depth);
        }
    }

    fn stmt(&mut self, function: Option<&Function<'db>>, stmt: &Stmt<'db>, depth: usize) {
        let kind = match &stmt.kind {
            StmtKind::Let { name, .. } => format!("let {name}"),
            StmtKind::Assign { .. } => "assign".to_owned(),
            StmtKind::Expr(_) => "expr-stmt".to_owned(),
            StmtKind::Return(_) => "return".to_owned(),
            StmtKind::Block(_) => "block".to_owned(),
            StmtKind::For { .. } => "for".to_owned(),
            StmtKind::Break => "break".to_owned(),
            StmtKind::Continue => "continue".to_owned(),
            StmtKind::Match { .. } => "match".to_owned(),
            StmtKind::Assembly(_) => "assembly".to_owned(),
            StmtKind::Revert(_) => "revert".to_owned(),
            StmtKind::Comment(_) => "comment".to_owned(),
        };
        let origin = stmt_origin(function, stmt.span).resolve(self.db);
        self.counts.stmts += 1;
        match &origin {
            Some(ResolvedOrigin {
                generated: None, ..
            }) => self.counts.source += 1,
            Some(_) => self.counts.generated += 1,
            None => self.counts.none += 1,
        }
        let _ = writeln!(
            self.out,
            "{}S {kind} {}",
            indent(depth),
            origin_text(origin.as_ref())
        );

        let within = stmt.span.resolve_to_absolute(self.db);
        let inner = depth + 1;
        match &stmt.kind {
            StmtKind::Assign { lhs, rhs } => {
                self.expr(lhs, within, inner);
                self.expr(rhs, within, inner);
            }
            StmtKind::Expr(expr) | StmtKind::Return(expr) => self.expr(expr, within, inner),
            StmtKind::Block(body) => self.stmts(function, body, inner),
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                self.stmts(function, init, inner);
                self.expr(cond, within, inner);
                self.stmts(function, post, inner);
                self.stmts(function, body, inner);
            }
            StmtKind::Match {
                scrutinee, alts, ..
            } => {
                self.expr(scrutinee, within, inner);
                for Alt { body, .. } in alts {
                    self.stmts(function, body, inner);
                }
            }
            StmtKind::Let { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Assembly(_)
            | StmtKind::Revert(_)
            | StmtKind::Comment(_) => {}
        }
    }

    fn expr(&mut self, expr: &Expr<'db>, within: AbsoluteSpan, depth: usize) {
        let absolute = expr.span.resolve_to_absolute(self.db);
        let (kind, literal) = match &expr.kind {
            ExprKind::Word(text) => ("word", Some(text.clone())),
            ExprKind::Bool(value) => ("bool", Some(value.to_string())),
            ExprKind::Unit => ("unit", None),
            ExprKind::Var(_) => ("var", None),
            ExprKind::Pair(..) => ("pair", None),
            ExprKind::Fst(_) => ("fst", None),
            ExprKind::Snd(_) => ("snd", None),
            ExprKind::Inl { .. } => ("inl", None),
            ExprKind::Inr { .. } => ("inr", None),
            ExprKind::InK { .. } => ("ink", None),
            ExprKind::Call { .. } => ("call", None),
            ExprKind::If { .. } => ("if", None),
        };
        if literal.is_some()
            && !(absolute.file == within.file
                && within.start <= absolute.start
                && absolute.end <= within.end)
        {
            self.counts.escaping_literals += 1;
        }
        let literal = literal.map(|text| format!("{text} ")).unwrap_or_default();
        let url = absolute.file.url(self.db);
        let _ = writeln!(
            self.out,
            "{}E {kind} {literal}{}-{} {}",
            indent(depth),
            absolute.start.as_u32(),
            absolute.end.as_u32(),
            short_path(url.as_str())
        );

        let inner = depth + 1;
        match &expr.kind {
            ExprKind::Pair(lhs, rhs) => {
                self.expr(lhs, within, inner);
                self.expr(rhs, within, inner);
            }
            ExprKind::Fst(value) | ExprKind::Snd(value) => self.expr(value, within, inner),
            ExprKind::Inl { value, .. }
            | ExprKind::Inr { value, .. }
            | ExprKind::InK { value, .. } => self.expr(value, within, inner),
            ExprKind::Call { args, .. } => {
                for arg in args {
                    self.expr(arg, within, inner);
                }
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.expr(cond, within, inner);
                self.expr(then_expr, within, inner);
                self.expr(else_expr, within, inner);
            }
            ExprKind::Word(_) | ExprKind::Bool(_) | ExprKind::Unit | ExprKind::Var(_) => {}
        }
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

/// `src <start>-<end> <path>`, `gen <reason> <start>-<end> <path>` or `none`.
fn origin_text(origin: Option<&ResolvedOrigin>) -> String {
    match origin {
        None => "none".to_owned(),
        Some(origin) => {
            let range = format!(
                "{}-{} {}",
                origin.start,
                origin.end,
                short_path(&origin.url)
            );
            match origin.generated {
                None => format!("src {range}"),
                Some(reason) => format!("gen {} {range}", reason.as_str()),
            }
        }
    }
}

/// The last two path segments of a file URL, for example `std/dispatch.sol`.
fn short_path(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let segments = without_scheme
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    segments[segments.len().saturating_sub(2)..].join("/")
}

#[cfg(test)]
mod tests {
    use super::short_path;

    #[test]
    fn short_path_keeps_the_last_two_segments() {
        assert_eq!(
            short_path("file:///lab/argot/std/dispatch.sol"),
            "std/dispatch.sol"
        );
        assert_eq!(short_path("memory:///main.sol"), "main.sol");
        assert_eq!(short_path("file:///a/b/c/d.sol"), "c/d.sol");
    }
}
