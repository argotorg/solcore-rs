use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct YulFunctionSig<'db> {
    params: Vec<InferTy<'db>>,
    ret: InferTy<'db>,
}

#[derive(Debug, Clone, Default)]
struct YulScope<'db> {
    values: FxHashSet<String>,
    functions: FxHashMap<String, YulFunctionSig<'db>>,
}

impl<'db> InferCtx<'db> {
    pub(super) fn infer_yul_block(&mut self, body: &[YulStmt<'db>]) -> (Vec<String>, InferTy<'db>) {
        let mut scopes = vec![YulScope::default()];
        self.infer_yul_block_scoped(body, &mut scopes)
    }

    fn infer_yul_block_scoped(
        &mut self,
        body: &[YulStmt<'db>],
        scopes: &mut Vec<YulScope<'db>>,
    ) -> (Vec<String>, InferTy<'db>) {
        let mut binds = Vec::new();
        let mut ty = self.engine.from_ty(Ty::unit(self.db));
        for stmt in body {
            let (new_binds, stmt_ty) = self.infer_yul_stmt(stmt, scopes);
            binds.extend(new_binds);
            ty = stmt_ty;
        }
        (binds, ty)
    }

    fn infer_yul_stmt(
        &mut self,
        stmt: &YulStmt<'db>,
        scopes: &mut Vec<YulScope<'db>>,
    ) -> (Vec<String>, InferTy<'db>) {
        match &stmt.kind {
            YulStmtKind::Block(body) => {
                scopes.push(YulScope::default());
                self.infer_yul_block_scoped(body, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Let { names, init } => {
                if let Some(init) = init {
                    let init_ty = self.infer_yul_expr(init, scopes);
                    self.check_yul_assign_arity(
                        self.yul_stmt_label_span(stmt),
                        "Yul let",
                        names.len(),
                        init_ty,
                    );
                }
                let binds = names
                    .iter()
                    .map(|name| (*name.atom()).text(self.db).to_owned())
                    .collect::<Vec<_>>();
                for name in &binds {
                    self.add_yul_local(scopes, name);
                }
                (binds, self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Assign { names, value } => {
                let value_ty = self.infer_yul_expr(value, scopes);
                self.check_yul_assign_arity(
                    self.yul_stmt_label_span(stmt),
                    "Yul assignment",
                    names.len(),
                    value_ty,
                );
                for name in names {
                    let text = (*name.atom()).text(self.db);
                    if !self.is_yul_local(scopes, text) {
                        self.check_yul_sail_var_write(self.label_span(name.span(self.db)), text);
                    }
                }
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Expr(expr) => (Vec::new(), self.infer_yul_expr(expr, scopes)),
            YulStmtKind::If { cond, body } => {
                self.infer_yul_expr(cond, scopes);
                scopes.push(YulScope::default());
                self.infer_yul_block_scoped(body, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                scopes.push(YulScope::default());
                self.infer_yul_block_scoped(init, scopes);
                self.infer_yul_expr(cond, scopes);
                self.infer_yul_block_scoped(body, scopes);
                self.infer_yul_block_scoped(post, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                self.infer_yul_expr(expr, scopes);
                for case in cases {
                    self.infer_yul_case(case, scopes);
                }
                if let Some(default) = default {
                    scopes.push(YulScope::default());
                    self.infer_yul_block_scoped(default, scopes);
                    scopes.pop();
                }
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::FunctionDef {
                name,
                params,
                rets,
                body,
            } => {
                let fn_name = (*name.atom()).text(self.db).to_owned();
                let sig = YulFunctionSig {
                    params: self.yul_word_tys(params.len()),
                    ret: self.yul_return_ty(rets.len()),
                };
                self.add_yul_function(scopes, fn_name, sig);
                scopes.push(YulScope::default());
                for name in params.iter().chain(rets) {
                    self.add_yul_local(scopes, (*name.atom()).text(self.db));
                }
                self.infer_yul_block_scoped(body, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Leave | YulStmtKind::Break | YulStmtKind::Continue => {
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Error => (Vec::new(), InferTy::Error),
        }
    }

    fn infer_yul_case(&mut self, case: &YulCase<'db>, scopes: &mut Vec<YulScope<'db>>) {
        self.infer_yul_lit(&case.lit);
        scopes.push(YulScope::default());
        self.infer_yul_block_scoped(&case.body, scopes);
        scopes.pop();
    }

    fn infer_yul_expr(
        &mut self,
        expr: &YulExpr<'db>,
        scopes: &mut Vec<YulScope<'db>>,
    ) -> InferTy<'db> {
        match &expr.kind {
            YulExprKind::Lit(lit) => self.infer_yul_lit(lit),
            YulExprKind::Ident(name) => {
                let text = (*name.atom()).text(self.db);
                if self.is_yul_local(scopes, text) {
                    self.engine.from_ty(Ty::word(self.db))
                } else {
                    self.check_yul_sail_var_read(self.yul_expr_label_span(expr), text)
                }
            }
            YulExprKind::Call { name, args } => {
                let text = (*name.atom()).text(self.db);
                let arg_tys = args
                    .iter()
                    .map(|arg| self.infer_yul_expr(arg, scopes))
                    .collect::<Vec<_>>();
                let sig = self
                    .lookup_yul_function(scopes, text)
                    .or_else(|| self.yul_builtin_sig(text));
                let Some(sig) = sig else {
                    self.diagnostics.push(TypeckDiagnostic::UnknownYulName {
                        span: self.yul_expr_label_span(expr),
                        name: text.to_owned(),
                    });
                    return InferTy::Error;
                };
                if sig.params.len() != arg_tys.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span: self.yul_expr_label_span(expr),
                        context: format!("Yul call `{text}`"),
                        expected: sig.params.len(),
                        actual: arg_tys.len(),
                    });
                }
                for ((expected, actual), arg) in sig.params.iter().cloned().zip(arg_tys).zip(args) {
                    self.unify_at(self.yul_expr_label_span(arg), expected, actual);
                }
                sig.ret
            }
            YulExprKind::Error => InferTy::Error,
        }
    }

    fn infer_yul_lit(&mut self, lit: &YulLitKind) -> InferTy<'db> {
        match lit {
            YulLitKind::Number(_) | YulLitKind::Hex(_) | YulLitKind::Bool(_) => {
                self.engine.from_ty(Ty::word(self.db))
            }
            YulLitKind::String(_) => self.engine.from_ty(Ty::string(self.db)),
            YulLitKind::Error => InferTy::Error,
        }
    }

    fn add_yul_local(&self, scopes: &mut [YulScope<'db>], name: &str) {
        if let Some(scope) = scopes.last_mut() {
            scope.values.insert(name.to_owned());
        }
    }

    fn add_yul_function(
        &self,
        scopes: &mut [YulScope<'db>],
        name: String,
        sig: YulFunctionSig<'db>,
    ) {
        if let Some(scope) = scopes.last_mut() {
            scope.functions.insert(name, sig);
        }
    }

    fn is_yul_local(&self, scopes: &[YulScope<'db>], name: &str) -> bool {
        scopes.iter().rev().any(|scope| scope.values.contains(name))
    }

    fn lookup_yul_function(
        &self,
        scopes: &[YulScope<'db>],
        name: &str,
    ) -> Option<YulFunctionSig<'db>> {
        scopes
            .iter()
            .rev()
            .find_map(|scope| scope.functions.get(name).cloned())
    }

    fn check_yul_sail_var_read(&mut self, span: LabelSpan, name: &str) -> InferTy<'db> {
        let Some(ty) = self.lookup_sail_local(name) else {
            self.diagnostics.push(TypeckDiagnostic::UnknownYulName {
                span,
                name: name.to_owned(),
            });
            return InferTy::Error;
        };
        let word = self.engine.from_ty(Ty::word(self.db));
        if self.can_unify(ty.clone(), word.clone()) {
            self.unify_at(span, ty, word.clone());
        } else {
            let actual = self.display_infer_ty(ty);
            self.diagnostics.push(TypeckDiagnostic::NonWordYulVar {
                span,
                name: name.to_owned(),
                actual,
            });
        }
        word
    }

    fn check_yul_sail_var_write(&mut self, span: LabelSpan, name: &str) {
        let Some(ty) = self.lookup_sail_local(name) else {
            return;
        };
        let word = self.engine.from_ty(Ty::word(self.db));
        if self.can_unify(ty.clone(), word.clone()) {
            self.unify_at(span, ty, word);
        } else {
            let actual = self.display_infer_ty(ty);
            self.diagnostics.push(TypeckDiagnostic::NonWordYulVar {
                span,
                name: name.to_owned(),
                actual,
            });
        }
    }

    fn check_yul_assign_arity(
        &mut self,
        span: LabelSpan,
        context: &str,
        expected: usize,
        actual_ty: InferTy<'db>,
    ) {
        if matches!(self.engine.resolve(actual_ty.clone()), InferTy::Error) {
            return;
        }
        let actual = self.yul_return_arity(actual_ty);
        if expected != actual {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                span,
                context: context.to_owned(),
                expected,
                actual,
            });
        }
    }

    fn yul_return_arity(&mut self, ty: InferTy<'db>) -> usize {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Error => 0,
            InferTy::Tuple(elems) => elems.len(),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => 0,
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => 1 + self.yul_return_arity(args[1].clone()),
            _ => 1,
        }
    }

    fn yul_word_tys(&mut self, count: usize) -> Vec<InferTy<'db>> {
        let word = self.engine.from_ty(Ty::word(self.db));
        vec![word; count]
    }

    fn yul_return_ty(&mut self, count: usize) -> InferTy<'db> {
        match count {
            0 => self.engine.from_ty(Ty::unit(self.db)),
            1 => self.engine.from_ty(Ty::word(self.db)),
            _ => InferTy::Tuple(self.yul_word_tys(count)),
        }
    }

    fn yul_builtin_sig(&mut self, name: &str) -> Option<YulFunctionSig<'db>> {
        let word = self.engine.from_ty(Ty::word(self.db));
        let string = self.engine.from_ty(Ty::string(self.db));
        let unit = self.engine.from_ty(Ty::unit(self.db));
        let word_params = |count: usize| vec![word.clone(); count];
        let sig = match name {
            "stop" | "invalid" => YulFunctionSig {
                params: Vec::new(),
                ret: unit.clone(),
            },
            "add" | "mul" | "sub" | "div" | "sdiv" | "mod" | "smod" | "exp" | "signextend"
            | "lt" | "gt" | "slt" | "sgt" | "eq" | "and" | "or" | "xor" | "byte" | "shl"
            | "shr" | "sar" => YulFunctionSig {
                params: word_params(2),
                ret: word.clone(),
            },
            "addmod" | "mulmod" => YulFunctionSig {
                params: word_params(3),
                ret: word.clone(),
            },
            "iszero" | "not" | "clz" | "balance" | "calldataload" | "extcodesize"
            | "extcodehash" | "blockhash" | "blobhash" | "pop" | "mload" | "sload" | "tload"
            | "selfdestruct" => {
                let ret = if matches!(name, "pop" | "selfdestruct") {
                    unit.clone()
                } else {
                    word.clone()
                };
                YulFunctionSig {
                    params: word_params(1),
                    ret,
                }
            }
            "address" | "origin" | "caller" | "callvalue" | "calldatasize" | "codesize"
            | "gasprice" | "returndatasize" | "coinbase" | "timestamp" | "number"
            | "prevrandao" | "gaslimit" | "chainid" | "selfbalance" | "basefee" | "blobbasefee"
            | "msize" | "gas" => YulFunctionSig {
                params: Vec::new(),
                ret: word.clone(),
            },
            "calldatacopy" | "codecopy" | "returndatacopy" | "mstore" | "mstore8" | "sstore"
            | "tstore" | "mcopy" | "datacopy" => YulFunctionSig {
                params: word_params(3)
                    .into_iter()
                    .take(match name {
                        "mstore" | "mstore8" | "sstore" | "tstore" => 2,
                        _ => 3,
                    })
                    .collect(),
                ret: unit.clone(),
            },
            "extcodecopy" => YulFunctionSig {
                params: word_params(4),
                ret: unit.clone(),
            },
            "log0" => YulFunctionSig {
                params: word_params(2),
                ret: unit.clone(),
            },
            "log1" => YulFunctionSig {
                params: word_params(3),
                ret: unit.clone(),
            },
            "log2" => YulFunctionSig {
                params: word_params(4),
                ret: unit.clone(),
            },
            "log3" => YulFunctionSig {
                params: word_params(5),
                ret: unit.clone(),
            },
            "log4" => YulFunctionSig {
                params: word_params(6),
                ret: unit.clone(),
            },
            "create" => YulFunctionSig {
                params: word_params(3),
                ret: word.clone(),
            },
            "create2" => YulFunctionSig {
                params: word_params(4),
                ret: word.clone(),
            },
            "call" | "callcode" => YulFunctionSig {
                params: word_params(7),
                ret: word.clone(),
            },
            "delegatecall" | "staticcall" => YulFunctionSig {
                params: word_params(6),
                ret: word.clone(),
            },
            "return" | "revert" => YulFunctionSig {
                params: word_params(2),
                ret: self.engine.fresh_var(),
            },
            "datasize" | "dataoffset" | "loadimmutable" | "linkersymbol" => YulFunctionSig {
                params: vec![string.clone()],
                ret: word.clone(),
            },
            "setimmutable" => YulFunctionSig {
                params: vec![word.clone(), string.clone(), word.clone()],
                ret: unit.clone(),
            },
            "memoryguard" => YulFunctionSig {
                params: word_params(1),
                ret: word.clone(),
            },
            _ => return None,
        };
        Some(sig)
    }
}
