use std::{collections::BTreeMap, fmt};

use hir::{
    Db as HirDb,
    ast::{
        Ident,
        function::{YulExpr, YulExprKind, YulStmt, YulStmtKind},
    },
    diag::{Diagnostic, DiagnosticCode},
    span::{Span, SpannedElem},
};

use crate::{
    ir::{
        Alt, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt, StmtKind, Ty,
        TyKind,
    },
    scope_stack::ScopeStack,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDiagnostic<'db> {
    pub span: Span<'db>,
    pub kind: CheckDiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckDiagnosticKind {
    UndefinedVariable {
        name: String,
    },
    UndefinedFunction {
        name: String,
    },
    DuplicateFunction {
        name: String,
    },
    ArityMismatch {
        name: String,
        expected: usize,
        actual: usize,
    },
    TypeMismatch {
        expected: String,
        actual: String,
    },
    ExprAnnotationMismatch {
        annotated: String,
        inferred: String,
    },
    ExpectedProduct {
        actual: String,
    },
    ExpectedSum {
        actual: String,
    },
    ExpectedBool {
        actual: String,
    },
    BadInjectionIndex {
        index: usize,
        ty: String,
    },
    BadMatchPattern {
        pat: String,
        ty: String,
    },
    ReturnOutsideFunction,
    FunctionTypeNotFirstOrder {
        name: String,
    },
    MissingTerminator {
        function: String,
    },
    AssemblyRequiresDatabase,
    AssemblyReturnCountMismatch {
        context: String,
        expected: usize,
        actual: usize,
    },
    AssemblyExpressionNotUnit {
        actual: String,
    },
    AssemblyExpectedWordArgument {
        actual: String,
    },
    AssemblyExpectedWordAssignment {
        name: String,
        actual: String,
    },
    AssemblyVoidArgument,
}

impl<'db> CheckDiagnostic<'db> {
    pub fn lower(&self, db: &'db dyn HirDb) -> Diagnostic {
        Diagnostic::error(self.kind.to_string())
            .with_code(self.kind.code())
            .with_primary_label(db, self.span, Some(self.kind.primary_label()))
    }
}

impl CheckDiagnosticKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UndefinedVariable { .. } => DiagnosticCode::HULL_UNDEFINED_VARIABLE,
            Self::UndefinedFunction { .. } => DiagnosticCode::HULL_UNDEFINED_FUNCTION,
            Self::DuplicateFunction { .. } => DiagnosticCode::HULL_DUPLICATE_FUNCTION,
            Self::ArityMismatch { .. } => DiagnosticCode::HULL_ARITY_MISMATCH,
            Self::TypeMismatch { .. } => DiagnosticCode::HULL_TYPE_MISMATCH,
            Self::ExprAnnotationMismatch { .. } => DiagnosticCode::HULL_EXPR_ANNOTATION_MISMATCH,
            Self::ExpectedProduct { .. } => DiagnosticCode::HULL_EXPECTED_PRODUCT,
            Self::ExpectedSum { .. } => DiagnosticCode::HULL_EXPECTED_SUM,
            Self::ExpectedBool { .. } => DiagnosticCode::HULL_EXPECTED_BOOL,
            Self::BadInjectionIndex { .. } => DiagnosticCode::HULL_BAD_INJECTION_INDEX,
            Self::BadMatchPattern { .. } => DiagnosticCode::HULL_BAD_MATCH_PATTERN,
            Self::ReturnOutsideFunction => DiagnosticCode::HULL_RETURN_OUTSIDE_FUNCTION,
            Self::FunctionTypeNotFirstOrder { .. } => {
                DiagnosticCode::HULL_FUNCTION_TYPE_NOT_FIRST_ORDER
            }
            Self::MissingTerminator { .. } => DiagnosticCode::HULL_MISSING_TERMINATOR,
            Self::AssemblyRequiresDatabase => DiagnosticCode::HULL_ASSEMBLY_REQUIRES_DATABASE,
            Self::AssemblyReturnCountMismatch { .. } => {
                DiagnosticCode::HULL_ASSEMBLY_RETURN_COUNT_MISMATCH
            }
            Self::AssemblyExpressionNotUnit { .. } => {
                DiagnosticCode::HULL_ASSEMBLY_EXPRESSION_NOT_UNIT
            }
            Self::AssemblyExpectedWordArgument { .. } => {
                DiagnosticCode::HULL_ASSEMBLY_EXPECTED_WORD_ARGUMENT
            }
            Self::AssemblyExpectedWordAssignment { .. } => {
                DiagnosticCode::HULL_ASSEMBLY_EXPECTED_WORD_ASSIGNMENT
            }
            Self::AssemblyVoidArgument => DiagnosticCode::HULL_ASSEMBLY_VOID_ARGUMENT,
        }
    }

    fn primary_label(&self) -> &'static str {
        match self {
            Self::UndefinedVariable { .. } => "undefined variable",
            Self::UndefinedFunction { .. } => "undefined function",
            Self::DuplicateFunction { .. } => "duplicate function",
            Self::ArityMismatch { .. } => "wrong number of arguments",
            Self::TypeMismatch { .. } => "type mismatch",
            Self::ExprAnnotationMismatch { .. } => "annotation mismatch",
            Self::ExpectedProduct { .. } => "product value required",
            Self::ExpectedSum { .. } => "sum value required",
            Self::ExpectedBool { .. } => "boolean value required",
            Self::BadInjectionIndex { .. } => "bad injection index",
            Self::BadMatchPattern { .. } => "bad match pattern",
            Self::ReturnOutsideFunction => "return outside function",
            Self::FunctionTypeNotFirstOrder { .. } => "function type is not first-order",
            Self::MissingTerminator { .. } => "missing terminator",
            Self::AssemblyRequiresDatabase => "database required for assembly check",
            Self::AssemblyReturnCountMismatch { .. } => "assembly return count mismatch",
            Self::AssemblyExpressionNotUnit { .. } => "assembly expression must be unit",
            Self::AssemblyExpectedWordArgument { .. } => "assembly argument must be word",
            Self::AssemblyExpectedWordAssignment { .. } => "assembly assignment must be word",
            Self::AssemblyVoidArgument => "assembly argument has no value",
        }
    }
}

impl fmt::Display for CheckDiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedVariable { name } => write!(f, "undefined Hull variable `{name}`"),
            Self::UndefinedFunction { name } => write!(f, "undefined Hull function `{name}`"),
            Self::DuplicateFunction { name } => write!(f, "duplicate Hull function `{name}`"),
            Self::ArityMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "wrong arity for Hull function `{name}`: expected {expected}, got {actual}"
            ),
            Self::TypeMismatch { expected, actual } => {
                write!(f, "Hull type mismatch: expected {expected}, got {actual}")
            }
            Self::ExprAnnotationMismatch {
                annotated,
                inferred,
            } => write!(
                f,
                "Hull expression annotation mismatch: annotated {annotated}, inferred {inferred}"
            ),
            Self::ExpectedProduct { actual } => write!(f, "expected Hull product, got {actual}"),
            Self::ExpectedSum { actual } => write!(f, "expected Hull sum, got {actual}"),
            Self::ExpectedBool { actual } => write!(f, "expected Hull bool, got {actual}"),
            Self::BadInjectionIndex { index, ty } => {
                write!(f, "bad Hull injection index {index} for {ty}")
            }
            Self::BadMatchPattern { pat, ty } => {
                write!(f, "Hull pattern {pat} does not match {ty}")
            }
            Self::ReturnOutsideFunction => write!(f, "Hull return appears outside a function"),
            Self::FunctionTypeNotFirstOrder { name } => {
                write!(f, "Hull function `{name}` has a non-first-order type")
            }
            Self::MissingTerminator { function } => {
                write!(f, "Hull function `{function}` is missing a terminator")
            }
            Self::AssemblyRequiresDatabase => {
                write!(f, "cannot check inline assembly without a source database")
            }
            Self::AssemblyReturnCountMismatch {
                context,
                expected,
                actual,
            } => write!(
                f,
                "inline assembly {context} returns {actual} values, expected {expected}"
            ),
            Self::AssemblyExpressionNotUnit { actual } => {
                write!(
                    f,
                    "inline assembly expression must have unit type, got {actual}"
                )
            }
            Self::AssemblyExpectedWordArgument { actual } => {
                write!(
                    f,
                    "inline assembly argument must have word type, got {actual}"
                )
            }
            Self::AssemblyExpectedWordAssignment { name, actual } => write!(
                f,
                "inline assembly assignment to `{name}` requires word type, got {actual}"
            ),
            Self::AssemblyVoidArgument => {
                write!(f, "inline assembly argument does not produce a value")
            }
        }
    }
}

#[derive(Debug, Clone)]
struct FunSig<'db> {
    args: Vec<Ty<'db>>,
    ret: Ty<'db>,
}

struct Env<'db> {
    db: Option<&'db dyn HirDb>,
    vars: ScopeStack<BTreeMap<String, Ty<'db>>>,
    funs: BTreeMap<String, FunSig<'db>>,
    ret: Option<Ty<'db>>,
    diagnostics: Vec<CheckDiagnostic<'db>>,
}

pub fn check_program<'db>(program: &Program<'db>) -> Vec<CheckDiagnostic<'db>> {
    check_program_inner(None, program)
}

pub fn check_program_with_db<'db>(
    db: &'db dyn HirDb,
    program: &Program<'db>,
) -> Vec<CheckDiagnostic<'db>> {
    check_program_inner(Some(db), program)
}

fn check_program_inner<'db>(
    db: Option<&'db dyn HirDb>,
    program: &Program<'db>,
) -> Vec<CheckDiagnostic<'db>> {
    let mut env = Env {
        db,
        vars: ScopeStack::new_root(BTreeMap::new()),
        funs: builtin_funs(program.span),
        ret: None,
        diagnostics: Vec::new(),
    };
    for function in &program.functions {
        env.register_function(function);
    }
    for function in &program.functions {
        env.check_function(function);
    }
    for object in &program.objects {
        env.check_object(object);
    }
    env.diagnostics
}

impl<'db> Env<'db> {
    fn register_function(&mut self, function: &Function<'db>) {
        if self.funs.contains_key(&function.name) {
            self.push(
                function.span,
                CheckDiagnosticKind::DuplicateFunction {
                    name: function.name.clone(),
                },
            );
        }
        self.funs.insert(
            function.name.clone(),
            FunSig {
                args: function.args.iter().map(|arg| arg.ty.clone()).collect(),
                ret: function.ret.clone(),
            },
        );
    }

    fn check_object(&mut self, object: &Object<'db>) {
        let saved_funs = self.funs.clone();
        for function in &object.code.functions {
            self.register_function(function);
        }
        self.with_scope(|env| {
            for function in &object.code.functions {
                env.check_function(function);
            }
            env.check_body(&object.code.stmts);
        });
        // Yul object scoping: an inner object's code does not see the outer
        // object's functions, so restore before recursing.
        self.funs = saved_funs;
        for inner in &object.inners {
            self.check_object(inner);
        }
    }

    fn check_function(&mut self, function: &Function<'db>) {
        for arg in &function.args {
            if arg.ty.contains_function() {
                self.push(
                    arg.span,
                    CheckDiagnosticKind::FunctionTypeNotFirstOrder {
                        name: function.name.clone(),
                    },
                );
            }
        }
        if function.ret.contains_function() {
            self.push(
                function.ret.span,
                CheckDiagnosticKind::FunctionTypeNotFirstOrder {
                    name: function.name.clone(),
                },
            );
        }
        self.with_scope(|env| {
            for arg in &function.args {
                env.insert_var(arg.name.clone(), arg.ty.clone());
            }
            let saved_ret = env.ret.clone();
            env.ret = Some(function.ret.clone());
            env.check_body(&function.body);
            if requires_terminator(&function.ret) && !body_terminates(&function.body, env.db) {
                env.push(
                    function.span,
                    CheckDiagnosticKind::MissingTerminator {
                        function: function.name.clone(),
                    },
                );
            }
            env.ret = saved_ret;
        });
    }

    fn check_body(&mut self, body: &[Stmt<'db>]) {
        for stmt in body {
            self.check_stmt(stmt);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt<'db>) {
        match &stmt.kind {
            StmtKind::Let { name, ty } => self.insert_var(name.clone(), ty.clone()),
            StmtKind::Assign { lhs, rhs } => {
                let lhs_ty = self.check_expr(lhs);
                let rhs_ty = self.check_expr(rhs);
                self.expect_type(lhs.span, &lhs_ty, &rhs_ty);
            }
            StmtKind::Expr(expr) => {
                self.check_expr(expr);
            }
            StmtKind::Return(expr) => {
                let actual = self.check_expr(expr);
                match self.ret.clone() {
                    Some(expected) => self.expect_type(expr.span, &expected, &actual),
                    None => self.push(expr.span, CheckDiagnosticKind::ReturnOutsideFunction),
                }
            }
            StmtKind::Block(stmts) => self.with_scope(|env| env.check_body(stmts)),
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => self.with_scope(|env| {
                env.check_body(init);
                let cond_ty = env.check_expr(cond);
                if !is_bool_like(&cond_ty) {
                    env.push(
                        cond.span,
                        CheckDiagnosticKind::ExpectedBool {
                            actual: ty_display(&cond_ty),
                        },
                    );
                }
                env.check_body(post);
                env.check_body(body);
            }),
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Match {
                target,
                scrutinee,
                alts,
            } => {
                let scrutinee_ty = self.check_expr(scrutinee);
                self.expect_type(scrutinee.span, target, &scrutinee_ty);
                for alt in alts {
                    self.check_alt(target, alt);
                }
            }
            StmtKind::Assembly(stmts) => {
                if self.db.is_some() {
                    self.with_scope(|env| env.check_asm_block(stmts));
                } else {
                    self.push(stmt.span, CheckDiagnosticKind::AssemblyRequiresDatabase);
                }
            }
            StmtKind::Revert(_) | StmtKind::Comment(_) => {}
        }
    }

    fn check_alt(&mut self, target: &Ty<'db>, alt: &Alt<'db>) {
        let payload = match payload_type(target, &alt.pat) {
            Some(payload) => payload,
            None => {
                self.push(
                    alt.span,
                    CheckDiagnosticKind::BadMatchPattern {
                        pat: pat_display(&alt.pat),
                        ty: ty_display(target),
                    },
                );
                Ty::unit(alt.span)
            }
        };
        self.with_scope(|env| {
            env.insert_var(alt.binder.clone(), payload);
            env.check_body(&alt.body);
        });
    }

    fn check_expr(&mut self, expr: &Expr<'db>) -> Ty<'db> {
        let inferred = self.infer_expr(expr);
        if !type_eq(&expr.ty, &inferred) {
            self.push(
                expr.span,
                CheckDiagnosticKind::ExprAnnotationMismatch {
                    annotated: ty_display(&expr.ty),
                    inferred: ty_display(&inferred),
                },
            );
        }
        inferred
    }

    fn infer_expr(&mut self, expr: &Expr<'db>) -> Ty<'db> {
        match &expr.kind {
            ExprKind::Word(_) => Ty::word(expr.span),
            ExprKind::Bool(_) => Ty::bool(expr.span),
            ExprKind::Unit => Ty::unit(expr.span),
            ExprKind::Var(name) => self.lookup_var(name).unwrap_or_else(|| {
                self.push(
                    expr.span,
                    CheckDiagnosticKind::UndefinedVariable { name: name.clone() },
                );
                expr.ty.clone()
            }),
            ExprKind::Pair(lhs, rhs) => {
                let lhs_ty = self.check_expr(lhs);
                let rhs_ty = self.check_expr(rhs);
                Ty::product(expr.span, lhs_ty, rhs_ty)
            }
            ExprKind::Fst(inner) => {
                let actual = self.check_expr(inner);
                match actual.strip_named().kind.clone() {
                    TyKind::Product(lhs, _) => *lhs,
                    _ => {
                        self.push(
                            inner.span,
                            CheckDiagnosticKind::ExpectedProduct {
                                actual: ty_display(&actual),
                            },
                        );
                        expr.ty.clone()
                    }
                }
            }
            ExprKind::Snd(inner) => {
                let actual = self.check_expr(inner);
                match actual.strip_named().kind.clone() {
                    TyKind::Product(_, rhs) => *rhs,
                    _ => {
                        self.push(
                            inner.span,
                            CheckDiagnosticKind::ExpectedProduct {
                                actual: ty_display(&actual),
                            },
                        );
                        expr.ty.clone()
                    }
                }
            }
            ExprKind::Inl { target, value } => {
                match target.strip_named().kind.clone() {
                    TyKind::Sum(lhs, _) => {
                        let actual = self.check_expr(value);
                        self.expect_type(value.span, &lhs, &actual);
                    }
                    _ => self.push(
                        target.span,
                        CheckDiagnosticKind::ExpectedSum {
                            actual: ty_display(target),
                        },
                    ),
                }
                target.clone()
            }
            ExprKind::Inr { target, value } => {
                match target.strip_named().kind.clone() {
                    TyKind::Sum(_, rhs) => {
                        let actual = self.check_expr(value);
                        self.expect_type(value.span, &rhs, &actual);
                    }
                    _ => self.push(
                        target.span,
                        CheckDiagnosticKind::ExpectedSum {
                            actual: ty_display(target),
                        },
                    ),
                }
                target.clone()
            }
            ExprKind::InK {
                index,
                target,
                value,
            } => {
                match nth_sum_payload(target, *index) {
                    Some(expected) => {
                        let actual = self.check_expr(value);
                        self.expect_type(value.span, &expected, &actual);
                    }
                    None => self.push(
                        target.span,
                        CheckDiagnosticKind::BadInjectionIndex {
                            index: *index,
                            ty: ty_display(target),
                        },
                    ),
                }
                target.clone()
            }
            ExprKind::Call { callee, args } => {
                let Some(sig) = self.funs.get(callee).cloned() else {
                    self.push(
                        expr.span,
                        CheckDiagnosticKind::UndefinedFunction {
                            name: callee.clone(),
                        },
                    );
                    return expr.ty.clone();
                };
                if sig.args.len() != args.len() {
                    self.push(
                        expr.span,
                        CheckDiagnosticKind::ArityMismatch {
                            name: callee.clone(),
                            expected: sig.args.len(),
                            actual: args.len(),
                        },
                    );
                    return sig.ret;
                }
                for (expected, arg) in sig.args.iter().zip(args) {
                    let actual = self.check_expr(arg);
                    self.expect_type(arg.span, expected, &actual);
                }
                sig.ret
            }
            ExprKind::If {
                target,
                cond,
                then_expr,
                else_expr,
            } => {
                let cond_ty = self.check_expr(cond);
                if !is_bool_like(&cond_ty) {
                    self.push(
                        cond.span,
                        CheckDiagnosticKind::ExpectedBool {
                            actual: ty_display(&cond_ty),
                        },
                    );
                }
                let then_ty = self.check_expr(then_expr);
                let else_ty = self.check_expr(else_expr);
                self.expect_type(then_expr.span, target, &then_ty);
                self.expect_type(else_expr.span, target, &else_ty);
                target.clone()
            }
        }
    }

    fn check_asm_block(&mut self, stmts: &[YulStmt<'db>]) {
        for stmt in stmts {
            self.check_asm_stmt(stmt);
        }
    }

    fn check_asm_stmt(&mut self, stmt: &YulStmt<'db>) {
        match &stmt.kind {
            YulStmtKind::Block(stmts) => self.with_scope(|env| env.check_asm_block(stmts)),
            YulStmtKind::Let { names, init } => {
                if let Some(init) = init {
                    let ty = self.check_asm_expr(init);
                    let expected = names.len();
                    let actual = return_count(&ty);
                    if actual != expected {
                        self.push(
                            init.span,
                            CheckDiagnosticKind::AssemblyReturnCountMismatch {
                                context: "let binding".to_owned(),
                                expected,
                                actual,
                            },
                        );
                    }
                }
                for name in names {
                    self.insert_var(self.yul_name(name), Ty::word(stmt.span));
                }
            }
            YulStmtKind::Assign { names, value } => {
                let mut expected = 0usize;
                for name in names {
                    let name_text = self.yul_name(name);
                    match self.lookup_var(&name_text) {
                        Some(ty) => {
                            if !is_word_type(&ty) {
                                self.push(
                                    stmt.span,
                                    CheckDiagnosticKind::AssemblyExpectedWordAssignment {
                                        name: name_text,
                                        actual: ty_display(&ty),
                                    },
                                );
                            }
                        }
                        None => self.push(
                            stmt.span,
                            CheckDiagnosticKind::UndefinedVariable { name: name_text },
                        ),
                    }
                    expected += 1;
                }
                let actual_ty = self.check_asm_expr(value);
                let actual = return_count(&actual_ty);
                if actual != expected {
                    self.push(
                        value.span,
                        CheckDiagnosticKind::AssemblyReturnCountMismatch {
                            context: "assignment".to_owned(),
                            expected,
                            actual,
                        },
                    );
                }
            }
            YulStmtKind::Expr(expr) => {
                let ty = self.check_asm_expr(expr);
                if !type_eq(&ty, &Ty::unit(expr.span)) {
                    self.push(
                        expr.span,
                        CheckDiagnosticKind::AssemblyExpressionNotUnit {
                            actual: ty_display(&ty),
                        },
                    );
                }
            }
            YulStmtKind::If { cond, body } => {
                self.check_asm_arg(cond);
                self.check_asm_block(body);
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => self.with_scope(|env| {
                env.check_asm_block(init);
                env.check_asm_arg(cond);
                env.check_asm_block(post);
                env.check_asm_block(body);
            }),
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                self.check_asm_arg(expr);
                for case in cases {
                    self.check_asm_block(&case.body);
                }
                if let Some(default) = default {
                    self.check_asm_block(default);
                }
            }
            YulStmtKind::FunctionDef {
                name,
                params,
                rets,
                body,
            } => {
                let fun_name = self.yul_name(name);
                self.funs.insert(
                    fun_name,
                    FunSig {
                        args: vec![Ty::word(stmt.span); params.len()],
                        ret: n_returns(stmt.span, rets.len()),
                    },
                );
                self.with_scope(|env| {
                    for param in params {
                        env.insert_var(env.yul_name(param), Ty::word(stmt.span));
                    }
                    for ret in rets {
                        env.insert_var(env.yul_name(ret), Ty::word(stmt.span));
                    }
                    env.check_asm_block(body);
                });
            }
            YulStmtKind::Leave
            | YulStmtKind::Break
            | YulStmtKind::Continue
            | YulStmtKind::Error => {}
        }
    }

    fn check_asm_expr(&mut self, expr: &YulExpr<'db>) -> Ty<'db> {
        match &expr.kind {
            YulExprKind::Lit(_) => Ty::word(expr.span),
            YulExprKind::Ident(name) => {
                let name = self.yul_name(name);
                self.lookup_var(&name).unwrap_or_else(|| {
                    self.push(expr.span, CheckDiagnosticKind::UndefinedVariable { name });
                    Ty::word(expr.span)
                })
            }
            YulExprKind::Call { name, args } => {
                let name = self.yul_name(name);
                let sig = self.lookup_asm_fun(expr.span, &name);
                if sig.args.len() != args.len() {
                    self.push(
                        expr.span,
                        CheckDiagnosticKind::ArityMismatch {
                            name,
                            expected: sig.args.len(),
                            actual: args.len(),
                        },
                    );
                    return sig.ret;
                }
                for arg in args {
                    self.check_asm_arg(arg);
                }
                sig.ret
            }
            YulExprKind::Error => Ty::word(expr.span),
        }
    }

    fn check_asm_arg(&mut self, expr: &YulExpr<'db>) {
        let ty = self.check_asm_expr(expr);
        if is_word_type(&ty) {
            return;
        }
        if type_eq(&ty, &Ty::unit(expr.span)) {
            self.push(expr.span, CheckDiagnosticKind::AssemblyVoidArgument);
        } else {
            self.push(
                expr.span,
                CheckDiagnosticKind::AssemblyExpectedWordArgument {
                    actual: ty_display(&ty),
                },
            );
        }
    }

    fn lookup_asm_fun(&mut self, span: Span<'db>, name: &str) -> FunSig<'db> {
        if let Some(sig) = asm_builtin_sig(span, name) {
            return sig;
        }
        let key = name.strip_prefix("usr$").unwrap_or(name);
        match self.funs.get(key).cloned() {
            Some(sig) => FunSig {
                args: vec![Ty::word(span); sig.args.len()],
                ret: n_returns(span, return_count(&sig.ret)),
            },
            None => {
                self.push(
                    span,
                    CheckDiagnosticKind::UndefinedFunction {
                        name: name.to_owned(),
                    },
                );
                FunSig {
                    args: Vec::new(),
                    ret: Ty::unit(span),
                }
            }
        }
    }

    fn yul_name(&self, name: &SpannedElem<'db, Ident<'db>>) -> String {
        if let Some(db) = self.db {
            (*name.atom()).text(db).to_owned()
        } else {
            "<unknown>".to_owned()
        }
    }

    fn expect_type(&mut self, span: Span<'db>, expected: &Ty<'db>, actual: &Ty<'db>) {
        if !type_eq(expected, actual) {
            self.push(
                span,
                CheckDiagnosticKind::TypeMismatch {
                    expected: ty_display(expected),
                    actual: ty_display(actual),
                },
            );
        }
    }

    fn insert_var(&mut self, name: String, ty: Ty<'db>) {
        self.vars.last_mut().insert(name, ty);
    }

    fn lookup_var(&self, name: &str) -> Option<Ty<'db>> {
        self.vars
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn with_scope(&mut self, f: impl FnOnce(&mut Self)) {
        self.vars.push(BTreeMap::new());
        f(self);
        let _ = self.vars.pop();
    }

    fn push(&mut self, span: Span<'db>, kind: CheckDiagnosticKind) {
        self.diagnostics.push(CheckDiagnostic { span, kind });
    }
}

fn builtin_funs<'db>(span: Span<'db>) -> BTreeMap<String, FunSig<'db>> {
    let word = Ty::word(span);
    let unit = Ty::unit(span);
    let bool_sum = bool_sum_ty(span);
    let mut funs = BTreeMap::new();
    let mut add = |name: &str, args: Vec<Ty<'db>>, ret: Ty<'db>| {
        funs.insert(name.to_owned(), FunSig { args, ret });
    };
    for name in [
        "add",
        "sub",
        "mul",
        "div",
        "sdiv",
        "mod",
        "smod",
        "exp",
        "signextend",
        "and",
        "or",
        "xor",
        "byte",
        "shl",
        "shr",
        "sar",
        "keccak256",
        "primAddWord",
        "subWord",
        "bxorWord",
        "bandWord",
        "borWord",
        "integerAdd",
        "integerSub",
        "integerMul",
        "wordFromInteger",
    ] {
        let argc = if name == "wordFromInteger" { 1 } else { 2 };
        add(name, vec![word.clone(); argc], word.clone());
    }
    for name in ["addmod", "mulmod"] {
        add(name, vec![word.clone(); 3], word.clone());
    }
    for name in [
        "mload",
        "sload",
        "tload",
        "calldataload",
        "memoryguard",
        "balance",
        "extcodesize",
        "extcodehash",
        "blockhash",
        "blobhash",
    ] {
        add(name, vec![word.clone()], word.clone());
    }
    for name in [
        "address",
        "origin",
        "caller",
        "callvalue",
        "calldatasize",
        "codesize",
        "gasprice",
        "returndatasize",
        "coinbase",
        "timestamp",
        "number",
        "prevrandao",
        "gaslimit",
        "chainid",
        "selfbalance",
        "basefee",
        "blobbasefee",
        "msize",
        "gas",
    ] {
        add(name, Vec::new(), word.clone());
    }
    for name in [
        "lt",
        "gt",
        "slt",
        "sgt",
        "eq",
        "primEqWord",
        "gtWord",
        "integerLt",
        "integerEq",
    ] {
        add(name, vec![word.clone(), word.clone()], bool_sum.clone());
    }
    add("iszero", vec![bool_sum.clone()], bool_sum.clone());
    for name in ["not", "clz", "wordToInteger"] {
        add(name, vec![word.clone()], word.clone());
    }
    for name in [
        "stop",
        "invalid",
        "mstore",
        "mstore8",
        "sstore",
        "tstore",
        "return",
        "revert",
        "pop",
        "selfdestruct",
        "calldatacopy",
        "codecopy",
        "returndatacopy",
        "mcopy",
        "datacopy",
    ] {
        let argc = match name {
            "stop" | "invalid" => 0,
            "pop" | "selfdestruct" => 1,
            "calldatacopy" | "codecopy" | "returndatacopy" | "mcopy" | "datacopy" => 3,
            _ => 2,
        };
        add(name, vec![word.clone(); argc], unit.clone());
    }
    add("extcodecopy", vec![word.clone(); 4], unit.clone());
    add("create", vec![word.clone(); 3], word.clone());
    add("create2", vec![word.clone(); 4], word.clone());
    add("call", vec![word.clone(); 7], word.clone());
    add("callcode", vec![word.clone(); 7], word.clone());
    add("delegatecall", vec![word.clone(); 6], word.clone());
    add("staticcall", vec![word.clone(); 6], word.clone());
    for index in 0..=4 {
        add(
            &format!("log{index}"),
            vec![word.clone(); 2 + index],
            unit.clone(),
        );
    }
    for name in ["dataoffset", "datasize", "loadimmutable", "linkersymbol"] {
        add(name, vec![word.clone()], word.clone());
    }
    add(
        "setimmutable",
        vec![word.clone(), word.clone(), word.clone()],
        unit.clone(),
    );
    funs
}

fn asm_builtin_sig<'db>(span: Span<'db>, name: &str) -> Option<FunSig<'db>> {
    let word = Ty::word(span);
    let unit = Ty::unit(span);
    let sig = |args: usize, ret: Ty<'db>| FunSig {
        args: vec![word.clone(); args],
        ret,
    };
    let fun = match name {
        "stop" | "invalid" => sig(0, unit.clone()),
        "add" | "sub" | "mul" | "div" | "sdiv" | "mod" | "smod" | "exp" | "signextend" | "lt"
        | "gt" | "slt" | "sgt" | "eq" | "and" | "or" | "xor" | "byte" | "shl" | "shr" | "sar"
        | "keccak256" => sig(2, word.clone()),
        "addmod" | "mulmod" => sig(3, word.clone()),
        "iszero" | "not" | "clz" | "balance" | "calldataload" | "extcodesize" | "extcodehash"
        | "blockhash" | "blobhash" | "mload" | "sload" | "tload" => sig(1, word.clone()),
        "pop" | "selfdestruct" => sig(1, unit.clone()),
        "address" | "origin" | "caller" | "callvalue" | "calldatasize" | "codesize"
        | "gasprice" | "returndatasize" | "coinbase" | "timestamp" | "number" | "prevrandao"
        | "gaslimit" | "chainid" | "selfbalance" | "basefee" | "blobbasefee" | "msize" | "gas" => {
            sig(0, word.clone())
        }
        "mstore" | "mstore8" | "sstore" | "tstore" | "return" | "revert" => sig(2, unit.clone()),
        "calldatacopy" | "codecopy" | "returndatacopy" | "mcopy" | "datacopy" => {
            sig(3, unit.clone())
        }
        "extcodecopy" => sig(4, unit.clone()),
        "create" => sig(3, word.clone()),
        "create2" => sig(4, word.clone()),
        "call" | "callcode" => sig(7, word.clone()),
        "delegatecall" | "staticcall" => sig(6, word.clone()),
        "log0" => sig(2, unit.clone()),
        "log1" => sig(3, unit.clone()),
        "log2" => sig(4, unit.clone()),
        "log3" => sig(5, unit.clone()),
        "log4" => sig(6, unit.clone()),
        "memoryguard" | "dataoffset" | "datasize" | "loadimmutable" | "linkersymbol" => {
            sig(1, word.clone())
        }
        "setimmutable" => sig(3, unit.clone()),
        _ => return None,
    };
    Some(fun)
}

fn payload_type<'db>(target: &Ty<'db>, pat: &Pat<'db>) -> Option<Ty<'db>> {
    match (&target.strip_named().kind, &pat.kind) {
        (TyKind::Sum(lhs, _), PatKind::Con(Con::Inl)) => Some((**lhs).clone()),
        (TyKind::Sum(_, rhs), PatKind::Con(Con::Inr)) => Some((**rhs).clone()),
        (_, PatKind::Con(Con::InK(index))) => nth_sum_payload(target, *index),
        (_, PatKind::Wildcard | PatKind::Var(_)) => Some(target.clone()),
        (TyKind::Word, PatKind::IntLit(_)) => Some(Ty::word(pat.span)),
        _ => None,
    }
}

fn nth_sum_payload<'db>(target: &Ty<'db>, index: usize) -> Option<Ty<'db>> {
    let mut current = target.strip_named();
    let mut remaining = index;
    loop {
        match &current.strip_named().kind {
            TyKind::Sum(lhs, rhs) if remaining == 0 => return Some((**lhs).clone()),
            TyKind::Sum(_, rhs) => {
                current = rhs.strip_named();
                remaining -= 1;
            }
            _ if remaining == 0 => return Some(current.clone()),
            _ => return None,
        }
    }
}

fn bool_sum_ty<'db>(span: Span<'db>) -> Ty<'db> {
    Ty::sum(span, Ty::unit(span), Ty::unit(span))
}

fn is_bool_like(ty: &Ty<'_>) -> bool {
    matches!(ty.strip_named().kind, TyKind::Bool)
        || matches!(
            &ty.strip_named().kind,
            TyKind::Sum(lhs, rhs)
                if matches!(lhs.strip_named().kind, TyKind::Unit)
                    && matches!(rhs.strip_named().kind, TyKind::Unit)
        )
}

fn is_word_type(ty: &Ty<'_>) -> bool {
    matches!(ty.strip_named().kind, TyKind::Word)
}

fn return_count(ty: &Ty<'_>) -> usize {
    match &ty.strip_named().kind {
        TyKind::Unit => 0,
        TyKind::Word | TyKind::Bool => 1,
        TyKind::Product(lhs, rhs) => return_count(lhs) + return_count(rhs),
        TyKind::Sum(lhs, rhs) => 1 + return_count(lhs).max(return_count(rhs)),
        TyKind::Named { inner, .. } => return_count(inner),
        TyKind::NamedRef { .. } => 1,
        TyKind::Function { .. } => 1,
    }
}

fn n_returns<'db>(span: Span<'db>, count: usize) -> Ty<'db> {
    match count {
        0 => Ty::unit(span),
        1 => Ty::word(span),
        _ => Ty::product(span, Ty::word(span), n_returns(span, count - 1)),
    }
}

fn requires_terminator(ty: &Ty<'_>) -> bool {
    return_count(ty) > 0
}

fn type_eq(lhs: &Ty<'_>, rhs: &Ty<'_>) -> bool {
    match (&lhs.kind, &rhs.kind) {
        (TyKind::NamedRef { name: a }, TyKind::NamedRef { name: b })
        | (TyKind::NamedRef { name: a }, TyKind::Named { name: b, .. })
        | (TyKind::Named { name: a, .. }, TyKind::NamedRef { name: b }) => {
            return a == b;
        }
        _ => {}
    }

    match (&lhs.strip_named().kind, &rhs.strip_named().kind) {
        (TyKind::Word, TyKind::Word)
        | (TyKind::Bool, TyKind::Bool)
        | (TyKind::Unit, TyKind::Unit) => true,
        (TyKind::Product(a_lhs, a_rhs), TyKind::Product(b_lhs, b_rhs))
        | (TyKind::Sum(a_lhs, a_rhs), TyKind::Sum(b_lhs, b_rhs)) => {
            type_eq(a_lhs, b_lhs) && type_eq(a_rhs, b_rhs)
        }
        (
            TyKind::Function {
                params: a_params,
                ret: a_ret,
            },
            TyKind::Function {
                params: b_params,
                ret: b_ret,
            },
        ) => {
            a_params.len() == b_params.len()
                && a_params
                    .iter()
                    .zip(b_params)
                    .all(|(lhs, rhs)| type_eq(lhs, rhs))
                && type_eq(a_ret, b_ret)
        }
        _ => false,
    }
}

fn body_terminates(body: &[Stmt<'_>], db: Option<&dyn HirDb>) -> bool {
    body.last().is_some_and(|stmt| stmt_terminates(stmt, db))
}

fn stmt_terminates(stmt: &Stmt<'_>, db: Option<&dyn HirDb>) -> bool {
    match &stmt.kind {
        StmtKind::Return(_) | StmtKind::Revert(_) => true,
        StmtKind::Block(body) => body_terminates(body, db),
        StmtKind::Match { alts, .. } => {
            !alts.is_empty() && alts.iter().all(|alt| body_terminates(&alt.body, db))
        }
        StmtKind::Assembly(stmts) => asm_block_terminates(stmts, db),
        StmtKind::Let { .. }
        | StmtKind::Assign { .. }
        | StmtKind::Expr(_)
        | StmtKind::For { .. }
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Comment(_) => false,
    }
}

fn asm_block_terminates(stmts: &[YulStmt<'_>], db: Option<&dyn HirDb>) -> bool {
    stmts
        .last()
        .is_some_and(|stmt| asm_stmt_terminates(stmt, db))
}

fn asm_stmt_terminates(stmt: &YulStmt<'_>, db: Option<&dyn HirDb>) -> bool {
    match &stmt.kind {
        YulStmtKind::Block(stmts) => asm_block_terminates(stmts, db),
        YulStmtKind::Expr(YulExpr {
            kind: YulExprKind::Call { name, .. },
            ..
        }) => db
            .map(|db| {
                let name = (*name.atom()).text(db);
                matches!(name, "return" | "revert")
            })
            .unwrap_or(false),
        YulStmtKind::Switch { cases, default, .. } => {
            !cases.is_empty()
                && default.is_some()
                && cases
                    .iter()
                    .all(|case| asm_block_terminates(&case.body, db))
                && default
                    .as_ref()
                    .is_some_and(|body| asm_block_terminates(body, db))
        }
        _ => false,
    }
}

fn ty_display(ty: &Ty<'_>) -> String {
    match &ty.kind {
        TyKind::Word => "word".to_owned(),
        TyKind::Bool => "bool".to_owned(),
        TyKind::Unit => "unit".to_owned(),
        TyKind::Product(lhs, rhs) => format!("({} * {})", ty_display(lhs), ty_display(rhs)),
        TyKind::Sum(lhs, rhs) => format!("({} + {})", ty_display(lhs), ty_display(rhs)),
        TyKind::Named { name, inner } => format!("{name}{{{}}}", ty_display(inner)),
        TyKind::NamedRef { name } => name.clone(),
        TyKind::Function { params, ret } => {
            let params = params.iter().map(ty_display).collect::<Vec<_>>().join(", ");
            format!("({params} -> {})", ty_display(ret))
        }
    }
}

fn pat_display(pat: &Pat<'_>) -> String {
    match &pat.kind {
        PatKind::Var(name) => name.clone(),
        PatKind::Con(Con::Inl) => "inl".to_owned(),
        PatKind::Con(Con::Inr) => "inr".to_owned(),
        PatKind::Con(Con::InK(index)) => format!("in({index})"),
        PatKind::Wildcard => "_".to_owned(),
        PatKind::IntLit(value) => value.clone(),
    }
}
