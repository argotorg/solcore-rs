use std::collections::BTreeMap;

use hir::span::Span;

use crate::ir::{
    Alt, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt, StmtKind, Ty, TyKind,
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
}

#[derive(Debug, Clone)]
struct FunSig<'db> {
    args: Vec<Ty<'db>>,
    ret: Ty<'db>,
}

#[derive(Debug, Default)]
struct Env<'db> {
    vars: Vec<BTreeMap<String, Ty<'db>>>,
    funs: BTreeMap<String, FunSig<'db>>,
    ret: Option<Ty<'db>>,
    diagnostics: Vec<CheckDiagnostic<'db>>,
}

pub fn check_program<'db>(program: &Program<'db>) -> Vec<CheckDiagnostic<'db>> {
    let mut env = Env {
        vars: vec![BTreeMap::new()],
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
        for inner in &object.inners {
            self.check_object(inner);
        }
        self.funs = saved_funs;
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
            if !body_terminates(&function.body) {
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
                env.check_expr(cond);
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
            StmtKind::Assembly(_) | StmtKind::Revert(_) | StmtKind::Comment(_) => {}
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
            ExprKind::Fst(inner) => match self.check_expr(inner).strip_named().kind.clone() {
                TyKind::Product(lhs, _) => *lhs,
                _ => {
                    let actual = self.check_expr(inner);
                    self.push(
                        inner.span,
                        CheckDiagnosticKind::ExpectedProduct {
                            actual: ty_display(&actual),
                        },
                    );
                    expr.ty.clone()
                }
            },
            ExprKind::Snd(inner) => match self.check_expr(inner).strip_named().kind.clone() {
                TyKind::Product(_, rhs) => *rhs,
                _ => {
                    let actual = self.check_expr(inner);
                    self.push(
                        inner.span,
                        CheckDiagnosticKind::ExpectedProduct {
                            actual: ty_display(&actual),
                        },
                    );
                    expr.ty.clone()
                }
            },
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
        self.vars
            .last_mut()
            .expect("scope stack is never empty")
            .insert(name, ty);
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
        self.vars.pop();
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
        "calldataload",
        "memoryguard",
        "datasize",
        "dataoffset",
    ] {
        add(name, vec![word.clone()], word.clone());
    }
    for name in ["calldatasize", "callvalue", "caller", "codesize"] {
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
    for name in ["iszero", "not", "clz", "wordToInteger"] {
        add(name, vec![word.clone()], word.clone());
    }
    for name in [
        "stop", "invalid", "mstore", "mstore8", "sstore", "tstore", "return", "revert", "pop",
        "codecopy",
    ] {
        let argc = match name {
            "stop" | "invalid" => 0,
            "pop" => 1,
            "codecopy" => 3,
            _ => 2,
        };
        add(name, vec![word.clone(); argc], unit.clone());
    }
    funs
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

fn type_eq(lhs: &Ty<'_>, rhs: &Ty<'_>) -> bool {
    match (&lhs.kind, &rhs.kind) {
        (TyKind::NamedRef { name: lhs }, TyKind::NamedRef { name: rhs }) => return lhs == rhs,
        (TyKind::NamedRef { name: lhs }, TyKind::Named { name: rhs, .. })
        | (TyKind::Named { name: lhs, .. }, TyKind::NamedRef { name: rhs }) => {
            return lhs == rhs;
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
        (TyKind::NamedRef { name: lhs }, TyKind::NamedRef { name: rhs }) => lhs == rhs,
        (TyKind::NamedRef { name: lhs }, TyKind::Named { name: rhs, .. })
        | (TyKind::Named { name: lhs, .. }, TyKind::NamedRef { name: rhs }) => lhs == rhs,
        _ => false,
    }
}

fn body_terminates(body: &[Stmt<'_>]) -> bool {
    body.last().is_some_and(stmt_terminates)
}

fn stmt_terminates(stmt: &Stmt<'_>) -> bool {
    match &stmt.kind {
        StmtKind::Return(_) | StmtKind::Revert(_) => true,
        StmtKind::Block(body) => body_terminates(body),
        StmtKind::Match { alts, .. } => {
            !alts.is_empty() && alts.iter().all(|alt| body_terminates(&alt.body))
        }
        StmtKind::Let { .. }
        | StmtKind::Assign { .. }
        | StmtKind::Expr(_)
        | StmtKind::For { .. }
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Assembly(_)
        | StmtKind::Comment(_) => false,
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
