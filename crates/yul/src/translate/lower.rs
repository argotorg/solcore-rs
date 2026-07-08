use std::collections::{BTreeMap, BTreeSet};

use hir::Db as HirDb;
use hull::{
    Alt, CodeBlock as HullCodeBlock, Expr as HullExpr, ExprKind, Function as HullFunction,
    Object as HullObject, PatKind, Program as HullProgram, Stmt as HullStmt, StmtKind,
    Ty as HullTy, TyKind,
};

use crate::ast::{Case, Code, Expr, Inner, Literal, Object, Program, Stmt};

use super::{
    TranslationError, Translator,
    asm::AsmScopes,
    location::{
        Location, alloc_loc, con_lit, con_payload, copy_locs, flatten_lhs, flatten_rhs,
        is_unit_loc, is_word_type, load_loc, lower_in_k_loc, normalize_loc, pad_to_size, pair_locs,
        partition_allocs, size_of_loc, size_of_ty, zero_sized_type,
    },
    names::{LoweredCallee, canonical_word_lit, lower_callee, yul_fun_name},
    validate::render_strict_assembly_program,
};

pub fn translate_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<Program, TranslationError> {
    let mut translator = Translator::new(db);
    translator.translate_program(program)
}

pub fn render_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<String, TranslationError> {
    render_hull_program_object(db, program, None)
}

pub fn render_hull_program_object<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
    object_name: Option<&str>,
) -> Result<String, TranslationError> {
    let program = translate_hull_program(db, program)?;
    render_strict_assembly_program(&program, object_name)
}

impl<'db> Translator<'db> {
    fn translate_program(
        &mut self,
        program: &HullProgram<'db>,
    ) -> Result<Program, TranslationError> {
        if program.objects.is_empty() {
            let mut code = self.translate_code_parts(&program.functions, &[])?;
            code.stmts.extend(main_result_return_block());
            return Ok(Program::single_object(Object {
                name: "OutputDeploy".into(),
                code: Code::new(Vec::new()),
                inners: vec![Inner::Object(Object {
                    name: "Output".into(),
                    code,
                    inners: Vec::new(),
                })],
            }));
        }

        let objects = program
            .objects
            .iter()
            .map(|object| self.translate_object(object))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Program { objects })
    }

    fn translate_object(&mut self, object: &HullObject<'db>) -> Result<Object, TranslationError> {
        let code = self.translate_code_block(&object.code)?;
        let inners = object
            .inners
            .iter()
            .map(|inner| self.translate_object(inner).map(Inner::Object))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Object {
            name: object.name.as_str().into(),
            code,
            inners,
        })
    }

    fn translate_code_block(
        &mut self,
        code: &HullCodeBlock<'db>,
    ) -> Result<Code, TranslationError> {
        self.translate_code_parts(&code.functions, &code.stmts)
    }

    fn translate_code_parts(
        &mut self,
        functions: &[HullFunction<'db>],
        stmts: &[HullStmt<'db>],
    ) -> Result<Code, TranslationError> {
        let saved_vars = std::mem::replace(&mut self.vars, vec![BTreeMap::new()]);
        let saved_functions = std::mem::take(&mut self.user_functions);
        self.user_functions = functions
            .iter()
            .map(|function| function.name.as_str().to_owned())
            .collect::<BTreeSet<_>>();

        let result = (|| {
            let mut out = Vec::new();
            for function in functions {
                out.push(self.translate_function(function)?);
            }
            out.extend(self.gen_stmts(stmts)?);
            Ok(Code::new(out))
        })();

        self.vars = saved_vars;
        self.user_functions = saved_functions;
        result
    }

    fn translate_function(
        &mut self,
        function: &HullFunction<'db>,
    ) -> Result<Stmt, TranslationError> {
        let saved_vars = std::mem::replace(&mut self.vars, vec![BTreeMap::new()]);

        let result = (|| {
            let mut params = Vec::new();
            for arg in &function.args {
                if is_word_type(&arg.ty) {
                    let name = self.fresh_source_name(arg.name.as_str());
                    self.insert_var(arg.name.as_str().to_owned(), Location::Named(name.clone()));
                    params.push(name);
                } else {
                    let loc = self.build_loc(&arg.ty)?;
                    params.extend(flatten_lhs(&loc)?);
                    self.insert_var(arg.name.as_str().to_owned(), loc);
                }
            }

            let returns = match function.ret.strip_named().kind {
                TyKind::Unit => Vec::new(),
                TyKind::Word => {
                    let name = self.fresh_internal_name("result");
                    self.insert_var("_result".to_owned(), Location::Named(name.clone()));
                    vec![name]
                }
                _ if zero_sized_type(&function.ret) => Vec::new(),
                _ => {
                    let loc = self.build_loc(&function.ret)?;
                    let returns = flatten_lhs(&loc)?;
                    self.insert_var("_result".to_owned(), loc);
                    returns
                }
            };

            let body = self.gen_stmts(&function.body)?;
            Ok(Stmt::Function {
                name: yul_fun_name(function.name.as_str()),
                params,
                returns,
                body,
            })
        })();

        self.vars = saved_vars;
        result
    }

    fn gen_stmts(&mut self, stmts: &[HullStmt<'db>]) -> Result<Vec<Stmt>, TranslationError> {
        let mut out = Vec::new();
        for stmt in stmts {
            out.extend(self.gen_stmt(stmt)?);
        }
        Ok(out)
    }

    fn gen_stmt(&mut self, stmt: &HullStmt<'db>) -> Result<Vec<Stmt>, TranslationError> {
        match &stmt.kind {
            StmtKind::Let { name, ty } => self.alloc_var(name.as_str(), ty),
            StmtKind::Assign { lhs, rhs } => self.hull_assign(lhs, rhs),
            StmtKind::Expr(expr) => self.gen_expr(expr).map(|(stmts, _)| stmts),
            StmtKind::Return(expr) => {
                let (mut out, loc) = self.gen_expr(expr)?;
                if !is_unit_loc(&loc) {
                    let result = self.lookup_var("_result")?;
                    out.extend(copy_locs(&result, &loc)?);
                }
                out.push(Stmt::Leave);
                Ok(out)
            }
            StmtKind::Block(stmts) => {
                self.with_local_env(|this| Ok(vec![Stmt::Block(this.gen_stmts(stmts)?)]))
            }
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => self.with_local_env(|this| {
                let mut init_stmts = this.gen_stmts(init)?;
                let (cond_stmts, cond_loc) = this.gen_expr(cond)?;
                let cond_expr = load_loc(&normalize_loc(cond_loc))?;
                let post_stmts = this.gen_stmts(post)?;
                let body_stmts = this.gen_stmts(body)?;

                let (cond_allocs, cond_compute) = partition_allocs(cond_stmts);
                let (post_allocs, post_compute) = partition_allocs(post_stmts);
                init_stmts.extend(cond_allocs);
                init_stmts.extend(post_allocs);
                init_stmts.extend(cond_compute.clone());

                let mut post = post_compute;
                post.extend(cond_compute);
                Ok(vec![Stmt::For {
                    init: init_stmts,
                    cond: cond_expr,
                    post,
                    body: body_stmts,
                }])
            }),
            StmtKind::Break => Ok(vec![Stmt::Break]),
            StmtKind::Continue => Ok(vec![Stmt::Continue]),
            StmtKind::Match {
                target,
                scrutinee,
                alts,
            } => {
                let (mut out, loc) = self.gen_expr(scrutinee)?;
                let normalized = normalize_loc(loc);
                let (tag, payload) = match normalized {
                    Location::Seq(locs) => {
                        let mut iter = locs.into_iter();
                        let Some(tag) = iter.next() else {
                            return Err(TranslationError::new("cannot match an empty location"));
                        };
                        (tag, Location::Seq(iter.collect()))
                    }
                    tag => (tag, Location::Seq(Vec::new())),
                };
                let (cases, default) = self.gen_alts(target.strip_named(), payload, alts)?;
                out.push(Stmt::Switch {
                    expr: load_loc(&tag)?,
                    cases,
                    default,
                });
                Ok(out)
            }
            StmtKind::Assembly(stmts) => {
                let mut asm = AsmScopes::new();
                self.convert_yul_stmts(stmts, &mut asm)
            }
            StmtKind::Revert(message) => Ok(revert_stmts(message)),
            StmtKind::Comment(comment) => Ok(vec![Stmt::Comment(comment.clone())]),
        }
    }

    fn gen_expr(
        &mut self,
        expr: &HullExpr<'db>,
    ) -> Result<(Vec<Stmt>, Location), TranslationError> {
        match &expr.kind {
            ExprKind::Word(value) => Ok((Vec::new(), Location::Word(canonical_word_lit(value)?))),
            ExprKind::Bool(value) => Ok((Vec::new(), Location::Bool(*value))),
            ExprKind::Unit => Ok((Vec::new(), Location::Seq(Vec::new()))),
            ExprKind::Var(name) => self.lookup_var(name.as_str()).map(|loc| (Vec::new(), loc)),
            ExprKind::Pair(lhs, rhs) => {
                let (mut lhs_stmts, lhs_loc) = self.gen_expr(lhs)?;
                let (rhs_stmts, rhs_loc) = self.gen_expr(rhs)?;
                lhs_stmts.extend(rhs_stmts);
                Ok((lhs_stmts, Location::Seq(vec![lhs_loc, rhs_loc])))
            }
            ExprKind::Fst(inner) => {
                let (stmts, loc) = self.gen_expr(inner)?;
                let (lhs, _) = pair_locs(loc)?;
                Ok((stmts, lhs))
            }
            ExprKind::Snd(inner) => {
                let (stmts, loc) = self.gen_expr(inner)?;
                let (_, rhs) = pair_locs(loc)?;
                Ok((stmts, rhs))
            }
            ExprKind::Inl { target, value } => {
                let (stmts, loc) = self.gen_expr(value)?;
                let target = target.strip_named();
                let TyKind::Sum(lhs, rhs) = &target.kind else {
                    return Err(TranslationError::new("inl target is not a sum"));
                };
                let padded = pad_to_size(loc, size_of_ty(lhs)?.max(size_of_ty(rhs)?));
                Ok((stmts, Location::Seq(vec![Location::Bool(false), padded])))
            }
            ExprKind::Inr { target, value } => {
                let (stmts, loc) = self.gen_expr(value)?;
                let target = target.strip_named();
                let TyKind::Sum(lhs, rhs) = &target.kind else {
                    return Err(TranslationError::new("inr target is not a sum"));
                };
                let padded = pad_to_size(loc, size_of_ty(lhs)?.max(size_of_ty(rhs)?));
                Ok((stmts, Location::Seq(vec![Location::Bool(true), padded])))
            }
            ExprKind::InK {
                index,
                target,
                value,
            } => {
                let (stmts, loc) = self.gen_expr(value)?;
                Ok((stmts, lower_in_k_loc(target, *index, loc)?))
            }
            ExprKind::Call { callee, args } => {
                let mut out = Vec::new();
                let mut yul_args = Vec::new();
                let mut arg_locs = Vec::new();
                for arg in args {
                    let (arg_stmts, arg_loc) = self.gen_expr(arg)?;
                    out.extend(arg_stmts);
                    yul_args.extend(flatten_rhs(&arg_loc));
                    arg_locs.push(arg_loc);
                }

                if matches!(
                    lower_callee(callee.as_str(), &self.user_functions),
                    LoweredCallee::Identity
                ) {
                    let Some(loc) = arg_locs.into_iter().next() else {
                        return Err(TranslationError::new("identity call without argument"));
                    };
                    return Ok((out, loc));
                }

                let (alloc_stmts, result_loc) = self.hull_alloc(&expr.ty)?;
                out.extend(alloc_stmts);
                let LoweredCallee::Call(name) = lower_callee(callee.as_str(), &self.user_functions)
                else {
                    unreachable!("identity handled above");
                };
                let call = Expr::call(name, yul_args);
                if size_of_loc(&result_loc) == 0 {
                    out.push(Stmt::Expr(call));
                } else {
                    out.push(Stmt::Assign {
                        names: flatten_lhs(&result_loc)?,
                        value: call,
                    });
                }
                Ok((out, result_loc))
            }
            ExprKind::If {
                target,
                cond,
                then_expr,
                else_expr,
            } => {
                let (mut out, result_loc) = self.hull_alloc(target)?;
                let (cond_stmts, cond_loc) = self.gen_expr(cond)?;
                let (then_stmts, then_loc) = self.gen_expr(then_expr)?;
                let (else_stmts, else_loc) = self.gen_expr(else_expr)?;
                out.extend(cond_stmts);
                let mut then_body = then_stmts;
                then_body.extend(copy_locs(&result_loc, &then_loc)?);
                let mut else_body = else_stmts;
                else_body.extend(copy_locs(&result_loc, &else_loc)?);
                out.push(Stmt::Switch {
                    expr: load_loc(&normalize_loc(cond_loc))?,
                    cases: vec![Case {
                        lit: Literal::Number("0".to_owned()),
                        body: else_body,
                    }],
                    default: Some(then_body),
                });
                Ok((out, result_loc))
            }
        }
    }

    fn gen_alts(
        &mut self,
        target: &HullTy<'db>,
        payload: Location,
        alts: &[Alt<'db>],
    ) -> Result<(Vec<Case>, Option<Vec<Stmt>>), TranslationError> {
        let mut cases = Vec::new();
        let mut default = None;
        for alt in alts {
            match &alt.pat.kind {
                PatKind::Con(con) => {
                    let lit = con_lit(target, *con)?;
                    let payload = con_payload(target, *con, &payload)?;
                    let body = self.with_local_env(|this| {
                        this.insert_var(alt.binder.as_str().to_owned(), payload);
                        this.gen_stmts(&alt.body)
                    })?;
                    cases.push(Case { lit, body });
                }
                PatKind::IntLit(value) => {
                    let body = self.with_local_env(|this| {
                        this.insert_var(alt.binder.as_str().to_owned(), payload.clone());
                        this.gen_stmts(&alt.body)
                    })?;
                    cases.push(Case {
                        lit: Literal::Number(canonical_word_lit(value)?),
                        body,
                    });
                }
                PatKind::Var(name) => {
                    let body = self.with_local_env(|this| {
                        this.insert_var(name.as_str().to_owned(), payload.clone());
                        this.insert_var(alt.binder.as_str().to_owned(), payload.clone());
                        this.gen_stmts(&alt.body)
                    })?;
                    default = Some(body);
                }
                PatKind::Wildcard => {
                    let body = self.with_local_env(|this| {
                        this.insert_var(alt.binder.as_str().to_owned(), payload.clone());
                        this.gen_stmts(&alt.body)
                    })?;
                    default = Some(body);
                }
            }
        }
        Ok((cases, default))
    }

    fn alloc_var(&mut self, name: &str, ty: &HullTy<'db>) -> Result<Vec<Stmt>, TranslationError> {
        if is_word_type(ty) {
            let yul_name = self.fresh_source_name(name);
            self.insert_var(name.to_owned(), Location::Named(yul_name.clone()));
            return Ok(vec![Stmt::Let {
                names: vec![yul_name],
                init: None,
            }]);
        }
        let (stmts, loc) = self.hull_alloc(ty)?;
        self.insert_var(name.to_owned(), loc);
        Ok(stmts)
    }

    fn hull_alloc(&mut self, ty: &HullTy<'db>) -> Result<(Vec<Stmt>, Location), TranslationError> {
        let loc = self.build_loc(ty)?;
        let stmts = alloc_loc(&loc);
        Ok((stmts, loc))
    }

    fn build_loc(&mut self, ty: &HullTy<'db>) -> Result<Location, TranslationError> {
        match &ty.strip_named().kind {
            TyKind::Word | TyKind::Bool | TyKind::NamedRef { .. } | TyKind::Function { .. } => {
                Ok(self.fresh_stack_loc())
            }
            TyKind::Unit => Ok(Location::Seq(Vec::new())),
            TyKind::Product(lhs, rhs) => Ok(Location::Seq(vec![
                self.build_loc(lhs)?,
                self.build_loc(rhs)?,
            ])),
            TyKind::Sum(_, _) => {
                let slots = (0..size_of_ty(ty)?)
                    .map(|_| self.fresh_stack_loc())
                    .collect();
                Ok(Location::Seq(slots))
            }
            TyKind::Named { inner, .. } => self.build_loc(inner),
        }
    }

    fn hull_assign(
        &mut self,
        lhs: &HullExpr<'db>,
        rhs: &HullExpr<'db>,
    ) -> Result<Vec<Stmt>, TranslationError> {
        let (mut lhs_stmts, lhs_loc) = self.gen_expr(lhs)?;
        let (rhs_stmts, rhs_loc) = self.gen_expr(rhs)?;
        if size_of_loc(&lhs_loc) == 0 {
            return Ok(rhs_stmts);
        }
        lhs_stmts.extend(rhs_stmts);
        lhs_stmts.extend(copy_locs(&lhs_loc, &rhs_loc)?);
        Ok(lhs_stmts)
    }

    fn fresh_stack_loc(&mut self) -> Location {
        let loc = Location::Stack(self.counter);
        self.counter += 1;
        loc
    }
    fn lookup_var(&self, name: &str) -> Result<Location, TranslationError> {
        self.lookup_var_opt(name)
            .ok_or_else(|| TranslationError::new(format!("variable not found: {name}")))
    }

    pub(super) fn lookup_var_opt(&self, name: &str) -> Option<Location> {
        self.vars
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn insert_var(&mut self, name: String, loc: Location) {
        self.vars
            .last_mut()
            .expect("scope stack is never empty")
            .insert(name, loc);
    }

    fn with_local_env<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, TranslationError>,
    ) -> Result<T, TranslationError> {
        let outer_depth = self.vars.len();
        self.vars.push(BTreeMap::new());
        let result = f(self);
        debug_assert_eq!(
            self.vars.len(),
            outer_depth + 1,
            "local environment scope stack depth changed unexpectedly"
        );
        self.vars.pop().expect("scope stack is never empty");
        debug_assert_eq!(
            self.vars.len(),
            outer_depth,
            "local environment scope stack depth was not restored"
        );
        result
    }
}

fn main_result_return_block() -> Vec<Stmt> {
    vec![Stmt::Block(vec![
        Stmt::Expr(Expr::call(
            "mstore",
            vec![Expr::number("0"), Expr::ident("_mainresult")],
        )),
        Stmt::Expr(Expr::call(
            "return",
            vec![Expr::number("0"), Expr::number("32")],
        )),
    ])]
}

fn revert_stmts(message: &str) -> Vec<Stmt> {
    vec![
        Stmt::Expr(Expr::call(
            "mstore",
            vec![Expr::number("0"), Expr::string(message)],
        )),
        Stmt::Expr(Expr::call(
            "revert",
            vec![Expr::number("0"), Expr::number(message.len().to_string())],
        )),
    ]
}
