//! HIR inspection helpers.
//!
//! This module currently exposes an error-node collector used by tests and
//! callers that need to distinguish parser recovery from later semantic errors.
//! It follows a silent-`Error` contract: recovered HIR nodes are collected as
//! data, not reported as diagnostics here. The parser/lowerer is responsible
//! for emitting parse diagnostics exactly once.

use rustc_hash::FxHashSet;

use crate::{
    Db,
    ast::{
        function::{
            BinOp, Expr, ExprKind, FuncBody, FuncParam, FuncSig, LitKind, Pat, PatKind, Stmt,
            StmtKind, UnOp, YulCase, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
        },
        item::{ContractItem, FunctionDef, Item, Module},
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    span::{Span, Spanned},
};

/// Recovered error placeholder found in lowered HIR.
///
/// The `kind` names the enum variant that carried the placeholder, and `span`
/// is the anchor-relative source range of the recovered syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorNode<'db> {
    /// Static enum-variant name for the recovered node.
    pub kind: &'static str,
    /// Anchor-relative range associated with the recovery node.
    pub span: Span<'db>,
}

/// Collects recovered `Error` nodes from a module without emitting diagnostics.
///
/// This is intentionally a read-only inspection pass. It recurses through item
/// signatures, function bodies, nested lambda bodies, type references, and Yul
/// blocks, but it does not interpret names or types.
pub fn collect_error_nodes<'db>(db: &'db dyn Db, module: Module<'db>) -> Vec<ErrorNode<'db>> {
    let mut collector = ErrorCollector {
        db,
        errors: Vec::new(),
        seen_types: FxHashSet::default(),
    };
    for item in module.items(db) {
        collector.item(*item);
    }
    collector.errors
}

struct ErrorCollector<'db> {
    db: &'db dyn Db,
    errors: Vec<ErrorNode<'db>>,
    seen_types: FxHashSet<TypeRef<'db>>,
}

impl<'db> ErrorCollector<'db> {
    fn push(&mut self, kind: &'static str, span: Span<'db>) {
        self.errors.push(ErrorNode { kind, span });
    }

    fn item(&mut self, item: Item<'db>) {
        match item {
            Item::FunctionDef(def) => self.function(def),
            Item::TypeAlias(def) => self.ty(def.ty(self.db)),
            Item::AdtDef(def) => {
                for ctor in def.ctors(self.db) {
                    self.ty(*ctor.fields.atom());
                }
            }
            Item::ClassDef(def) => {
                for pred in def.super_preds(self.db) {
                    self.pred(*pred);
                }
                self.pred(def.head(self.db));
                for method in def.methods(self.db) {
                    self.sig(method);
                }
            }
            Item::InstanceDef(def) => {
                for pred in def.preds(self.db) {
                    self.pred(*pred);
                }
                self.pred(def.head(self.db));
                for method in def.methods(self.db) {
                    self.function(*method);
                }
            }
            Item::ContractDef(def) => {
                for field in def.fields(self.db) {
                    self.ty(field.ty());
                    if let Some(init) = field.init() {
                        self.field_init(init);
                    }
                }
                for item in def.items(self.db) {
                    self.contract_item(*item);
                }
            }
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) => {}
            Item::Error { span, .. } => self.push("Item::Error", span),
        }
    }

    fn contract_item(&mut self, item: ContractItem<'db>) {
        match item {
            ContractItem::FunctionDef(def) => self.function(def),
            ContractItem::TypeAlias(def) => self.ty(def.ty(self.db)),
            ContractItem::AdtDef(def) => {
                for ctor in def.ctors(self.db) {
                    self.ty(*ctor.fields.atom());
                }
            }
            ContractItem::Error { span, .. } => self.push("ContractItem::Error", span),
        }
    }

    fn function(&mut self, def: FunctionDef<'db>) {
        self.sig(def.sig(self.db));
        if let Some(body) = def.body(self.db) {
            self.body(body);
        }
    }

    fn sig(&mut self, sig: &FuncSig<'db>) {
        for pred in &sig.preds {
            self.pred(*pred);
        }
        for param in sig.params.atom() {
            self.param(param);
        }
        if let Some(ret) = sig.ret {
            self.ty(ret);
        }
    }

    fn param(&mut self, param: &FuncParam<'db>) {
        match param {
            FuncParam::Typed { ty, .. } => self.ty(*ty),
            FuncParam::Untyped { .. } => {}
            FuncParam::Error { span } => self.push("FuncParam::Error", *span),
        }
    }

    fn pred(&mut self, pred: PredRef<'db>) {
        let kind = pred.kind(self.db);
        self.ty(kind.ty);
        for arg in kind.args.atom() {
            self.ty(*arg);
        }
    }

    fn ty(&mut self, ty: TypeRef<'db>) {
        if !self.seen_types.insert(ty) {
            return;
        }
        match ty.kind(self.db) {
            TypeRefKind::Named { args, .. } | TypeRefKind::Tuple { elems: args } => {
                for arg in args.atom() {
                    self.ty(*arg);
                }
            }
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.ty(*param);
                }
                self.ty(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.ty(*inner),
            TypeRefKind::Error { span } => self.push("TypeRefKind::Error", *span),
        }
    }

    fn body(&mut self, body: FuncBody<'db>) {
        for (_, stmt) in body.stmts(self.db).iter() {
            self.stmt(stmt);
        }
        for (_, expr) in body.exprs(self.db).iter() {
            self.expr(expr);
        }
        for (_, pat) in body.pats(self.db).iter() {
            self.pat(pat);
        }
    }

    fn field_init(&mut self, init: &crate::ast::item::FieldInit<'db>) {
        for (_, expr) in init.exprs.iter() {
            self.expr(expr);
        }
    }

    fn stmt(&mut self, stmt: &Stmt<'db>) {
        match &stmt.kind {
            StmtKind::Let { ty: Some(ty), .. } => self.ty(*ty),
            StmtKind::Assembly { body } => {
                for stmt in body {
                    self.yul_stmt(stmt);
                }
            }
            StmtKind::Error => self.push("StmtKind::Error", stmt.span),
            _ => {}
        }
    }

    fn expr(&mut self, expr: &Expr<'db>) {
        match &expr.kind {
            ExprKind::Lit(LitKind::Error) => self.push("LitKind::Error", expr.span),
            ExprKind::Proxy { ty, .. } | ExprKind::Conversion { ty, .. } => self.ty(*ty),
            ExprKind::Lambda { params, ret, body } => {
                for param in params.atom() {
                    self.param(param);
                }
                if let Some(ret) = ret {
                    self.ty(*ret);
                }
                self.body(*body);
            }
            ExprKind::BinOp { op, .. } if *op.atom() == BinOp::Error => {
                self.push("BinOp::Error", op.span(self.db));
            }
            ExprKind::UnaryOp { op, .. } if *op.atom() == UnOp::Error => {
                self.push("UnOp::Error", op.span(self.db));
            }
            ExprKind::Error => self.push("ExprKind::Error", expr.span),
            _ => {}
        }
    }

    fn pat(&mut self, pat: &Pat<'db>) {
        match &pat.kind {
            PatKind::Lit(LitKind::Error) => self.push("LitKind::Error", pat.span),
            PatKind::Error => self.push("PatKind::Error", pat.span),
            _ => {}
        }
    }

    fn yul_stmt(&mut self, stmt: &YulStmt<'db>) {
        match &stmt.kind {
            YulStmtKind::Block(body) | YulStmtKind::FunctionDef { body, .. } => {
                self.yul_stmts(body);
            }
            YulStmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    self.yul_expr(init);
                }
            }
            YulStmtKind::Assign { value, .. } | YulStmtKind::Expr(value) => self.yul_expr(value),
            YulStmtKind::If { cond, body } => {
                self.yul_expr(cond);
                self.yul_stmts(body);
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                self.yul_stmts(init);
                self.yul_expr(cond);
                self.yul_stmts(post);
                self.yul_stmts(body);
            }
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                self.yul_expr(expr);
                for case in cases {
                    self.yul_case(case);
                }
                if let Some(default) = default {
                    self.yul_stmts(default);
                }
            }
            YulStmtKind::Error => self.push("YulStmtKind::Error", stmt.span),
            YulStmtKind::Leave | YulStmtKind::Break | YulStmtKind::Continue => {}
        }
    }

    fn yul_stmts(&mut self, stmts: &[YulStmt<'db>]) {
        for stmt in stmts {
            self.yul_stmt(stmt);
        }
    }

    fn yul_case(&mut self, case: &YulCase<'db>) {
        if matches!(case.lit, YulLitKind::Error) {
            self.push("YulLitKind::Error", case.span);
        }
        self.yul_stmts(&case.body);
    }

    fn yul_expr(&mut self, expr: &YulExpr<'db>) {
        match &expr.kind {
            YulExprKind::Lit(YulLitKind::Error) => self.push("YulLitKind::Error", expr.span),
            YulExprKind::Call { args, .. } => {
                for arg in args {
                    self.yul_expr(arg);
                }
            }
            YulExprKind::Error => self.push("YulExprKind::Error", expr.span),
            _ => {}
        }
    }
}
