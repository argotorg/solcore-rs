use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use hir::{
    Db as HirDb,
    ast::function::{
        YulCase as HirYulCase, YulExpr as HirYulExpr, YulExprKind, YulLitKind,
        YulStmt as HirYulStmt, YulStmtKind,
    },
};
use hull::{
    Alt, CodeBlock as HullCodeBlock, Con, Expr as HullExpr, ExprKind, Function as HullFunction,
    Object as HullObject, PatKind, Program as HullProgram, Stmt as HullStmt, StmtKind,
    Ty as HullTy, TyKind, wrap_word_literal,
};

use crate::{
    ast::{Case, Code, Expr, Inner, Literal, Object, Program, Stmt},
    pretty::pretty_object,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationError {
    message: String,
}

impl TranslationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TranslationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for TranslationError {}

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Location {
    Word(String),
    Bool(bool),
    Stack(usize),
    Named(String),
    Seq(Vec<Location>),
    Empty(usize),
}

struct Translator<'db> {
    db: &'db dyn HirDb,
    counter: usize,
    name_counter: usize,
    used_yul_names: BTreeSet<String>,
    vars: Vec<BTreeMap<String, Location>>,
    user_functions: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct AsmScopes {
    values: Vec<BTreeMap<String, String>>,
    functions: Vec<BTreeMap<String, String>>,
}

enum LoweredCallee {
    Call(String),
    Identity,
}

impl<'db> Translator<'db> {
    fn new(db: &'db dyn HirDb) -> Self {
        Self {
            db,
            counter: 0,
            name_counter: 0,
            used_yul_names: BTreeSet::new(),
            vars: vec![BTreeMap::new()],
            user_functions: BTreeSet::new(),
        }
    }

    fn translate_program(
        &mut self,
        program: &HullProgram<'db>,
    ) -> Result<Program, TranslationError> {
        if program.objects.is_empty() {
            let mut code = self.translate_code_parts(&program.functions, &[])?;
            code.stmts.extend(main_result_return_block());
            return Ok(Program::single_object(Object {
                name: "OutputDeploy".to_owned(),
                code: Code::new(Vec::new()),
                inners: vec![Inner::Object(Object {
                    name: "Output".to_owned(),
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
            name: object.name.clone(),
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
            .map(|function| function.name.clone())
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
                    let name = self.fresh_source_name(&arg.name);
                    self.insert_var(arg.name.clone(), Location::Named(name.clone()));
                    params.push(name);
                } else {
                    let loc = self.build_loc(&arg.ty)?;
                    params.extend(flatten_lhs(&loc)?);
                    self.insert_var(arg.name.clone(), loc);
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
                name: yul_fun_name(&function.name),
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
            StmtKind::Let { name, ty } => self.alloc_var(name, ty),
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
            ExprKind::Var(name) => self.lookup_var(name).map(|loc| (Vec::new(), loc)),
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
                    lower_callee(callee, &self.user_functions),
                    LoweredCallee::Identity
                ) {
                    let Some(loc) = arg_locs.into_iter().next() else {
                        return Err(TranslationError::new("identity call without argument"));
                    };
                    return Ok((out, loc));
                }

                let (alloc_stmts, result_loc) = self.hull_alloc(&expr.ty)?;
                out.extend(alloc_stmts);
                let LoweredCallee::Call(name) = lower_callee(callee, &self.user_functions) else {
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
                        this.insert_var(alt.binder.clone(), payload);
                        this.gen_stmts(&alt.body)
                    })?;
                    cases.push(Case { lit, body });
                }
                PatKind::IntLit(value) => {
                    let body = self.with_local_env(|this| {
                        this.insert_var(alt.binder.clone(), payload.clone());
                        this.gen_stmts(&alt.body)
                    })?;
                    cases.push(Case {
                        lit: Literal::Number(canonical_word_lit(value)?),
                        body,
                    });
                }
                PatKind::Var(name) => {
                    let body = self.with_local_env(|this| {
                        this.insert_var(name.clone(), payload.clone());
                        this.insert_var(alt.binder.clone(), payload.clone());
                        this.gen_stmts(&alt.body)
                    })?;
                    default = Some(body);
                }
                PatKind::Wildcard => {
                    let body = self.with_local_env(|this| {
                        this.insert_var(alt.binder.clone(), payload.clone());
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

    fn convert_yul_stmts(
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

    fn fresh_stack_loc(&mut self) -> Location {
        let loc = Location::Stack(self.counter);
        self.counter += 1;
        loc
    }

    fn fresh_source_name(&mut self, source: &str) -> String {
        self.fresh_yul_name("src", source)
    }

    fn fresh_asm_name(&mut self, source: &str) -> String {
        self.fresh_yul_name("asm", source)
    }

    fn fresh_internal_name(&mut self, source: &str) -> String {
        self.fresh_yul_name("gen", source)
    }

    fn fresh_yul_name(&mut self, prefix: &str, source: &str) -> String {
        let source = yul_ident_fragment(source);
        loop {
            let name = format!("{prefix}${source}_{}", self.name_counter);
            self.name_counter += 1;
            if !is_forbidden_yul_identifier(&name) && self.used_yul_names.insert(name.clone()) {
                return name;
            }
        }
    }

    fn lookup_var(&self, name: &str) -> Result<Location, TranslationError> {
        self.lookup_var_opt(name)
            .ok_or_else(|| TranslationError::new(format!("variable not found: {name}")))
    }

    fn lookup_var_opt(&self, name: &str) -> Option<Location> {
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
        let saved = self.vars.clone();
        self.vars.push(BTreeMap::new());
        let result = f(self);
        self.vars = saved;
        result
    }
}

impl AsmScopes {
    fn new() -> Self {
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

fn render_strict_assembly_program(
    program: &Program,
    object_name: Option<&str>,
) -> Result<String, TranslationError> {
    let object = select_strict_object(program, object_name)?;
    validate_object(object)?;
    Ok(pretty_object(object))
}

fn select_strict_object<'a>(
    program: &'a Program,
    object_name: Option<&str>,
) -> Result<&'a Object, TranslationError> {
    if let Some(name) = object_name {
        return program
            .objects
            .iter()
            .find(|object| object.name == name)
            .ok_or_else(|| {
                TranslationError::new(format!(
                    "Yul object `{name}` not found; available top-level objects: {}",
                    top_level_object_list(program)
                ))
            });
    }

    match program.objects.as_slice() {
        [object] => Ok(object),
        [] => Err(TranslationError::new(
            "strict-assembly output requires one top-level object; found none",
        )),
        _ => Err(TranslationError::new(format!(
            "strict-assembly output requires one top-level object; found {} ({})",
            program.objects.len(),
            top_level_object_list(program)
        ))),
    }
}

fn top_level_object_list(program: &Program) -> String {
    program
        .objects
        .iter()
        .map(|object| object.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, Copy)]
enum ControlRegion {
    Outside,
    LoopInit,
    LoopPost,
    LoopBody,
}

fn validate_object(object: &Object) -> Result<(), TranslationError> {
    validate_code(&object.code)?;
    for inner in &object.inners {
        match inner {
            Inner::Object(object) => validate_object(object)?,
            Inner::Data(_) => {}
        }
    }
    Ok(())
}

fn validate_code(code: &Code) -> Result<(), TranslationError> {
    validate_stmts(&code.stmts, ControlRegion::Outside)
}

fn validate_stmts(stmts: &[Stmt], region: ControlRegion) -> Result<(), TranslationError> {
    for stmt in stmts {
        validate_stmt(stmt, region)?;
    }
    Ok(())
}

fn validate_stmt(stmt: &Stmt, region: ControlRegion) -> Result<(), TranslationError> {
    match stmt {
        Stmt::Block(stmts) => validate_stmts(stmts, region),
        Stmt::Function {
            name,
            params,
            returns,
            body,
        } => {
            validate_decl_name(name)?;
            for name in params.iter().chain(returns) {
                validate_decl_name(name)?;
            }
            validate_stmts(body, ControlRegion::Outside)
        }
        Stmt::Let { names, init } => {
            for name in names {
                validate_decl_name(name)?;
            }
            if let Some(init) = init {
                validate_expr(init)?;
            }
            Ok(())
        }
        Stmt::Assign { names, value } => {
            for name in names {
                validate_decl_name(name)?;
            }
            validate_expr(value)
        }
        Stmt::If { cond, body } => {
            validate_expr(cond)?;
            validate_stmts(body, region)
        }
        Stmt::Switch {
            expr,
            cases,
            default,
        } => {
            validate_expr(expr)?;
            for case in cases {
                validate_lit(&case.lit)?;
                validate_stmts(&case.body, region)?;
            }
            if let Some(default) = default {
                validate_stmts(default, region)?;
            }
            Ok(())
        }
        Stmt::For {
            init,
            cond,
            post,
            body,
        } => {
            validate_stmts(init, ControlRegion::LoopInit)?;
            validate_expr(cond)?;
            validate_stmts(post, ControlRegion::LoopPost)?;
            validate_stmts(body, ControlRegion::LoopBody)
        }
        Stmt::Break => validate_break_continue("break", region),
        Stmt::Continue => validate_break_continue("continue", region),
        Stmt::Leave | Stmt::Comment(_) => Ok(()),
        Stmt::Expr(expr) => validate_expr(expr),
    }
}

fn validate_break_continue(keyword: &str, region: ControlRegion) -> Result<(), TranslationError> {
    match region {
        ControlRegion::LoopBody => Ok(()),
        ControlRegion::LoopInit => Err(TranslationError::new(format!(
            "`{keyword}` in for-loop init block is not allowed"
        ))),
        ControlRegion::LoopPost => Err(TranslationError::new(format!(
            "`{keyword}` in for-loop post block is not allowed"
        ))),
        ControlRegion::Outside => Err(TranslationError::new(format!(
            "`{keyword}` must be inside a for-loop body"
        ))),
    }
}

fn validate_expr(expr: &Expr) -> Result<(), TranslationError> {
    match expr {
        Expr::Call { name, args } => {
            validate_call_name(name)?;
            for arg in args {
                validate_expr(arg)?;
            }
            Ok(())
        }
        Expr::Ident(name) => validate_decl_name(name),
        Expr::Lit(lit) => validate_lit(lit),
    }
}

fn validate_lit(lit: &Literal) -> Result<(), TranslationError> {
    match lit {
        Literal::Number(value) => canonical_numeric_lit(value).map(|_| ()),
        Literal::Hex(value) => canonical_hex_lit(value).map(|_| ()),
        Literal::String(_) | Literal::Bool(_) => Ok(()),
    }
}

fn validate_decl_name(name: &str) -> Result<(), TranslationError> {
    if !is_valid_yul_identifier(name) {
        return Err(TranslationError::new(format!(
            "invalid Yul identifier `{name}`"
        )));
    }
    if is_forbidden_yul_identifier(name) {
        return Err(TranslationError::new(format!(
            "Yul identifier `{name}` is reserved or builtin"
        )));
    }
    Ok(())
}

fn validate_call_name(name: &str) -> Result<(), TranslationError> {
    if is_valid_yul_identifier(name) {
        Ok(())
    } else {
        Err(TranslationError::new(format!(
            "invalid Yul function name `{name}`"
        )))
    }
}

fn is_valid_yul_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$')) {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
}

fn yul_fun_name(name: &str) -> String {
    format!("usr${name}")
}

fn yul_var_name(name: &str) -> String {
    name.to_owned()
}

fn stack_name(index: usize) -> String {
    format!("_v{index}")
}

fn lower_callee(callee: &str, user_functions: &BTreeSet<String>) -> LoweredCallee {
    if user_functions.contains(callee) {
        return LoweredCallee::Call(yul_fun_name(callee));
    }

    let name = match callee {
        "primAddWord" | "integerAdd" => "add",
        "subWord" | "integerSub" => "sub",
        "integerMul" => "mul",
        "primEqWord" | "integerEq" => "eq",
        "gtWord" => "gt",
        "integerLt" => "lt",
        "bxorWord" => "xor",
        "bandWord" => "and",
        "borWord" => "or",
        "wordFromInteger" | "wordToInteger" => return LoweredCallee::Identity,
        name => name,
    };
    LoweredCallee::Call(name.to_owned())
}

fn is_word_type(ty: &HullTy<'_>) -> bool {
    matches!(ty.strip_named().kind, TyKind::Word)
}

fn zero_sized_type(ty: &HullTy<'_>) -> bool {
    size_of_ty(ty).is_ok_and(|size| size == 0)
}

fn lower_in_k_loc(
    target: &HullTy<'_>,
    index: usize,
    payload: Location,
) -> Result<Location, TranslationError> {
    match &target.strip_named().kind {
        TyKind::Named { inner, .. } => lower_in_k_loc(inner, index, payload),
        TyKind::Sum(lhs, rhs) if index == 0 => {
            let padded = pad_to_size(payload, size_of_ty(lhs)?.max(size_of_ty(rhs)?));
            Ok(Location::Seq(vec![Location::Bool(false), padded]))
        }
        TyKind::Sum(lhs, rhs) => {
            let nested = lower_in_k_loc(rhs, index - 1, payload)?;
            let padded = pad_to_size(nested, size_of_ty(lhs)?.max(size_of_ty(rhs)?));
            Ok(Location::Seq(vec![Location::Bool(true), padded]))
        }
        _ if index == 0 => Ok(payload),
        _ => Err(TranslationError::new(format!(
            "bad injection index {index} for non-sum target"
        ))),
    }
}

fn size_of_ty(ty: &HullTy<'_>) -> Result<usize, TranslationError> {
    match &ty.strip_named().kind {
        TyKind::Word | TyKind::Bool | TyKind::NamedRef { .. } | TyKind::Function { .. } => Ok(1),
        TyKind::Unit => Ok(0),
        TyKind::Product(lhs, rhs) => Ok(size_of_ty(lhs)? + size_of_ty(rhs)?),
        TyKind::Sum(lhs, rhs) => Ok(1 + size_of_ty(lhs)?.max(size_of_ty(rhs)?)),
        TyKind::Named { inner, .. } => size_of_ty(inner),
    }
}

fn size_of_loc(loc: &Location) -> usize {
    match loc {
        Location::Empty(size) => *size,
        Location::Seq(locs) => locs.iter().map(size_of_loc).sum(),
        _ => 1,
    }
}

fn alloc_loc(loc: &Location) -> Vec<Stmt> {
    stack_slots(loc)
        .into_iter()
        .map(|index| Stmt::Let {
            names: vec![stack_name(index)],
            init: None,
        })
        .collect()
}

fn stack_slots(loc: &Location) -> Vec<usize> {
    match loc {
        Location::Stack(index) => vec![*index],
        Location::Seq(locs) => locs.iter().flat_map(stack_slots).collect(),
        _ => Vec::new(),
    }
}

fn flatten_rhs(loc: &Location) -> Vec<Expr> {
    match loc {
        Location::Word(value) => vec![Expr::number(value.clone())],
        Location::Bool(value) => vec![Expr::bool(*value)],
        Location::Stack(index) => vec![Expr::ident(stack_name(*index))],
        Location::Named(name) => vec![Expr::ident(yul_var_name(name))],
        Location::Seq(locs) => locs.iter().flat_map(flatten_rhs).collect(),
        Location::Empty(size) => (0..*size).map(|_| Expr::number("911")).collect(),
    }
}

fn flatten_lhs(loc: &Location) -> Result<Vec<String>, TranslationError> {
    match loc {
        Location::Stack(index) => Ok(vec![stack_name(*index)]),
        Location::Named(name) => Ok(vec![yul_var_name(name)]),
        Location::Seq(locs) => locs
            .iter()
            .map(flatten_lhs)
            .collect::<Result<Vec<_>, _>>()
            .map(|chunks| chunks.into_iter().flatten().collect()),
        other => Err(TranslationError::new(format!(
            "cannot use location as assignment target: {other:?}"
        ))),
    }
}

fn load_loc(loc: &Location) -> Result<Expr, TranslationError> {
    match loc {
        Location::Word(value) => Ok(Expr::number(value.clone())),
        Location::Bool(value) => Ok(Expr::bool(*value)),
        Location::Stack(index) => Ok(Expr::ident(stack_name(*index))),
        Location::Named(name) => Ok(Expr::ident(yul_var_name(name))),
        Location::Empty(_) => Ok(Expr::number("911")),
        Location::Seq(_) => Err(TranslationError::new(format!(
            "cannot load location: {loc:?}"
        ))),
    }
}

fn copy_locs(lhs: &Location, rhs: &Location) -> Result<Vec<Stmt>, TranslationError> {
    if matches!(lhs, Location::Seq(_)) || matches!(rhs, Location::Seq(_)) {
        let lhs = flatten_locs(lhs);
        let rhs = flatten_locs(rhs);
        if lhs.len() != rhs.len() {
            return Err(TranslationError::new(format!(
                "location copy arity mismatch: lhs={} rhs={}",
                lhs.len(),
                rhs.len()
            )));
        }
        return lhs
            .into_iter()
            .zip(rhs)
            .map(|(lhs, rhs)| copy_locs(&lhs, &rhs))
            .collect::<Result<Vec<_>, _>>()
            .map(|chunks| chunks.into_iter().flatten().collect());
    }

    match (lhs, rhs) {
        (Location::Stack(_), Location::Empty(_)) | (Location::Named(_), Location::Empty(_)) => {
            Ok(Vec::new())
        }
        (Location::Stack(index), rhs) => Ok(vec![Stmt::Assign {
            names: vec![stack_name(*index)],
            value: load_loc(rhs)?,
        }]),
        (Location::Named(name), rhs) => Ok(vec![Stmt::Assign {
            names: vec![yul_var_name(name)],
            value: load_loc(rhs)?,
        }]),
        _ => Err(TranslationError::new(format!(
            "location copy mismatch: lhs={lhs:?} rhs={rhs:?}"
        ))),
    }
}

fn flatten_locs(loc: &Location) -> Vec<Location> {
    match loc {
        Location::Empty(size) => (0..*size).map(|_| Location::Empty(1)).collect(),
        Location::Seq(locs) => locs.iter().flat_map(flatten_locs).collect(),
        loc => vec![loc.clone()],
    }
}

fn normalize_loc(loc: Location) -> Location {
    match loc {
        Location::Seq(_) => {
            let flattened = flatten_locs(&loc);
            match flattened.as_slice() {
                [one] => one.clone(),
                _ => Location::Seq(flattened),
            }
        }
        loc => loc,
    }
}

fn pair_locs(loc: Location) -> Result<(Location, Location), TranslationError> {
    match loc {
        Location::Seq(mut locs) if locs.len() == 2 => {
            let rhs = locs.pop().expect("rhs");
            let lhs = locs.pop().expect("lhs");
            Ok((lhs, rhs))
        }
        loc => Err(TranslationError::new(format!(
            "expected product location, got {loc:?}"
        ))),
    }
}

fn pad_to_size(loc: Location, size: usize) -> Location {
    let padding = size.saturating_sub(size_of_loc(&loc));
    if padding == 0 {
        loc
    } else {
        Location::Seq(vec![loc, Location::Empty(padding)])
    }
}

fn reshape_loc<'db>(ty: &HullTy<'db>, loc: &Location) -> Result<Location, TranslationError> {
    fn go<'db>(
        ty: &HullTy<'db>,
        slots: &[Location],
    ) -> Result<(Location, usize), TranslationError> {
        match &ty.strip_named().kind {
            TyKind::Named { inner, .. } => go(inner, slots),
            TyKind::Unit => Ok((Location::Seq(Vec::new()), 0)),
            TyKind::Product(lhs, rhs) => {
                let (lhs_loc, lhs_used) = go(lhs, slots)?;
                let (rhs_loc, rhs_used) = go(rhs, &slots[lhs_used..])?;
                Ok((Location::Seq(vec![lhs_loc, rhs_loc]), lhs_used + rhs_used))
            }
            _ => {
                let size = size_of_ty(ty)?;
                let here = slots.iter().take(size).cloned().collect::<Vec<_>>();
                let loc = match here.as_slice() {
                    [one] => one.clone(),
                    _ => Location::Seq(here),
                };
                Ok((loc, size))
            }
        }
    }

    let slots = flatten_locs(loc);
    let (loc, _) = go(ty, &slots)?;
    Ok(loc)
}

fn con_payload<'db>(
    target: &HullTy<'db>,
    con: Con,
    payload: &Location,
) -> Result<Location, TranslationError> {
    match (&target.strip_named().kind, con) {
        (TyKind::Named { inner, .. }, con) => con_payload(inner, con, payload),
        (TyKind::Sum(lhs, _), Con::Inl) => reshape_loc(lhs, payload),
        (TyKind::Sum(_, rhs), Con::Inr) => reshape_loc(rhs, payload),
        (_, Con::InK(index)) => {
            let Some(ty) = nth_sum_payload(target, index) else {
                return Ok(payload.clone());
            };
            reshape_loc(&ty, payload)
        }
        _ => Ok(payload.clone()),
    }
}

fn nth_sum_payload<'db>(target: &HullTy<'db>, index: usize) -> Option<HullTy<'db>> {
    let mut current = target.strip_named();
    let mut remaining = index;
    loop {
        match &current.strip_named().kind {
            TyKind::Sum(lhs, _) if remaining == 0 => return Some((**lhs).clone()),
            TyKind::Sum(_, rhs) => {
                current = rhs.strip_named();
                remaining -= 1;
            }
            _ if remaining == 0 => return Some(current.clone()),
            _ => return None,
        }
    }
}

fn con_lit(target: &HullTy<'_>, con: Con) -> Result<Literal, TranslationError> {
    match con {
        Con::Inl => Ok(Literal::Bool(false)),
        Con::Inr => Ok(Literal::Bool(true)),
        Con::InK(index) if matches!(target.strip_named().kind, TyKind::Sum(_, _)) => {
            Err(TranslationError::new(format!(
                "in({index}) patterns require nested binary inl/inr matches"
            )))
        }
        Con::InK(index) => Ok(Literal::Number(index.to_string())),
    }
}

fn partition_allocs(stmts: Vec<Stmt>) -> (Vec<Stmt>, Vec<Stmt>) {
    stmts
        .into_iter()
        .partition(|stmt| matches!(stmt, Stmt::Let { init: None, .. }))
}

fn is_unit_loc(loc: &Location) -> bool {
    matches!(loc, Location::Seq(locs) if locs.is_empty())
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

fn convert_yul_lit(lit: &YulLitKind) -> Result<Literal, TranslationError> {
    Ok(match lit {
        YulLitKind::Number(value) => Literal::Number(canonical_numeric_lit(value)?),
        YulLitKind::Hex(value) => Literal::Hex(canonical_hex_lit(value)?),
        YulLitKind::String(value) => Literal::String(strip_quotes(value).to_owned()),
        YulLitKind::Bool(value) => Literal::Bool(*value),
        YulLitKind::Error => Literal::Number("0".to_owned()),
    })
}

fn strip_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn yul_name<'db>(
    db: &'db dyn HirDb,
    name: &hir::span::SpannedElem<'db, hir::ast::Ident<'db>>,
) -> String {
    (*name.atom()).text(db).to_owned()
}

fn canonical_decimal_lit(value: &str) -> Result<String, TranslationError> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(TranslationError::new(format!(
            "invalid decimal Yul literal `{value}`"
        )));
    }
    let trimmed = value.trim_start_matches('0');
    Ok(if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    })
}

fn canonical_numeric_lit(value: &str) -> Result<String, TranslationError> {
    if value.starts_with("0x") || value.starts_with("0X") {
        canonical_hex_lit(value)
    } else {
        canonical_decimal_lit(value)
    }
}

fn canonical_word_lit(value: &str) -> Result<String, TranslationError> {
    let wrapped = wrap_word_literal(value).map_err(|err| TranslationError::new(err.to_string()))?;
    canonical_numeric_lit(&wrapped)
}

fn canonical_hex_lit(value: &str) -> Result<String, TranslationError> {
    let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    else {
        return Err(TranslationError::new(format!(
            "hex Yul literal `{value}` must use a 0x prefix"
        )));
    };
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(TranslationError::new(format!(
            "invalid hex Yul literal `{value}`"
        )));
    }
    Ok(format!("0x{digits}"))
}

fn yul_ident_fragment(source: &str) -> String {
    let mut out = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "anon".to_owned()
    } else {
        out
    }
}

fn is_forbidden_yul_identifier(name: &str) -> bool {
    matches!(
        name,
        "object"
            | "code"
            | "data"
            | "function"
            | "let"
            | "if"
            | "switch"
            | "case"
            | "default"
            | "for"
            | "break"
            | "continue"
            | "leave"
            | "true"
            | "false"
            | "stop"
            | "add"
            | "sub"
            | "mul"
            | "div"
            | "sdiv"
            | "mod"
            | "smod"
            | "exp"
            | "not"
            | "lt"
            | "gt"
            | "slt"
            | "sgt"
            | "eq"
            | "iszero"
            | "and"
            | "or"
            | "xor"
            | "byte"
            | "shl"
            | "shr"
            | "sar"
            | "addmod"
            | "mulmod"
            | "signextend"
            | "keccak256"
            | "pc"
            | "pop"
            | "mload"
            | "mstore"
            | "mstore8"
            | "sload"
            | "sstore"
            | "tload"
            | "tstore"
            | "msize"
            | "gas"
            | "address"
            | "balance"
            | "selfbalance"
            | "caller"
            | "callvalue"
            | "calldataload"
            | "calldatasize"
            | "calldatacopy"
            | "codesize"
            | "codecopy"
            | "extcodesize"
            | "extcodecopy"
            | "returndatasize"
            | "returndatacopy"
            | "extcodehash"
            | "create"
            | "create2"
            | "call"
            | "callcode"
            | "delegatecall"
            | "staticcall"
            | "return"
            | "revert"
            | "selfdestruct"
            | "invalid"
            | "log0"
            | "log1"
            | "log2"
            | "log3"
            | "log4"
            | "chainid"
            | "origin"
            | "gasprice"
            | "blockhash"
            | "coinbase"
            | "timestamp"
            | "number"
            | "difficulty"
            | "prevrandao"
            | "gaslimit"
            | "basefee"
            | "blobhash"
            | "blobbasefee"
            | "memoryguard"
            | "dataoffset"
            | "datasize"
            | "datacopy"
            | "setimmutable"
            | "loadimmutable"
            | "linkersymbol"
            | "mcopy"
            | "clz"
    )
}
