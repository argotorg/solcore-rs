use std::collections::BTreeMap;

use hir::ast::function::{
    YulCase as HirYulCase, YulExpr as HirYulExpr, YulExprKind, YulStmt as HirYulStmt, YulStmtKind,
};

use super::{
    TranslationError, Translator,
    location::{flatten_lhs, flatten_rhs},
    names::{convert_yul_lit, is_yul_builtin, yul_fun_name, yul_name},
};
use crate::ast::{Case, Expr, FunctionName, Stmt, VarName};

#[derive(Debug, Clone)]
pub(super) struct AsmScopes {
    values: Vec<BTreeMap<String, VarName>>,
    functions: Vec<BTreeMap<String, FunctionName>>,
    outer_values_visible: bool,
}

impl<'db> Translator<'db> {
    pub(super) fn convert_yul_stmts(
        &mut self,
        stmts: &[HirYulStmt<'db>],
        asm: &mut AsmScopes,
    ) -> Result<Vec<Stmt>, TranslationError> {
        self.predeclare_yul_functions(stmts, asm)?;
        let mut out = Vec::new();
        for stmt in stmts {
            // A definition has no origin of its own, as in the Sonatina route.
            out.extend(if matches!(stmt.kind, YulStmtKind::FunctionDef { .. }) {
                self.no_src_loc()
            } else {
                self.src_loc(stmt.span)?
            });
            out.push(self.convert_yul_stmt(stmt, asm)?);
        }
        Ok(out)
    }

    fn predeclare_yul_functions(
        &mut self,
        stmts: &[HirYulStmt<'db>],
        asm: &mut AsmScopes,
    ) -> Result<(), TranslationError> {
        for stmt in stmts {
            let YulStmtKind::FunctionDef { name, .. } = &stmt.kind else {
                continue;
            };
            let raw = yul_name(self.db, name);
            if asm.current_function(&raw).is_some() {
                return Err(TranslationError::new(format!(
                    "duplicate inline Yul function `{raw}` in the same block"
                )));
            }
            let emitted = self.fresh_asm_name(&raw);
            asm.insert_function(raw, FunctionName::new(emitted.as_str()));
        }
        Ok(())
    }

    fn convert_yul_stmt(
        &mut self,
        stmt: &HirYulStmt<'db>,
        asm: &mut AsmScopes,
    ) -> Result<Stmt, TranslationError> {
        match &stmt.kind {
            YulStmtKind::Block(stmts) => {
                asm.push_scope();
                let body = self.convert_yul_stmts(stmts, asm);
                asm.pop_scope();
                Ok(Stmt::Block(body?))
            }
            YulStmtKind::Let { names, init } => {
                let init = init
                    .as_ref()
                    .map(|expr| self.convert_yul_expr(expr, asm))
                    .transpose()?;
                let names = names
                    .iter()
                    .map(|name| {
                        let raw = yul_name(self.db, name);
                        let emitted = self.fresh_asm_name(&raw);
                        asm.insert_value(raw, emitted.clone());
                        emitted
                    })
                    .collect();
                Ok(Stmt::Let { names, init })
            }
            YulStmtKind::Assign { names, value } => {
                let names = names
                    .iter()
                    .map(|name| {
                        let raw = yul_name(self.db, name);
                        match asm.lookup_value(&raw) {
                            Some(name) => name,
                            None if asm.outer_values_visible => self.subst_asm_lhs_name(&raw),
                            None => raw.into(),
                        }
                    })
                    .collect();
                Ok(Stmt::Assign {
                    names,
                    value: self.convert_yul_expr(value, asm)?,
                })
            }
            YulStmtKind::Expr(expr) => Ok(Stmt::Expr(self.convert_yul_expr(expr, asm)?)),
            YulStmtKind::If { cond, body } => {
                asm.push_scope();
                let body = self.convert_yul_stmts(body, asm);
                asm.pop_scope();
                Ok(Stmt::If {
                    cond: self.convert_yul_expr(cond, asm)?,
                    body: body?,
                })
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                asm.push_scope();
                let mut init = self.convert_yul_stmts(init, asm)?;
                // The condition belongs to the loop, not to the last statement
                // of `init`.
                if !init.is_empty() {
                    init.extend(self.src_loc(stmt.span)?);
                }
                let cond = self.convert_yul_expr(cond, asm)?;

                asm.push_scope();
                let post = self.convert_yul_stmts(post, asm);
                asm.pop_scope();

                asm.push_scope();
                let body = self.convert_yul_stmts(body, asm);
                asm.pop_scope();
                asm.pop_scope();

                Ok(Stmt::For {
                    init,
                    cond,
                    post: post?,
                    body: body?,
                })
            }
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => Ok(Stmt::Switch {
                expr: self.convert_yul_expr(expr, asm)?,
                cases: cases
                    .iter()
                    .map(|case| self.convert_yul_case(case, asm))
                    .collect::<Result<Vec<_>, _>>()?,
                default: default
                    .as_ref()
                    .map(|body| {
                        asm.push_scope();
                        let converted = self.convert_yul_stmts(body, asm);
                        asm.pop_scope();
                        converted
                    })
                    .transpose()?,
            }),
            YulStmtKind::FunctionDef {
                name,
                params,
                rets,
                body,
            } => {
                let raw_name = yul_name(self.db, name);
                let name = asm.lookup_function(&raw_name).ok_or_else(|| {
                    TranslationError::new(format!(
                        "missing predeclared inline Yul function `{raw_name}`"
                    ))
                })?;

                let mut function_asm = asm.function_body_scope();
                let params = params
                    .iter()
                    .map(|param| {
                        let raw = yul_name(self.db, param);
                        let emitted = self.fresh_asm_name(&raw);
                        function_asm.insert_value(raw, emitted.clone());
                        emitted
                    })
                    .collect();
                let returns = rets
                    .iter()
                    .map(|ret| {
                        let raw = yul_name(self.db, ret);
                        let emitted = self.fresh_asm_name(&raw);
                        function_asm.insert_value(raw, emitted.clone());
                        emitted
                    })
                    .collect();
                let body = self.convert_yul_stmts(body, &mut function_asm);

                Ok(Stmt::Function {
                    name,
                    params,
                    returns,
                    body: body?,
                })
            }
            YulStmtKind::Leave => Ok(Stmt::Leave),
            YulStmtKind::Break => Ok(Stmt::Break),
            YulStmtKind::Continue => Ok(Stmt::Continue),
            YulStmtKind::Error => Ok(Stmt::Comment("error".to_owned())),
        }
    }

    fn convert_yul_case(
        &mut self,
        case: &HirYulCase<'db>,
        asm: &mut AsmScopes,
    ) -> Result<Case, TranslationError> {
        asm.push_scope();
        let body = self.convert_yul_stmts(&case.body, asm);
        asm.pop_scope();
        Ok(Case {
            lit: convert_yul_lit(&case.lit)?,
            body: body?,
        })
    }

    fn convert_yul_expr(
        &self,
        expr: &HirYulExpr<'db>,
        asm: &AsmScopes,
    ) -> Result<Expr, TranslationError> {
        Ok(match &expr.kind {
            YulExprKind::Lit(lit) => Expr::Lit(convert_yul_lit(lit)?),
            YulExprKind::Ident(name) => {
                let name = yul_name(self.db, name);
                match asm.lookup_value(&name) {
                    Some(name) => Expr::ident(name),
                    None if asm.outer_values_visible => self.subst_asm_expr_name(&name),
                    None => Expr::ident(name),
                }
            }
            YulExprKind::Call { name, args } => {
                let raw_name = yul_name(self.db, name);
                // Match Hull's assembly resolution: explicit Hull names and
                // builtins take precedence, followed by lexically scoped Yul
                // functions and then unqualified Hull functions.
                let name = if raw_name.starts_with("usr$") || is_yul_builtin(&raw_name) {
                    raw_name.into()
                } else {
                    asm.lookup_function(&raw_name)
                        .or_else(|| {
                            self.user_functions
                                .contains(&raw_name)
                                .then(|| yul_fun_name(&raw_name))
                        })
                        .unwrap_or_else(|| raw_name.into())
                };
                Expr::call(
                    name,
                    args.iter()
                        .map(|arg| self.convert_yul_expr(arg, asm))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            YulExprKind::Error => Expr::ident("error"),
        })
    }

    fn subst_asm_expr_name(&self, name: &str) -> Expr {
        match self.lookup_var_opt(name).and_then(|loc| {
            let flattened = flatten_rhs(&loc);
            match flattened.as_slice() {
                [expr] => Some(expr.clone()),
                _ => None,
            }
        }) {
            Some(expr) => expr,
            None => Expr::ident(name),
        }
    }

    fn subst_asm_lhs_name(&self, name: &str) -> VarName {
        match self.lookup_var_opt(name).and_then(|loc| {
            let flattened = flatten_lhs(&loc).ok()?;
            match flattened.as_slice() {
                [name] => Some(name.clone()),
                _ => None,
            }
        }) {
            Some(name) => name,
            None => name.into(),
        }
    }
}

impl AsmScopes {
    pub(super) fn new() -> Self {
        Self {
            values: vec![BTreeMap::new()],
            functions: vec![BTreeMap::new()],
            outer_values_visible: true,
        }
    }

    fn push_scope(&mut self) {
        self.values.push(BTreeMap::new());
        self.functions.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.values.pop().expect("assembly value scope");
        self.functions.pop().expect("assembly function scope");
    }

    fn insert_value(&mut self, source: String, emitted: VarName) {
        self.values
            .last_mut()
            .expect("assembly value scope")
            .insert(source, emitted);
    }

    fn insert_function(&mut self, source: String, emitted: FunctionName) {
        self.functions
            .last_mut()
            .expect("assembly function scope")
            .insert(source, emitted);
    }

    fn current_function(&self, name: &str) -> Option<FunctionName> {
        self.functions
            .last()
            .and_then(|scope| scope.get(name).cloned())
    }

    fn lookup_value(&self, name: &str) -> Option<VarName> {
        self.values
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn lookup_function(&self, name: &str) -> Option<FunctionName> {
        self.functions
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn function_body_scope(&self) -> Self {
        let mut functions = self.functions.clone();
        functions.push(BTreeMap::new());
        Self {
            values: vec![BTreeMap::new()],
            functions,
            outer_values_visible: false,
        }
    }
}
