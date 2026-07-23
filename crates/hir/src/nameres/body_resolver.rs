use super::*;

pub(super) struct BodyResolver<'db, 'a> {
    db: &'db dyn Db,
    scope: &'a ItemScopeFacts<'db>,
    imports: &'a dyn ImportedNames<'db>,
    contract: Option<DefId<'db>>,
    local_scopes: Vec<FxHashMap<String, Resolution<'db>>>,
    type_vars: Vec<TypeVarBinding<'db>>,
    pub(super) map: BodyResolutionMap<'db>,
}

impl<'db, 'a> BodyResolver<'db, 'a> {
    pub(super) fn new(
        db: &'db dyn Db,
        scope: &'a ItemScopeFacts<'db>,
        imports: &'a dyn ImportedNames<'db>,
        contract: Option<DefId<'db>>,
    ) -> Self {
        Self {
            db,
            scope,
            imports,
            contract,
            local_scopes: Vec::new(),
            type_vars: Vec::new(),
            map: BodyResolutionMap::default(),
        }
    }

    pub(super) fn body(&mut self, body: FuncBody<'db>) {
        for stmt in body.top_level_stmts(self.db) {
            self.stmt(body, *stmt);
        }
    }

    fn stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) {
        let stmt = body.stmts(self.db).get(stmt_id);
        match &stmt.kind {
            StmtKind::Let { name, ty, init, .. } => {
                if let Some(ty) = ty {
                    self.ty(*ty);
                }
                if let Some(init) = init {
                    // Reference semantics: a let initializer is evaluated in
                    // the pre-binder scope, so the new local is inserted after
                    // the initializer has been resolved.
                    self.expr(body, *init);
                }
                let resolution = Resolution::Local(LocalBinding::Let {
                    body,
                    stmt: stmt_id,
                });
                self.add_local(ident_text_str(self.db, name), resolution.clone());
                self.map.record_stmt(body, stmt_id, resolution);
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(body, *expr);
                }
            }
            StmtKind::Expr(expr) => self.expr(body, *expr),
            StmtKind::Assign { lhs, rhs, .. } => {
                self.expr(body, *lhs);
                self.expr(body, *rhs);
            }
            StmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.expr(body, *scrutinee);
                }
                for arm in arms {
                    self.match_arm(body, arm);
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                // `for` does not create a lexical scope; initializer, condition,
                // post statements, and body share the surrounding scope.
                for stmt in init {
                    self.stmt(body, *stmt);
                }
                self.expr(body, *cond);
                for stmt in post {
                    self.stmt(body, *stmt);
                }
                for stmt in for_body {
                    self.stmt(body, *stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.expr(body, *cond);
                for stmt in then_body {
                    self.stmt(body, *stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.stmt(body, *stmt);
                    }
                }
            }
            StmtKind::Block { body: block } => {
                self.with_scope(|resolver| {
                    for stmt in block {
                        resolver.stmt(body, *stmt);
                    }
                });
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn match_arm(&mut self, body: FuncBody<'db>, arm: &MatchArm<'db>) {
        self.with_scope(|resolver| {
            for pat in &arm.pats {
                resolver.pat(body, *pat);
            }
            for stmt in &arm.body {
                resolver.stmt(body, *stmt);
            }
        });
    }

    fn expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Lit(_) => {}
            ExprKind::Error => {
                self.map.record_expr(body, expr_id, Resolution::Err);
            }
            ExprKind::Ident(name) => {
                let resolution = self.resolve_ident(name);
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::DotCtor { name, args, .. } => {
                for arg in args {
                    self.expr(body, *arg);
                }
                let leaf = ident_text_str(self.db, name);
                let resolution = if self.has_constructor_leaf(leaf) {
                    Resolution::DotCtorDeferred
                } else if self.imports.may_contain_unknown_unqualified(
                    self.db,
                    Namespace::Term,
                    leaf,
                ) {
                    Resolution::Err
                } else {
                    self.map.diagnostics.push(self.undefined_name_diag(
                        leaf,
                        name.span(self.db),
                        UndefinedNameKind::Other,
                    ));
                    Resolution::Err
                };
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::Proxy { ty, .. } => self.ty(*ty),
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => {
                for param in params.atom() {
                    self.param_type(param);
                }
                if let Some(ret) = ret {
                    self.ty(*ret);
                }
                self.with_scope(|resolver| {
                    for (index, param) in params.atom().iter().enumerate() {
                        if let Some(name) = param_name(param) {
                            resolver.add_param(*lambda_body, index, name);
                        }
                    }
                    resolver.body(*lambda_body);
                });
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(body, *lhs);
                self.expr(body, *rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(body, *base);
                self.expr(body, *index);
            }
            ExprKind::Call { callee, args } => {
                self.call_callee(body, *callee);
                for arg in args {
                    self.expr(body, *arg);
                }
            }
            ExprKind::Field { base, field } => {
                if self.is_namespace_qualifier(body, *base) {
                    let access_path =
                        expr_path(self.db, body, expr_id).map(|segments| segments.join("."));
                    self.expr_as_qualifier(body, *base, access_path.as_deref());
                } else {
                    self.expr(body, *base);
                }
                if let Some(resolution) = self.resolve_field_expr(body, *base, field) {
                    self.map.record_expr(body, expr_id, resolution);
                }
            }
            ExprKind::Conversion { expr, ty } | ExprKind::TypeAscription { expr, ty } => {
                self.expr(body, *expr);
                self.ty(*ty);
            }
            ExprKind::UnaryOp { expr, .. } => self.expr(body, *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(body, *cond);
                self.expr(body, *then_expr);
                self.expr(body, *else_expr);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.expr(body, *elem);
                }
            }
        }
    }

    fn pat(&mut self, body: FuncBody<'db>, pat_id: Id<Pat<'db>>) {
        let pat = body.pats(self.db).get(pat_id);
        match &pat.kind {
            PatKind::Wildcard | PatKind::Lit(_) => {}
            PatKind::Error => {
                self.map.record_pat(body, pat_id, Resolution::Err);
            }
            PatKind::Var(name) => {
                let leaf = ident_text_str(self.db, name);
                let resolution = if let Some(
                    res @ Resolution::Builtin(BuiltinKind::Constructor(
                        BuiltinCtor::True | BuiltinCtor::False,
                    )),
                ) = builtin_term(leaf)
                {
                    res
                } else if self.has_user_constructor_leaf(leaf) {
                    // Every in-scope user constructor must be written
                    // qualified; silently binding it as a variable would turn
                    // the arm into a catch-all.
                    self.map.diagnostics.push(unqualified_constructor(
                        self.db,
                        leaf,
                        name.span(self.db),
                        self.constructor_qualification(leaf),
                    ));
                    Resolution::Err
                } else {
                    let resolution = Resolution::Local(LocalBinding::Pattern { body, pat: pat_id });
                    self.add_local(leaf, resolution.clone());
                    resolution
                };
                self.map.record_pat(body, pat_id, resolution);
            }
            PatKind::Ctor { head, args } => {
                for arg in args {
                    self.pat(body, *arg);
                }
                let resolution = match head {
                    PatCtorHead::Deferred { .. } => Resolution::DotCtorDeferred,
                    PatCtorHead::Qualified { qualifier, name } => {
                        let qualifier_text = ident_text_str(self.db, qualifier);
                        let qualified = qualify(qualifier_text, ident_text_str(self.db, name));
                        self.lookup_ctor(&qualified).unwrap_or_else(|| {
                            if self
                                .imports
                                .has_incomplete_module_qualifier(self.db, qualifier_text)
                            {
                                return Resolution::Err;
                            }
                            let kind = if self.lookup_type(qualifier_text).is_some()
                                || self.lookup_module(qualifier_text).is_some()
                            {
                                UndefinedNameKind::Field
                            } else {
                                UndefinedNameKind::QualifiedConstructor {
                                    access_path: qualified.clone(),
                                }
                            };
                            self.map.diagnostics.push(self.undefined_name_diag(
                                &qualified,
                                name.span(self.db),
                                kind,
                            ));
                            Resolution::Err
                        })
                    }
                    PatCtorHead::Unqualified { name } => {
                        let leaf = ident_text_str(self.db, name);
                        if self.has_user_constructor_leaf(leaf) {
                            self.map.diagnostics.push(unqualified_constructor(
                                self.db,
                                leaf,
                                name.span(self.db),
                                self.constructor_qualification(leaf),
                            ));
                            Resolution::Err
                        } else if matches!(
                            builtin_term(leaf),
                            Some(Resolution::Builtin(BuiltinKind::Constructor(_)))
                        ) {
                            // Primitive constructors (`pair`, `inl`, ...) stay
                            // legal unqualified; their concrete constructor is
                            // picked from the expected type during inference.
                            Resolution::DotCtorDeferred
                        } else if self.imports.may_contain_unknown_unqualified(
                            self.db,
                            Namespace::Term,
                            leaf,
                        ) {
                            Resolution::Err
                        } else if args.is_empty() {
                            let resolution =
                                Resolution::Local(LocalBinding::Pattern { body, pat: pat_id });
                            self.add_local(leaf, resolution.clone());
                            resolution
                        } else {
                            self.map
                                .diagnostics
                                .push(invalid_pattern(self.db, pat.span));
                            Resolution::Err
                        }
                    }
                };
                self.map.record_pat(body, pat_id, resolution);
            }
            PatKind::ComptimeLabel { expr, .. } => self.expr(body, *expr),
            PatKind::Tuple { elems } => {
                for elem in elems {
                    self.pat(body, *elem);
                }
            }
        }
    }

    fn ty(&mut self, ty: TypeRef<'db>) {
        match ty.kind(self.db) {
            TypeRefKind::Named {
                qualifier,
                name,
                args,
            } => {
                for arg in args.atom() {
                    self.ty(*arg);
                }
                let resolution = if let Some(qualifier) = qualifier {
                    let qualifier_text = ident_text_str(self.db, qualifier);
                    let qualified = qualify(qualifier_text, ident_text_str(self.db, name));
                    self.lookup_type(&qualified).unwrap_or_else(|| {
                        if self
                            .imports
                            .has_incomplete_module_qualifier(self.db, qualifier_text)
                        {
                            return Resolution::Err;
                        }
                        self.map
                            .diagnostics
                            .push(self.undefined_type_ctor_diag(&qualified, name.span(self.db)));
                        Resolution::Err
                    })
                } else {
                    let name_text = ident_text_str(self.db, name);
                    self.lookup_type(name_text).unwrap_or_else(|| {
                        self.map
                            .diagnostics
                            .push(self.undefined_type_ctor_diag(name_text, name.span(self.db)));
                        Resolution::Err
                    })
                };
                self.map.types.push(TypeResolution { ty, resolution });
            }
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.ty(*param);
                }
                self.ty(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.ty(*inner),
            TypeRefKind::Tuple { elems } => {
                for elem in elems.atom() {
                    self.ty(*elem);
                }
            }
            TypeRefKind::Error { .. } => {
                self.map.types.push(TypeResolution {
                    ty,
                    resolution: Resolution::Err,
                });
            }
        }
    }

    fn param_type(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.ty(*ty);
        }
    }

    fn resolve_ident(&mut self, name: &SpannedElem<'db, Ident<'db>>) -> Resolution<'db> {
        let text = ident_text_str(self.db, name);
        if let Some(resolution) = self
            .lookup_local(text)
            // Contract fields intentionally beat same-name functions in the
            // contract term surface.
            .or_else(|| self.lookup_field(text))
            .or_else(|| self.lookup_qualified_term(text))
            .or_else(|| self.lookup_unqualified_class_method(text))
        {
            if matches!(&resolution, Resolution::Ctor { .. }) {
                return self.reject_unqualified_constructor(name);
            }
            return resolution;
        }
        if self.has_user_constructor_leaf(text) {
            return self.reject_unqualified_constructor(name);
        }
        self.lookup_type(text)
            .or_else(|| self.lookup_module(text))
            .unwrap_or_else(|| {
                if self
                    .imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Term, text)
                {
                    return Resolution::Err;
                }
                self.map.diagnostics.push(self.undefined_name_diag(
                    text,
                    name.span(self.db),
                    UndefinedNameKind::Term,
                ));
                Resolution::Err
            })
    }

    fn call_callee(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Ident(name) => {
                let resolution = self.resolve_call_ident(name);
                self.map.record_expr(body, expr_id, resolution);
            }
            _ => self.expr(body, expr_id),
        }
    }

    fn resolve_call_ident(&mut self, name: &SpannedElem<'db, Ident<'db>>) -> Resolution<'db> {
        let text = ident_text_str(self.db, name);
        if let Some(resolution) = self
            .lookup_local(text)
            .or_else(|| self.lookup_qualified_term(text))
            .or_else(|| self.lookup_field(text))
            .or_else(|| self.lookup_unqualified_class_method(text))
        {
            if matches!(&resolution, Resolution::Ctor { .. }) {
                return self.reject_unqualified_constructor(name);
            }
            return resolution;
        }
        self.resolve_ident(name)
    }

    fn reject_unqualified_constructor(
        &mut self,
        name: &SpannedElem<'db, Ident<'db>>,
    ) -> Resolution<'db> {
        let text = ident_text_str(self.db, name);
        self.map.diagnostics.push(unqualified_constructor(
            self.db,
            text,
            name.span(self.db),
            self.constructor_qualification(text),
        ));
        Resolution::Err
    }

    fn expr_as_qualifier(
        &mut self,
        body: FuncBody<'db>,
        expr_id: Id<Expr<'db>>,
        access_path: Option<&str>,
    ) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Ident(name) => {
                let text = ident_text_str(self.db, name);
                let resolution = self
                    .lookup_type(text)
                    .or_else(|| self.lookup_module(text))
                    .or_else(|| self.lookup_qualified_term(text))
                    .unwrap_or_else(|| {
                        if self.imports.may_contain_unknown_unqualified(
                            self.db,
                            Namespace::Module,
                            text,
                        ) {
                            return Resolution::Err;
                        }
                        self.map.diagnostics.push(self.undefined_name_diag(
                            text,
                            name.span(self.db),
                            UndefinedNameKind::ModuleQualifier {
                                access_path: access_path.unwrap_or(text).to_owned(),
                            },
                        ));
                        Resolution::Err
                    });
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::Field { base, field } => {
                self.expr_as_qualifier(body, *base, access_path);
                if let Some(resolution) = self.resolve_field_expr(body, *base, field) {
                    self.map.record_expr(body, expr_id, resolution);
                }
            }
            _ => self.expr(body, expr_id),
        }
    }

    fn resolve_field_expr(
        &mut self,
        body: FuncBody<'db>,
        base: Id<Expr<'db>>,
        field: &SpannedElem<'db, Ident<'db>>,
    ) -> Option<Resolution<'db>> {
        let path = expr_path(self.db, body, base)?;
        let qualifier = path.join(".");
        let field_text = ident_text_str(self.db, field);
        let qualified = qualify(&qualifier, field_text);

        if let Some(resolution) = self.lookup_qualified_term(&qualified) {
            return Some(resolution);
        }

        if let Some(resolution) = self.lookup_type(&qualified) {
            return Some(resolution);
        }

        if matches!(
            self.lookup_type(&qualifier),
            Some(
                Resolution::Def {
                    kind: DefResolutionKind::Adt
                        | DefResolutionKind::Contract
                        | DefResolutionKind::Class
                        | DefResolutionKind::TypeAlias,
                    ..
                } | Resolution::Builtin(BuiltinKind::Type(_) | BuiltinKind::Class(_))
            )
        ) {
            self.map.diagnostics.push(self.undefined_name_diag(
                field_text,
                field.span(self.db),
                UndefinedNameKind::Field,
            ));
            return Some(Resolution::Err);
        }

        if self.lookup_module(&qualifier).is_some() {
            if self.lookup_module(&qualified).is_none() {
                if self
                    .imports
                    .has_incomplete_module_qualifier(self.db, &qualifier)
                {
                    return Some(Resolution::Err);
                }
                let private_candidate = self.imports.private_candidate(
                    self.db,
                    Namespace::Term,
                    &qualifier,
                    field_text,
                );
                self.map
                    .diagnostics
                    .push(self.undefined_name_diag_with_private(
                        field_text,
                        field.span(self.db),
                        UndefinedNameKind::ModuleMember {
                            access_path: qualified,
                        },
                        private_candidate,
                    ));
                return Some(Resolution::Err);
            }
            return Some(Resolution::Module(ModuleRef {
                owner: self.scope.module.def_id_value(self.db),
                name: qualified,
            }));
        }

        None
    }

    fn undefined_name_diag(
        &self,
        name: &str,
        span: Span<'db>,
        kind: UndefinedNameKind,
    ) -> NameresDiagnostic {
        self.undefined_name_diag_with_private(name, span, kind, None)
    }

    fn undefined_name_diag_with_private(
        &self,
        name: &str,
        span: Span<'db>,
        kind: UndefinedNameKind,
        private_candidate: Option<PrivateCandidate>,
    ) -> NameresDiagnostic {
        let suggestion = private_candidate
            .is_none()
            .then(|| best_name_suggestion(name, self.name_candidate_names()))
            .flatten();
        undefined_name(self.db, name, span, kind, suggestion, private_candidate)
    }

    fn undefined_type_ctor_diag(&self, name: &str, span: Span<'db>) -> NameresDiagnostic {
        let constructor_candidate = unique_constructor_type_candidate(
            self.constructor_type_candidates(name)
                .into_iter()
                .filter(|candidate| candidate.ctor_name == name),
        );
        let suggestion = constructor_candidate
            .is_none()
            .then(|| best_name_suggestion(name, self.type_candidate_names()))
            .flatten();
        undefined_type_ctor(self.db, name, span, suggestion, constructor_candidate)
    }

    fn constructor_qualification(&self, leaf: &str) -> Option<String> {
        unique_constructor_type_candidate(
            self.constructor_type_candidates(leaf)
                .into_iter()
                .filter(|candidate| candidate.ctor_name == leaf),
        )
        .map(|candidate| qualify(&candidate.ty_name, &candidate.ctor_name))
    }

    fn name_candidate_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for scope in &self.local_scopes {
            names.extend(scope.keys().cloned());
        }
        if let Some(contract) = self
            .contract
            .and_then(|contract| self.scope.contract_scope(contract))
        {
            names.extend(contract.fields.iter().map(|entry| entry.name.clone()));
            names.extend(contract.terms.iter().map(|entry| entry.name.clone()));
            names.extend(contract.types.iter().map(|entry| entry.name.clone()));
        }
        names.extend(self.scope.terms.iter().map(|entry| entry.name.clone()));
        names.extend(self.scope.types.iter().map(|entry| entry.name.clone()));
        names.extend(self.scope.modules.iter().map(|entry| entry.name.clone()));
        names.extend(self.imports.candidate_names(self.db, Namespace::Term));
        names.extend(self.imports.candidate_names(self.db, Namespace::Type));
        names.extend(self.imports.candidate_names(self.db, Namespace::Module));
        names
    }

    fn type_candidate_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        names.extend(
            self.type_vars
                .iter()
                .map(|var| ident_text_str(self.db, &var.name).to_owned()),
        );
        if let Some(contract) = self
            .contract
            .and_then(|contract| self.scope.contract_scope(contract))
        {
            names.extend(contract.types.iter().map(|entry| entry.name.clone()));
        }
        names.extend(self.scope.types.iter().map(|entry| entry.name.clone()));
        names.extend(self.imports.candidate_names(self.db, Namespace::Type));
        names
    }

    fn constructor_type_candidates(&self, leaf: &str) -> Vec<ConstructorTypeCandidate> {
        let mut candidates = Vec::new();
        if let Some(contract) = self
            .contract
            .and_then(|contract| self.scope.contract_scope(contract))
        {
            collect_constructor_type_candidates(
                self.db,
                &contract.ctor_lists,
                leaf,
                &mut candidates,
            );
        }
        collect_constructor_type_candidates(self.db, &self.scope.ctor_lists, leaf, &mut candidates);
        candidates.extend(self.imports.constructor_type_candidates(self.db, leaf));
        candidates
    }

    fn lookup_qualified_term(&self, name: &str) -> Option<Resolution<'db>> {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .and_then(|contract| contract.term_resolution(name))
            .or_else(|| self.scope.term_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Term, name))
            .or_else(|| builtin_term(name))
    }

    fn lookup_unqualified_class_method(&self, name: &str) -> Option<Resolution<'db>> {
        let mut matches = self
            .scope
            .terms
            .iter()
            .filter(|entry| entry.name.rsplit('.').next() == Some(name))
            .filter_map(|entry| match &entry.resolution {
                Resolution::ClassMethod { .. } => Some(entry.resolution.clone()),
                _ => None,
            });
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }

    fn lookup_ctor(&self, name: &str) -> Option<Resolution<'db>> {
        match self.lookup_qualified_term(name) {
            Some(res @ Resolution::Ctor { .. })
            | Some(res @ Resolution::Builtin(BuiltinKind::Constructor(_))) => Some(res),
            _ => None,
        }
    }

    fn lookup_local(&self, name: &str) -> Option<Resolution<'db>> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn lookup_field(&self, name: &str) -> Option<Resolution<'db>> {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .and_then(|contract| contract.field_resolution(name))
    }

    fn lookup_type(&self, name: &str) -> Option<Resolution<'db>> {
        self.type_vars
            .iter()
            .rev()
            .find(|var| ident_text_str(self.db, &var.name) == name)
            .map(|var| {
                Resolution::Local(LocalBinding::TypeVar(TypeVarId {
                    owner: var.owner,
                    index: var.index,
                    name: name.to_owned(),
                }))
            })
            .or_else(|| {
                self.contract
                    .and_then(|contract| self.scope.contract_scope(contract))
                    .and_then(|contract| contract.type_resolution(name))
            })
            .or_else(|| self.scope.type_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Type, name))
            .or_else(|| builtin_type_or_class(name))
            .or_else(|| {
                self.imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Type, name)
                    .then_some(Resolution::Err)
            })
    }

    fn lookup_module(&self, name: &str) -> Option<Resolution<'db>> {
        self.scope
            .module_resolution(name)
            .or_else(|| self.imports.imported(self.db, Namespace::Module, name))
    }

    fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.has_user_constructor_leaf(leaf)
            || matches!(
                builtin_term(leaf),
                Some(Resolution::Builtin(BuiltinKind::Constructor(_)))
            )
    }

    /// Returns whether any user-declared constructor in scope has this leaf
    /// name, excluding the builtin (primitive) constructors.
    ///
    /// Unqualified references to such constructors are rejected with `SC0106`,
    /// while primitive constructors stay legal unqualified.
    fn has_user_constructor_leaf(&self, leaf: &str) -> bool {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .is_some_and(|contract| contract.has_constructor_leaf(leaf))
            || self.scope.has_constructor_leaf(leaf)
            || self.imports.has_constructor_leaf(self.db, leaf)
    }

    fn is_namespace_qualifier(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        let Some(path) = expr_path(self.db, body, expr) else {
            return false;
        };
        let Some(first) = path.first() else {
            return false;
        };
        if path.len() == 1
            && (self.lookup_local(first).is_some() || self.lookup_field(first).is_some())
        {
            return false;
        }
        if self.lookup_type(first).is_some() || self.lookup_module(first).is_some() {
            return true;
        }

        if self.lookup_local(first).is_some()
            || self.lookup_field(first).is_some()
            || self.lookup_qualified_term(first).is_some()
            || self.lookup_unqualified_class_method(first).is_some()
        {
            return false;
        }

        // A path-shaped expression whose base is still unresolved is not a
        // bare term lookup. Resolve it as a qualifier so typed diagnostics do
        // not expose its first segment as an auto-importable term. If the base
        // later becomes a value, the checks above keep ordinary member access
        // on the value-expression path.
        true
    }

    fn add_local(&mut self, name: &str, resolution: Resolution<'db>) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name.to_owned(), resolution);
        } else {
            let mut scope = FxHashMap::default();
            scope.insert(name.to_owned(), resolution);
            self.local_scopes.push(scope);
        }
    }

    pub(super) fn add_param(
        &mut self,
        body: FuncBody<'db>,
        index: usize,
        name: &SpannedElem<'db, Ident<'db>>,
    ) {
        self.add_local(
            ident_text_str(self.db, name),
            Resolution::Param(ParamId {
                body,
                index: ParamIndex::from_usize(index),
            }),
        );
    }

    pub(super) fn with_scope(&mut self, f: impl FnOnce(&mut Self)) {
        self.local_scopes.push(FxHashMap::default());
        f(self);
        self.local_scopes.pop();
    }

    pub(super) fn with_type_vars(
        &mut self,
        vars: &[TypeVarBinding<'db>],
        f: impl FnOnce(&mut Self),
    ) {
        let old_len = self.type_vars.len();
        self.type_vars.extend_from_slice(vars);
        f(self);
        self.type_vars.truncate(old_len);
    }
}
