use std::collections::BTreeMap;

use hir::ast::function::{
    YulCase as HirYulCase, YulExpr as HirYulExpr, YulExprKind, YulStmt as HirYulStmt, YulStmtKind,
};

use crate::ast::{Case, Expr, Stmt};

use super::{
    TranslationError, Translator,
    location::{flatten_lhs, flatten_rhs},
    names::{convert_yul_lit, yul_name},
};

#[derive(Debug, Clone)]
pub(super) struct AsmScopes {
    values: Vec<BTreeMap<String, String>>,
    functions: Vec<BTreeMap<String, String>>,
}

impl<'db> Translator<'db> {
    pub(super) fn convert_yul_stmts(
        &mut self,
        stmts: &[HirYulStmt<'db>],
        asm: &mut AsmScopes,
    ) -> Result<Vec<Stmt>, TranslationError> {
        stmts
            .iter()
            .map(|stmt| self.convert_yul_stmt(stmt, asm))
            .collect()
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
                        asm.lookup_value(&raw)
                            .unwrap_or_else(|| self.subst_asm_lhs_name(&raw))
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
                let init = self.convert_yul_stmts(init, asm)?;
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
                let name = self.fresh_asm_name(&raw_name);
                asm.insert_function(raw_name, name.clone());

                asm.push_scope();
                let params = params
                    .iter()
                    .map(|param| {
                        let raw = yul_name(self.db, param);
                        let emitted = self.fresh_asm_name(&raw);
                        asm.insert_value(raw, emitted.clone());
                        emitted
                    })
                    .collect();
                let returns = rets
                    .iter()
                    .map(|ret| {
                        let raw = yul_name(self.db, ret);
                        let emitted = self.fresh_asm_name(&raw);
                        asm.insert_value(raw, emitted.clone());
                        emitted
                    })
                    .collect();
                let body = self.convert_yul_stmts(body, asm);
                asm.pop_scope();

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
                    None => self.subst_asm_expr_name(&name),
                }
            }
            YulExprKind::Call { name, args } => {
                let raw_name = yul_name(self.db, name);
                let name = asm.lookup_function(&raw_name).unwrap_or(raw_name);
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

    fn subst_asm_lhs_name(&self, name: &str) -> String {
        match self.lookup_var_opt(name).and_then(|loc| {
            let flattened = flatten_lhs(&loc).ok()?;
            match flattened.as_slice() {
                [name] => Some(name.clone()),
                _ => None,
            }
        }) {
            Some(name) => name,
            None => name.to_owned(),
        }
    }
}

impl AsmScopes {
    pub(super) fn new() -> Self {
        Self {
            values: vec![BTreeMap::new()],
            functions: vec![BTreeMap::new()],
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

    fn insert_value(&mut self, source: String, emitted: String) {
        self.values
            .last_mut()
            .expect("assembly value scope")
            .insert(source, emitted);
    }

    fn insert_function(&mut self, source: String, emitted: String) {
        self.functions
            .last_mut()
            .expect("assembly function scope")
            .insert(source, emitted);
    }

    fn lookup_value(&self, name: &str) -> Option<String> {
        self.values
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn lookup_function(&self, name: &str) -> Option<String> {
        self.functions
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}
