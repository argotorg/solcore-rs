use std::collections::{BTreeMap, BTreeSet};

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        function::{BinOp, LitKind, UnOp},
        item::{AdtDef, ContractItem, Item, Module},
    },
    span::Span,
};
use hir_ty::{BuiltinTyCtor, Ty as SemTy, TyCtor, TyKind as SemTyKind, UserTyCtorKind};
use parser::parse_file_to_hir;
use specialize::{
    MonoArm, MonoCallOrigin, MonoContract, MonoExpr, MonoExprKind, MonoFunction, MonoIntrinsic,
    MonoItem, MonoModule, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind,
};

use crate::ir::{
    Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
    StmtKind, Ty, TyKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitOptions {
    pub emit_dispatcher_comments: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            emit_dispatcher_comments: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitOutput<'db> {
    pub program: Program<'db>,
    pub diagnostics: Vec<EmitDiagnostic<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitDiagnostic<'db> {
    pub span: Span<'db>,
    pub kind: EmitDiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitDiagnosticKind {
    UnsupportedType { ty: String },
    UnsupportedLiteral { literal: String },
    UnsupportedMonoConstruct { construct: String },
    MissingAdtLayout { adt: String },
    MissingConstructor { constructor: String, ty: String },
    MultiScrutineeMatch { count: usize },
    EmptyMatch,
    DispatcherDeferred { contract: String },
}

#[derive(Debug, Clone)]
struct AdtLayout<'db> {
    name: String,
    target: Ty<'db>,
    ctors: Vec<CtorLayout<'db>>,
}

#[derive(Debug, Clone)]
struct CtorLayout<'db> {
    name: String,
    payload: Ty<'db>,
}

#[derive(Debug, Clone)]
struct Branch<'db> {
    binder: String,
    body: Vec<Stmt<'db>>,
}

struct Emitter<'db> {
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    options: EmitOptions,
    diagnostics: Vec<EmitDiagnostic<'db>>,
    scopes: Vec<BTreeMap<String, Expr<'db>>>,
    fresh: usize,
}

pub fn emit_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: &MonoModule<'db>,
    options: EmitOptions,
) -> EmitOutput<'db> {
    Emitter::new(db, module, options).emit(module)
}

impl<'db> Emitter<'db> {
    fn new(db: &'db dyn hir_ty::Db, module: &MonoModule<'db>, options: EmitOptions) -> Self {
        let hir_module = parse_file_to_hir(db, module.module.file(db)).module(db);
        Self {
            db,
            module: hir_module,
            options,
            diagnostics: Vec::new(),
            scopes: vec![BTreeMap::new()],
            fresh: 0,
        }
    }

    fn emit(mut self, module: &MonoModule<'db>) -> EmitOutput<'db> {
        let span = self.module.span(self.db);
        let mut functions = BTreeMap::<String, Function<'db>>::new();
        let mut contracts = Vec::new();
        for item in &module.items {
            match item {
                MonoItem::Function(function) => {
                    let function = self.emit_function(function);
                    functions.insert(function.name.clone(), function);
                }
                MonoItem::Contract(contract) => contracts.push(contract.clone()),
                MonoItem::Adt(_) => {}
            }
        }

        let program = if contracts.is_empty() {
            Program {
                span,
                functions: functions.into_values().collect(),
                objects: Vec::new(),
            }
        } else {
            let all_functions = functions.values().cloned().collect::<Vec<_>>();
            let objects = contracts
                .iter()
                .map(|contract| self.emit_contract(contract, &all_functions))
                .collect();
            Program {
                span,
                functions: Vec::new(),
                objects,
            }
        };

        EmitOutput {
            program,
            diagnostics: self.diagnostics,
        }
    }

    fn emit_contract(
        &mut self,
        contract: &MonoContract<'db>,
        functions: &[Function<'db>],
    ) -> Object<'db> {
        let mut constructor_names = BTreeSet::new();
        if let Some(name) = &contract.constructor.specialized {
            constructor_names.insert(name.clone());
        }
        for entry in &contract.entries {
            if matches!(entry.kind, specialize::MonoEntryKind::Constructor) {
                constructor_names.insert(entry.specialized.clone());
            }
        }

        let deployment_functions = functions
            .iter()
            .filter(|function| constructor_names.contains(&function.name))
            .cloned()
            .collect::<Vec<_>>();
        let runtime_functions = functions
            .iter()
            .filter(|function| !constructor_names.contains(&function.name))
            .cloned()
            .collect::<Vec<_>>();

        if self.options.emit_dispatcher_comments
            && contract
                .entries
                .iter()
                .any(|entry| entry.selector.is_some())
        {
            self.push(
                contract.span,
                EmitDiagnosticKind::DispatcherDeferred {
                    contract: contract.name.clone(),
                },
            );
        }

        let mut deploy_stmts = Vec::new();
        if contract.constructor.specialized.is_none() {
            deploy_stmts.push(Stmt {
                span: contract.span,
                kind: StmtKind::Comment(format!("deployment code for {}", contract.name)),
            });
        }

        let mut runtime_stmts = Vec::new();
        if self.options.emit_dispatcher_comments {
            for entry in &contract.entries {
                if let Some(selector) = entry.selector {
                    runtime_stmts.push(Stmt {
                        span: entry.span,
                        kind: StmtKind::Comment(format!(
                            "selector 0x{:02x}{:02x}{:02x}{:02x} -> {}",
                            selector[0], selector[1], selector[2], selector[3], entry.specialized
                        )),
                    });
                }
            }
        }

        Object {
            span: contract.span,
            name: contract.name.clone(),
            code: CodeBlock {
                span: contract.span,
                stmts: deploy_stmts,
                functions: deployment_functions,
            },
            inners: vec![Object {
                span: contract.span,
                name: format!("{}_deployed", contract.name),
                code: CodeBlock {
                    span: contract.span,
                    stmts: runtime_stmts,
                    functions: runtime_functions,
                },
                inners: Vec::new(),
            }],
        }
    }

    fn emit_function(&mut self, function: &MonoFunction<'db>) -> Function<'db> {
        self.with_scope(|this| {
            let args = function
                .params
                .iter()
                .filter_map(|param| {
                    if param.comptime {
                        this.push(
                            param.span,
                            EmitDiagnosticKind::UnsupportedMonoConstruct {
                                construct: format!("comptime parameter `{}`", param.name),
                            },
                        );
                        return None;
                    }
                    let ty = this.hull_ty(param.ty.ty(), param.span);
                    Some(Arg {
                        span: param.span,
                        name: param.name.clone(),
                        ty,
                    })
                })
                .collect::<Vec<_>>();
            let ret = this.hull_ty(function.ret.ty(), function.span);
            let body = this.emit_stmts(&function.body);
            Function {
                span: function.span,
                name: function.name.clone(),
                args,
                ret,
                body,
            }
        })
    }

    fn emit_stmts(&mut self, stmts: &[MonoStmt<'db>]) -> Vec<Stmt<'db>> {
        stmts.iter().flat_map(|stmt| self.emit_stmt(stmt)).collect()
    }

    fn emit_stmt(&mut self, stmt: &MonoStmt<'db>) -> Vec<Stmt<'db>> {
        match &stmt.kind {
            MonoStmtKind::Let { id, ty, init, .. } => {
                let declared = ty
                    .map(|ty| self.hull_ty(ty.ty(), stmt.span))
                    .unwrap_or_else(|| self.hull_ty(id.ty.ty(), stmt.span));
                let mut out = vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Let {
                        name: id.name.clone(),
                        ty: declared,
                    },
                }];
                if let Some(init) = init {
                    out.push(Stmt {
                        span: stmt.span,
                        kind: StmtKind::Assign {
                            lhs: Expr::var(
                                stmt.span,
                                id.name.clone(),
                                self.hull_ty(id.ty.ty(), id.span),
                            ),
                            rhs: self.emit_expr(init),
                        },
                    });
                }
                out
            }
            MonoStmtKind::Return(expr) => {
                let expr = expr
                    .as_ref()
                    .map(|expr| self.emit_expr(expr))
                    .unwrap_or_else(|| Expr::unit(stmt.span));
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Return(expr),
                }]
            }
            MonoStmtKind::Expr(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Expr(self.emit_expr(expr)),
            }],
            MonoStmtKind::Assign { lhs, rhs } => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Assign {
                    lhs: self.emit_expr(lhs),
                    rhs: self.emit_expr(rhs),
                },
            }],
            MonoStmtKind::AddAssign { lhs, rhs } => self.emit_assign_op(stmt.span, lhs, "add", rhs),
            MonoStmtKind::SubAssign { lhs, rhs } => self.emit_assign_op(stmt.span, lhs, "sub", rhs),
            MonoStmtKind::BitXorAssign { lhs, rhs } => {
                self.emit_assign_op(stmt.span, lhs, "xor", rhs)
            }
            MonoStmtKind::BitAndAssign { lhs, rhs } => {
                self.emit_assign_op(stmt.span, lhs, "and", rhs)
            }
            MonoStmtKind::BitOrAssign { lhs, rhs } => {
                self.emit_assign_op(stmt.span, lhs, "or", rhs)
            }
            MonoStmtKind::ModAssign { lhs, rhs } => self.emit_assign_op(stmt.span, lhs, "mod", rhs),
            MonoStmtKind::Match { scrutinees, arms } => {
                self.emit_match(stmt.span, scrutinees, arms)
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => vec![self.emit_if_stmt(stmt.span, cond, then_body, else_body.as_deref())],
            MonoStmtKind::Block(body) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Block(self.with_scope(|this| this.emit_stmts(body))),
            }],
            MonoStmtKind::Assembly(body) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Assembly(body.clone()),
            }],
            MonoStmtKind::For { .. } => {
                self.push(
                    stmt.span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: "for loop".to_owned(),
                    },
                );
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Revert("unsupported for loop".to_owned()),
                }]
            }
            MonoStmtKind::Break | MonoStmtKind::Continue => {
                self.push(
                    stmt.span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: "loop control".to_owned(),
                    },
                );
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Revert("unsupported loop control".to_owned()),
                }]
            }
            MonoStmtKind::Error => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Revert("error statement".to_owned()),
            }],
        }
    }

    fn emit_assign_op(
        &mut self,
        span: Span<'db>,
        lhs: &MonoExpr<'db>,
        callee: &str,
        rhs: &MonoExpr<'db>,
    ) -> Vec<Stmt<'db>> {
        let lhs_expr = self.emit_expr(lhs);
        let rhs_expr = self.emit_expr(rhs);
        let call = Expr {
            span,
            ty: lhs_expr.ty.clone(),
            kind: ExprKind::Call {
                callee: callee.to_owned(),
                args: vec![lhs_expr.clone(), rhs_expr],
            },
        };
        vec![Stmt {
            span,
            kind: StmtKind::Assign {
                lhs: lhs_expr,
                rhs: call,
            },
        }]
    }

    fn emit_if_stmt(
        &mut self,
        span: Span<'db>,
        cond: &MonoExpr<'db>,
        then_body: &[MonoStmt<'db>],
        else_body: Option<&[MonoStmt<'db>]>,
    ) -> Stmt<'db> {
        let target = self.hull_ty(cond.ty.ty(), cond.span);
        let scrutinee = self.emit_expr(cond);
        let then_stmts = self.with_scope(|this| this.emit_stmts(then_body));
        let else_stmts = else_body
            .map(|body| self.with_scope(|this| this.emit_stmts(body)))
            .unwrap_or_default();
        Stmt {
            span,
            kind: StmtKind::Match {
                target,
                scrutinee,
                alts: vec![
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inr),
                        },
                        binder: self.fresh_alt(),
                        body: then_stmts,
                    },
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inl),
                        },
                        binder: self.fresh_alt(),
                        body: else_stmts,
                    },
                ],
            },
        }
    }

    fn emit_expr(&mut self, expr: &MonoExpr<'db>) -> Expr<'db> {
        let ty = self.hull_ty(expr.ty.ty(), expr.span);
        match &expr.kind {
            MonoExprKind::Var(id) => self.lookup_expr(&id.name).unwrap_or_else(|| Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Var(id.name.clone()),
            }),
            MonoExprKind::Lit(lit) => self.emit_lit(expr.span, lit),
            MonoExprKind::Tuple(elems) => {
                let elems = elems
                    .iter()
                    .map(|elem| self.emit_expr(elem))
                    .collect::<Vec<_>>();
                product_expr(expr.span, ty, elems)
            }
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: call_name(origin, &callee.name),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            },
            MonoExprKind::Con { ctor, args } => self.emit_constructor(expr, &ctor.name, args),
            MonoExprKind::BinOp { lhs, op, rhs } => self.emit_bin_op(expr.span, ty, lhs, *op, rhs),
            MonoExprKind::UnaryOp { op, expr: inner } => {
                self.emit_unary_op(expr.span, ty, *op, inner)
            }
            MonoExprKind::TypeAnnot { expr: inner, .. } => self.emit_expr(inner),
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => Expr {
                span: expr.span,
                ty: ty.clone(),
                kind: ExprKind::If {
                    target: ty,
                    cond: Box::new(self.emit_expr(cond)),
                    then_expr: Box::new(self.emit_expr(then_expr)),
                    else_expr: Box::new(self.emit_expr(else_expr)),
                },
            },
            MonoExprKind::Field { .. }
            | MonoExprKind::Index { .. }
            | MonoExprKind::Proxy(_)
            | MonoExprKind::Lambda { .. }
            | MonoExprKind::ClosureDispatch { .. }
            | MonoExprKind::Error => {
                self.push(
                    expr.span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: mono_expr_name(&expr.kind).to_owned(),
                    },
                );
                Expr {
                    span: expr.span,
                    ty,
                    kind: ExprKind::Call {
                        callee: "unsupported".to_owned(),
                        args: Vec::new(),
                    },
                }
            }
        }
    }

    fn emit_lit(&mut self, span: Span<'db>, lit: &LitKind) -> Expr<'db> {
        match lit {
            LitKind::Number(value) | LitKind::Hex(value) => Expr::word(span, value.clone()),
            LitKind::String(value) => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedLiteral {
                        literal: value.clone(),
                    },
                );
                Expr::word(span, "0")
            }
            LitKind::Error => Expr::word(span, "0"),
        }
    }

    fn emit_constructor(
        &mut self,
        expr: &MonoExpr<'db>,
        ctor_name: &str,
        args: &[MonoExpr<'db>],
    ) -> Expr<'db> {
        let target = self.hull_ty(expr.ty.ty(), expr.span);
        match ctor_name {
            "()" => return Expr::unit(expr.span),
            "pair" => {
                let args = args.iter().map(|arg| self.emit_expr(arg)).collect();
                return product_expr(expr.span, target, args);
            }
            "true" => {
                let payload = Expr::unit(expr.span);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: ExprKind::Inr {
                        target,
                        value: Box::new(payload),
                    },
                };
            }
            "false" => {
                let payload = Expr::unit(expr.span);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: ExprKind::Inl {
                        target,
                        value: Box::new(payload),
                    },
                };
            }
            "inl" | "inr" if args.len() == 1 => {
                let value = self.emit_expr(&args[0]);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: if ctor_name == "inl" {
                        ExprKind::Inl {
                            target,
                            value: Box::new(value),
                        }
                    } else {
                        ExprKind::Inr {
                            target,
                            value: Box::new(value),
                        }
                    },
                };
            }
            _ => {}
        }

        let Some(layout) = self.adt_layout_for_sem_ty(expr.ty.ty(), expr.span) else {
            self.push(
                expr.span,
                EmitDiagnosticKind::MissingAdtLayout {
                    adt: expr.ty.ty().display(self.db),
                },
            );
            return Expr {
                span: expr.span,
                ty: target,
                kind: ExprKind::Call {
                    callee: ctor_name.to_owned(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            };
        };
        let Some(index) = layout
            .ctors
            .iter()
            .position(|ctor| constructor_name_matches(ctor_name, &layout.name, &ctor.name))
        else {
            self.push(
                expr.span,
                EmitDiagnosticKind::MissingConstructor {
                    constructor: ctor_name.to_owned(),
                    ty: layout.name,
                },
            );
            return Expr {
                span: expr.span,
                ty: target,
                kind: ExprKind::Call {
                    callee: ctor_name.to_owned(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            };
        };
        let payload_ty = layout.ctors[index].payload.clone();
        let payload_args = args
            .iter()
            .map(|arg| self.emit_expr(arg))
            .collect::<Vec<_>>();
        let payload = product_expr(expr.span, payload_ty, payload_args);
        encode_constructor(expr.span, layout.target, index, payload)
    }

    fn emit_bin_op(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        lhs: &MonoExpr<'db>,
        op: BinOp,
        rhs: &MonoExpr<'db>,
    ) -> Expr<'db> {
        let Some(callee) = bin_op_name(op) else {
            self.push(
                span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: format!("binary operator {op:?}"),
                },
            );
            return Expr {
                span,
                ty,
                kind: ExprKind::Call {
                    callee: "unsupported".to_owned(),
                    args: Vec::new(),
                },
            };
        };
        Expr {
            span,
            ty,
            kind: ExprKind::Call {
                callee: callee.to_owned(),
                args: vec![self.emit_expr(lhs), self.emit_expr(rhs)],
            },
        }
    }

    fn emit_unary_op(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        op: UnOp,
        expr: &MonoExpr<'db>,
    ) -> Expr<'db> {
        match op {
            UnOp::Not => Expr {
                span,
                ty,
                kind: ExprKind::Call {
                    callee: "iszero".to_owned(),
                    args: vec![self.emit_expr(expr)],
                },
            },
            UnOp::Error => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: "unary error".to_owned(),
                    },
                );
                Expr {
                    span,
                    ty,
                    kind: ExprKind::Call {
                        callee: "unsupported".to_owned(),
                        args: Vec::new(),
                    },
                }
            }
        }
    }

    fn emit_match(
        &mut self,
        span: Span<'db>,
        scrutinees: &[MonoExpr<'db>],
        arms: &[MonoArm<'db>],
    ) -> Vec<Stmt<'db>> {
        if scrutinees.is_empty() {
            self.push(span, EmitDiagnosticKind::EmptyMatch);
            return vec![Stmt {
                span,
                kind: StmtKind::Revert("empty match".to_owned()),
            }];
        }
        if scrutinees.len() != 1 {
            self.push(
                span,
                EmitDiagnosticKind::MultiScrutineeMatch {
                    count: scrutinees.len(),
                },
            );
            return vec![Stmt {
                span,
                kind: StmtKind::Revert("multi-scrutinee match deferred".to_owned()),
            }];
        }
        let scrutinee = self.emit_expr(&scrutinees[0]);
        let target = self.hull_ty(scrutinees[0].ty.ty(), scrutinees[0].span);
        let Some(first_pat) = arms.first().and_then(|arm| arm.pats.first()) else {
            self.push(span, EmitDiagnosticKind::EmptyMatch);
            return vec![Stmt {
                span,
                kind: StmtKind::Revert("empty match".to_owned()),
            }];
        };

        if matches!(
            first_pat.kind,
            MonoPatKind::Lit(_) | MonoPatKind::ComptimeLabel(_)
        ) || self.semantic_ty_is_word(scrutinees[0].ty.ty())
        {
            return vec![self.emit_word_match(span, target, scrutinee, arms)];
        }

        if let Some(layout) = self.adt_layout_for_sem_ty(scrutinees[0].ty.ty(), scrutinees[0].span)
        {
            return vec![self.emit_sum_match(span, scrutinee, layout, arms)];
        }

        arms.first()
            .map(|arm| {
                self.with_scope(|this| {
                    this.bind_pattern_projection(&scrutinee, arm.pats.first())
                        .emit_stmts(&arm.body)
                })
            })
            .unwrap_or_default()
    }

    fn emit_word_match(
        &mut self,
        span: Span<'db>,
        target: Ty<'db>,
        scrutinee: Expr<'db>,
        arms: &[MonoArm<'db>],
    ) -> Stmt<'db> {
        let mut alts = Vec::new();
        for arm in arms {
            let Some(pat) = arm.pats.first() else {
                continue;
            };
            let binder = self.fresh_alt();
            let hull_pat = match &pat.kind {
                MonoPatKind::Lit(LitKind::Number(value))
                | MonoPatKind::Lit(LitKind::Hex(value)) => Pat {
                    span: pat.span,
                    kind: PatKind::IntLit(value.clone()),
                },
                MonoPatKind::Var(id) => {
                    let expr = scrutinee.clone();
                    self.with_scope(|this| {
                        this.bind_expr(id.name.clone(), expr);
                    });
                    Pat {
                        span: pat.span,
                        kind: PatKind::Var(id.name.clone()),
                    }
                }
                MonoPatKind::Wildcard => Pat {
                    span: pat.span,
                    kind: PatKind::Wildcard,
                },
                _ => Pat {
                    span: pat.span,
                    kind: PatKind::Wildcard,
                },
            };
            let body = self.with_scope(|this| {
                if let MonoPatKind::Var(id) = &pat.kind {
                    this.bind_expr(id.name.clone(), scrutinee.clone());
                }
                this.emit_stmts(&arm.body)
            });
            alts.push(Alt {
                span: arm.span,
                pat: hull_pat,
                binder,
                body,
            });
        }
        Stmt {
            span,
            kind: StmtKind::Match {
                target,
                scrutinee,
                alts,
            },
        }
    }

    fn emit_sum_match(
        &mut self,
        span: Span<'db>,
        scrutinee: Expr<'db>,
        layout: AdtLayout<'db>,
        arms: &[MonoArm<'db>],
    ) -> Stmt<'db> {
        let mut branches = layout
            .ctors
            .iter()
            .map(|ctor| Branch {
                binder: self.fresh_alt(),
                body: vec![Stmt {
                    span,
                    kind: StmtKind::Revert(format!("no match for: {}", ctor.name)),
                }],
            })
            .collect::<Vec<_>>();

        for arm in arms {
            let Some(pat) = arm.pats.first() else {
                continue;
            };
            match &pat.kind {
                MonoPatKind::Wildcard => {
                    for branch in &mut branches {
                        branch.body = self.with_scope(|this| this.emit_stmts(&arm.body));
                    }
                }
                MonoPatKind::Var(id) => {
                    for branch in &mut branches {
                        let scrutinee = scrutinee.clone();
                        branch.body = self.with_scope(|this| {
                            this.bind_expr(id.name.clone(), scrutinee);
                            this.emit_stmts(&arm.body)
                        });
                    }
                }
                MonoPatKind::Con { ctor, args } => {
                    if let Some(index) = layout.ctors.iter().position(|candidate| {
                        constructor_name_matches(&ctor.name, &layout.name, &candidate.name)
                    }) {
                        let binder = branches[index].binder.clone();
                        let binder_expr = Expr::var(
                            pat.span,
                            binder.clone(),
                            layout.ctors[index].payload.clone(),
                        );
                        let mut body = self.with_scope(|this| {
                            this.bind_pattern_args(&binder_expr, args);
                            this.emit_stmts(&arm.body)
                        });
                        body.insert(
                            0,
                            Stmt {
                                span: pat.span,
                                kind: StmtKind::Comment(source_constructor_comment(&ctor.name)),
                            },
                        );
                        branches[index].body = body;
                    }
                }
                _ => {}
            }
        }

        build_nested_sum_match(span, scrutinee, layout.target, branches)
    }

    fn bind_pattern_projection(
        &mut self,
        scrutinee: &Expr<'db>,
        pat: Option<&MonoPat<'db>>,
    ) -> &mut Self {
        let Some(pat) = pat else {
            return self;
        };
        match &pat.kind {
            MonoPatKind::Var(id) => self.bind_expr(id.name.clone(), scrutinee.clone()),
            MonoPatKind::Tuple(elems) | MonoPatKind::Con { args: elems, .. } => {
                self.bind_pattern_args(scrutinee, elems);
            }
            MonoPatKind::Wildcard
            | MonoPatKind::Lit(_)
            | MonoPatKind::ComptimeLabel(_)
            | MonoPatKind::Error => {}
        }
        self
    }

    fn bind_pattern_args(&mut self, base: &Expr<'db>, args: &[MonoPat<'db>]) {
        match args {
            [] => {}
            [one] => {
                self.bind_pattern_projection(base, Some(one));
            }
            [head, tail @ ..] => {
                let fst = Expr {
                    span: base.span,
                    ty: product_left_ty(&base.ty),
                    kind: ExprKind::Fst(Box::new(base.clone())),
                };
                self.bind_pattern_projection(&fst, Some(head));
                let snd = Expr {
                    span: base.span,
                    ty: product_right_ty(&base.ty),
                    kind: ExprKind::Snd(Box::new(base.clone())),
                };
                self.bind_pattern_args(&snd, tail);
            }
        }
    }

    fn hull_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Ty<'db> {
        match self.try_hull_ty(ty, span) {
            Some(ty) => ty,
            None => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedType {
                        ty: ty.display(self.db),
                    },
                );
                Ty::word(span)
            }
        }
    }

    fn try_hull_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Option<Ty<'db>> {
        match ty.kind(self.db) {
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
                args,
            } if args.is_empty() => Some(Ty::word(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => Some(Ty::unit(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(bool_sum_ty(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => Some(Ty::product(
                span,
                self.hull_ty(args[0], span),
                self.hull_ty(args[1], span),
            )),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(Ty::sum(
                span,
                self.hull_ty(args[0], span),
                self.hull_ty(args[1], span),
            )),
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if matches!(user.kind, UserTyCtorKind::Adt) => {
                let layout = self.adt_layout(user.def, args, span)?;
                Some(layout.target)
            }
            SemTyKind::Function { params, ret } => Some(Ty::function(
                span,
                params
                    .iter()
                    .map(|param| self.hull_ty(*param, span))
                    .collect(),
                self.hull_ty(*ret, span),
            )),
            SemTyKind::Tuple(elems) => Some(tuple_ty(
                span,
                elems.iter().map(|elem| self.hull_ty(*elem, span)).collect(),
            )),
            SemTyKind::Comptime(inner) => self.try_hull_ty(*inner, span),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Integer | BuiltinTyCtor::String),
                ..
            }
            | SemTyKind::Named { .. }
            | SemTyKind::Error
            | SemTyKind::Unknown
            | SemTyKind::BoundVar(_) => None,
        }
    }

    fn adt_layout_for_sem_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Option<AdtLayout<'db>> {
        match ty.kind(self.db) {
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(AdtLayout {
                name: "Bool".to_owned(),
                target: bool_sum_ty(span),
                ctors: vec![
                    CtorLayout {
                        name: "false".to_owned(),
                        payload: Ty::unit(span),
                    },
                    CtorLayout {
                        name: "true".to_owned(),
                        payload: Ty::unit(span),
                    },
                ],
            }),
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if matches!(user.kind, UserTyCtorKind::Adt) => self.adt_layout(user.def, args, span),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(AdtLayout {
                name: "sum".to_owned(),
                target: self.hull_ty(ty, span),
                ctors: vec![
                    CtorLayout {
                        name: "inl".to_owned(),
                        payload: self.hull_ty(args[0], span),
                    },
                    CtorLayout {
                        name: "inr".to_owned(),
                        payload: self.hull_ty(args[1], span),
                    },
                ],
            }),
            _ => None,
        }
    }

    fn adt_layout(
        &mut self,
        def: DefId<'db>,
        args: &[SemTy<'db>],
        span: Span<'db>,
    ) -> Option<AdtLayout<'db>> {
        let module = parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
        let adt = find_adt(self.db, module, def)?;
        let plan = hir_ty::derived_generic_plan(self.db, module, adt)?;
        let rep = subst_sem_ty(self.db, plan.rep, args);
        let inner = self.hull_ty(rep, span);
        let name = def.name(self.db).unwrap_or_else(|| "Adt".to_owned());
        let target = Ty::named(span, name.clone(), inner);
        let ctors = plan
            .from_arms
            .iter()
            .map(|arm| CtorLayout {
                name: arm.ctor_name.clone(),
                payload: self.hull_ty(subst_sem_ty(self.db, arm.product_rep, args), span),
            })
            .collect();
        Some(AdtLayout {
            name,
            target,
            ctors,
        })
    }

    fn semantic_ty_is_word(&self, ty: SemTy<'db>) -> bool {
        matches!(
            ty.kind(self.db),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
                args,
            } if args.is_empty()
        )
    }

    fn fresh_alt(&mut self) -> String {
        let name = format!("$alt{}", self.fresh);
        self.fresh += 1;
        name
    }

    fn bind_expr(&mut self, name: String, expr: Expr<'db>) {
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .insert(name, expr);
    }

    fn lookup_expr(&self, name: &str) -> Option<Expr<'db>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn with_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.scopes.push(BTreeMap::new());
        let out = f(self);
        self.scopes.pop();
        out
    }

    fn push(&mut self, span: Span<'db>, kind: EmitDiagnosticKind) {
        self.diagnostics.push(EmitDiagnostic { span, kind });
    }
}

fn call_name(origin: &MonoCallOrigin<'_>, name: &str) -> String {
    match origin {
        MonoCallOrigin::Builtin(intrinsic) => intrinsic_name(*intrinsic).to_owned(),
        MonoCallOrigin::Source(_) | MonoCallOrigin::Unknown => name.to_owned(),
    }
}

fn intrinsic_name(intrinsic: MonoIntrinsic) -> &'static str {
    match intrinsic {
        MonoIntrinsic::PrimAddWord => "primAddWord",
        MonoIntrinsic::PrimEqWord => "primEqWord",
        MonoIntrinsic::SubWord => "subWord",
        MonoIntrinsic::GtWord => "gtWord",
        MonoIntrinsic::BxorWord => "bxorWord",
        MonoIntrinsic::BandWord => "bandWord",
        MonoIntrinsic::BorWord => "borWord",
        MonoIntrinsic::WordToInteger => "wordToInteger",
        MonoIntrinsic::WordFromInteger => "wordFromInteger",
        MonoIntrinsic::IntegerAdd => "integerAdd",
        MonoIntrinsic::IntegerSub => "integerSub",
        MonoIntrinsic::IntegerMul => "integerMul",
        MonoIntrinsic::IntegerLt => "integerLt",
        MonoIntrinsic::IntegerEq => "integerEq",
        MonoIntrinsic::ConcatLit => "concatLit",
        MonoIntrinsic::StrlenLit => "strlenLit",
        MonoIntrinsic::KeccakLit => "keccakLit",
    }
}

fn bin_op_name(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("add"),
        BinOp::Sub => Some("sub"),
        BinOp::Mul => Some("mul"),
        BinOp::Div => Some("div"),
        BinOp::Mod => Some("mod"),
        BinOp::BitAnd => Some("and"),
        BinOp::BitXor => Some("xor"),
        BinOp::BitOr => Some("or"),
        BinOp::Eq => Some("primEqWord"),
        BinOp::Lt => Some("lt"),
        BinOp::Gt => Some("gt"),
        BinOp::NotEq | BinOp::LtEq | BinOp::GtEq | BinOp::And | BinOp::Or | BinOp::Error => None,
    }
}

fn mono_expr_name(kind: &MonoExprKind<'_>) -> &'static str {
    match kind {
        MonoExprKind::Field { .. } => "field access",
        MonoExprKind::Index { .. } => "index access",
        MonoExprKind::Proxy(_) => "proxy expression",
        MonoExprKind::Lambda { .. } => "lambda expression",
        MonoExprKind::ClosureDispatch { .. } => "closure dispatch",
        MonoExprKind::Error => "error expression",
        _ => "expression",
    }
}

fn product_expr<'db>(span: Span<'db>, ty: Ty<'db>, elems: Vec<Expr<'db>>) -> Expr<'db> {
    match elems.as_slice() {
        [] => Expr::unit(span),
        [one] => {
            let mut one = one.clone();
            one.ty = ty;
            one
        }
        [head, tail @ ..] => {
            let tail_ty = product_right_ty(&ty);
            Expr {
                span,
                ty: ty.clone(),
                kind: ExprKind::Pair(
                    Box::new(head.clone()),
                    Box::new(product_expr(span, tail_ty, tail.to_vec())),
                ),
            }
        }
    }
}

fn tuple_ty<'db>(span: Span<'db>, elems: Vec<Ty<'db>>) -> Ty<'db> {
    match elems.as_slice() {
        [] => Ty::unit(span),
        [one] => one.clone(),
        [head, tail @ ..] => Ty::product(span, head.clone(), tuple_ty(span, tail.to_vec())),
    }
}

fn bool_sum_ty<'db>(span: Span<'db>) -> Ty<'db> {
    Ty::sum(span, Ty::unit(span), Ty::unit(span))
}

fn product_left_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Product(lhs, _) => (**lhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

fn product_right_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Product(_, rhs) => (**rhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

fn sum_right_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Sum(_, rhs) => (**rhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

fn encode_constructor<'db>(
    span: Span<'db>,
    target: Ty<'db>,
    index: usize,
    payload: Expr<'db>,
) -> Expr<'db> {
    let arity = sum_arity(&target);
    if arity <= 1 {
        let mut payload = payload;
        payload.ty = target;
        return payload;
    }
    if index == 0 {
        Expr {
            span,
            ty: target.clone(),
            kind: ExprKind::Inl {
                target,
                value: Box::new(payload),
            },
        }
    } else {
        let right = sum_right_ty(&target);
        let nested = encode_constructor(span, right, index - 1, payload);
        Expr {
            span,
            ty: target.clone(),
            kind: ExprKind::Inr {
                target,
                value: Box::new(nested),
            },
        }
    }
}

fn build_nested_sum_match<'db>(
    span: Span<'db>,
    scrutinee: Expr<'db>,
    target: Ty<'db>,
    branches: Vec<Branch<'db>>,
) -> Stmt<'db> {
    match branches.as_slice() {
        [] => Stmt {
            span,
            kind: StmtKind::Revert("empty branch list".to_owned()),
        },
        [branch] => Stmt {
            span,
            kind: StmtKind::Block(branch.body.clone()),
        },
        [left, rest @ ..] => {
            let right_ty = sum_right_ty(&target);
            let right_binder = rest
                .first()
                .map(|branch| branch.binder.clone())
                .unwrap_or_else(|| "$alt".to_owned());
            let right_expr = Expr::var(span, right_binder.clone(), right_ty.clone());
            let rest_stmt = build_nested_sum_match(span, right_expr, right_ty, rest.to_vec());
            Stmt {
                span,
                kind: StmtKind::Match {
                    target,
                    scrutinee,
                    alts: vec![
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inl),
                            },
                            binder: left.binder.clone(),
                            body: left.body.clone(),
                        },
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inr),
                            },
                            binder: right_binder,
                            body: vec![rest_stmt],
                        },
                    ],
                },
            }
        }
    }
}

fn sum_arity(ty: &Ty<'_>) -> usize {
    match &ty.strip_named().kind {
        TyKind::Sum(_, rhs) => 1 + sum_arity(rhs),
        _ => 1,
    }
}

fn constructor_name_matches(actual: &str, adt: &str, ctor: &str) -> bool {
    actual == ctor || actual == format!("{adt}_{ctor}") || actual.ends_with(&format!("_{ctor}"))
}

fn source_constructor_comment(name: &str) -> String {
    name.rsplit('_').next().unwrap_or(name).to_owned()
}

fn find_adt<'db>(db: &'db dyn HirDb, module: Module<'db>, def: DefId<'db>) -> Option<AdtDef<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_adt_in_item(db, *item, def))
}

fn find_adt_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    match item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(*adt),
            _ => None,
        }),
        _ => None,
    }
}

fn subst_sem_ty<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>, args: &[SemTy<'db>]) -> SemTy<'db> {
    match ty.kind(db) {
        SemTyKind::BoundVar(var) => args.get(var.index as usize).copied().unwrap_or(ty),
        SemTyKind::Named { ctor, args: inner } => SemTy::named(
            db,
            *ctor,
            inner
                .iter()
                .map(|arg| subst_sem_ty(db, *arg, args))
                .collect(),
        ),
        SemTyKind::Function { params, ret } => SemTy::function(
            db,
            params
                .iter()
                .map(|param| subst_sem_ty(db, *param, args))
                .collect(),
            subst_sem_ty(db, *ret, args),
        ),
        SemTyKind::Tuple(elems) => SemTy::tuple(
            db,
            elems
                .iter()
                .map(|elem| subst_sem_ty(db, *elem, args))
                .collect(),
        ),
        SemTyKind::Comptime(inner) => SemTy::comptime(db, subst_sem_ty(db, *inner, args)),
        SemTyKind::Error | SemTyKind::Unknown => ty,
    }
}
