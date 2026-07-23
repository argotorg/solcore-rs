use super::*;

pub(super) struct StorageField {
    slot: usize,
    pub(super) kind: StorageFieldKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageFieldKind {
    DirectWord,
    Mapping,
}

impl<'db> Emitter<'db> {
    pub(super) fn contract_storage_fields(
        &mut self,
        def: DefId<'db>,
    ) -> BTreeMap<String, StorageField> {
        let module = parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
        let Some(contract) = find_contract(self.db, module, def) else {
            return BTreeMap::new();
        };
        let resolutions = nameres::module_id_for_source_file(self.db, def.file(self.db))
            .map(|module_id| {
                let env = nameres::module_env_for_hir_module(self.db, module_id, module);
                let scope = env
                    .item_scope
                    .clone()
                    .unwrap_or_else(|| hir::nameres::item_scope(self.db, module));
                hir::nameres::resolve_item_types_with_imports(self.db, module, &scope, &env)
            })
            .unwrap_or_else(|| hir::nameres::resolve_item_types(self.db, module));
        let lowerer =
            TypeLowering::from_item_resolutions(self.db, &resolutions, BinderEnv::empty());
        let mut fields = BTreeMap::new();
        for (slot, field) in contract.fields(self.db).iter().enumerate() {
            let kind = field_storage_kind(self.db, field.ty()).or_else(|| {
                let ty = lowerer.lower_field(field).ty;
                self.user_adt_storage_field_kind(ty, field.ty().span(self.db))
            });
            if let Some(kind) = kind {
                fields.insert(
                    field.name().atom().text(self.db).to_owned(),
                    StorageField { slot, kind },
                );
            }
        }
        fields
    }

    fn user_adt_storage_field_kind(
        &mut self,
        ty: SemTy<'db>,
        span: Span<'db>,
    ) -> Option<StorageFieldKind> {
        let SemTyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } = ty.kind(self.db)
        else {
            return None;
        };
        match user.kind {
            UserTyCtorKind::ValueType if args.is_empty() => {
                let underlying = value_type_underlying(self.db, user.def).ok()?;
                let lowered = self.try_hull_ty(underlying, span);
                if lowered
                    .as_ref()
                    .is_some_and(|ty| matches!(ty.strip_named().kind, TyKind::Word))
                {
                    return Some(StorageFieldKind::DirectWord);
                }
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedType {
                        ty: ty.display(self.db),
                    },
                );
                None
            }
            UserTyCtorKind::Adt => {
                let ty = self.try_hull_ty(ty, span)?;
                (hull_ty_word_slots(&ty) == Some(1)).then_some(StorageFieldKind::DirectWord)
            }
            UserTyCtorKind::Alias | UserTyCtorKind::Contract | UserTyCtorKind::ValueType => None,
        }
    }

    pub(super) fn lower_storage_fields_in_function(
        &self,
        mut function: Function<'db>,
        fields: &BTreeMap<String, StorageField>,
        storage_hash_helper: Option<&str>,
        mapping_value_helper_used: &mut bool,
    ) -> Function<'db> {
        if fields.is_empty() {
            return function;
        }
        let mut lowerer = StorageLowerer::new(self, fields, storage_hash_helper, &function.args);
        function.body = lowerer.stmts(function.body);
        *mapping_value_helper_used |= lowerer.mapping_value_helper_used;
        function
    }

    pub(super) fn storage_hash2_function(&self, span: Span<'db>, name: &str) -> Function<'db> {
        let word = Ty::word(span);
        Function {
            span,
            name: name.into(),
            args: vec![
                Arg {
                    span,
                    name: "x".into(),
                    ty: word.clone(),
                },
                Arg {
                    span,
                    name: "y".into(),
                    ty: word.clone(),
                },
            ],
            ret: word.clone(),
            body: vec![
                Stmt {
                    span,
                    kind: StmtKind::Let {
                        name: "out".into(),
                        ty: word.clone(),
                    },
                },
                self.assembly_stmt(
                    span,
                    vec![
                        self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "mstore",
                                vec![self.yul_number(span, "0"), self.yul_ident_expr(span, "x")],
                            ),
                        ),
                        self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "mstore",
                                vec![self.yul_number(span, "32"), self.yul_ident_expr(span, "y")],
                            ),
                        ),
                        self.yul_assign(
                            span,
                            "out",
                            self.yul_call(
                                span,
                                "keccak256",
                                vec![self.yul_number(span, "0"), self.yul_number(span, "64")],
                            ),
                        ),
                    ],
                ),
                Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, "out", word)),
                },
            ],
        }
    }

    /// Mirrors the reference std's `storage(mapping(k, v)) : CanStore`
    /// instance, whose `load`/`store` bodies are `unimplemented()`: touching a
    /// whole mapping field as a value compiles, but reverts at runtime with
    /// the std `Unimplemented` error, nominally yielding the field's base
    /// slot (the storage reference).
    pub(super) fn storage_mapping_value_function(
        &self,
        span: Span<'db>,
        name: &str,
    ) -> Function<'db> {
        let word = Ty::word(span);
        Function {
            span,
            name: name.into(),
            args: vec![Arg {
                span,
                name: "slot".into(),
                ty: word.clone(),
            }],
            ret: word.clone(),
            body: vec![
                self.assembly_stmt(
                    span,
                    vec![
                        self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "mstore",
                                vec![
                                    self.yul_number(span, "0"),
                                    self.yul_number(span, UNIMPLEMENTED_SELECTOR),
                                ],
                            ),
                        ),
                        self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "revert",
                                vec![self.yul_number(span, "28"), self.yul_number(span, "4")],
                            ),
                        ),
                    ],
                ),
                Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, "slot", word)),
                },
            ],
        }
    }
}

struct StorageLowerer<'a, 'db> {
    emitter: &'a Emitter<'db>,
    fields: &'a BTreeMap<String, StorageField>,
    storage_hash_helper: Option<&'a str>,
    shadows: ScopeStack<BTreeSet<String>>,
    fresh: usize,
    mapping_value_helper_used: bool,
}

impl<'a, 'db> StorageLowerer<'a, 'db> {
    fn new(
        emitter: &'a Emitter<'db>,
        fields: &'a BTreeMap<String, StorageField>,
        storage_hash_helper: Option<&'a str>,
        args: &[Arg<'db>],
    ) -> Self {
        Self {
            emitter,
            fields,
            storage_hash_helper,
            shadows: ScopeStack::new_root_with_message(
                args.iter()
                    .map(|arg| arg.name.as_str().to_owned())
                    .collect(),
                "storage scope stack is never empty",
            ),
            fresh: 0,
            mapping_value_helper_used: false,
        }
    }

    fn stmts(&mut self, stmts: Vec<Stmt<'db>>) -> Vec<Stmt<'db>> {
        let mut out = Vec::new();
        for stmt in stmts {
            out.extend(self.stmt(stmt));
        }
        out
    }

    fn stmt(&mut self, stmt: Stmt<'db>) -> Vec<Stmt<'db>> {
        match stmt.kind {
            StmtKind::Let { name, ty } => {
                self.shadows.last_mut().insert(name.as_str().to_owned());
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Let { name, ty },
                }]
            }
            StmtKind::Assign { lhs, rhs } => {
                if let ExprKind::Var(name) = &lhs.kind
                    && let Some(slot) = self.direct_field(name.as_str()).map(|field| field.slot)
                {
                    let rhs = self.expr(rhs);
                    let temp = self.fresh_temp(name.as_str());
                    return vec![
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: temp.clone().into(),
                                ty: lhs.ty.clone(),
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Assign {
                                lhs: Expr::var(stmt.span, temp.clone(), lhs.ty),
                                rhs,
                            },
                        },
                        self.emitter.assembly_stmt(
                            stmt.span,
                            vec![self.emitter.yul_expr_stmt(
                                stmt.span,
                                self.emitter.yul_call(
                                    stmt.span,
                                    "sstore",
                                    vec![
                                        self.emitter.yul_number(stmt.span, slot.to_string()),
                                        self.emitter.yul_ident_expr(stmt.span, &temp),
                                    ],
                                ),
                            )],
                        ),
                    ];
                }
                if let ExprKind::Var(name) = &lhs.kind
                    && let Some(slot) = self.mapping_field(name.as_str()).map(|field| field.slot)
                {
                    // A whole mapping field as an assignment target: the
                    // reference compiles this via `CanStore.store`, which
                    // evaluates the rhs and then hits an `unimplemented()`
                    // runtime trap.
                    self.mapping_value_helper_used = true;
                    let rhs = self.expr(rhs);
                    let temp = self.fresh_temp(name.as_str());
                    let trap = self.fresh_temp(name.as_str());
                    let word = Ty::word(stmt.span);
                    return vec![
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: temp.clone().into(),
                                ty: lhs.ty.clone(),
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Assign {
                                lhs: Expr::var(stmt.span, temp, lhs.ty),
                                rhs,
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: trap.clone().into(),
                                ty: word.clone(),
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Assign {
                                lhs: Expr::var(stmt.span, trap, word.clone()),
                                rhs: Expr {
                                    span: stmt.span,
                                    ty: word,
                                    kind: ExprKind::Call {
                                        callee: STORAGE_MAPPING_VALUE_HELPER.into(),
                                        args: vec![Expr::word(stmt.span, slot.to_string())],
                                    },
                                },
                            },
                        },
                    ];
                }
                if let Some(slot) = self.storage_index_read_slot(&lhs) {
                    let lowered_slot = self.expr(slot.clone());
                    let slot_temp = self.fresh_temp("storage_index_slot");
                    let slot_ref = Expr::var(stmt.span, slot_temp.clone(), Ty::word(stmt.span));
                    let rhs = replace_storage_index_read_slot(rhs, &slot, &slot_ref);
                    let rhs = self.expr(rhs);
                    let value_temp = self.fresh_temp("storage_index");
                    return vec![
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: slot_temp.clone().into(),
                                ty: Ty::word(stmt.span),
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Assign {
                                lhs: slot_ref.clone(),
                                rhs: lowered_slot,
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: value_temp.clone().into(),
                                ty: lhs.ty.clone(),
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Assign {
                                lhs: Expr::var(stmt.span, value_temp.clone(), lhs.ty),
                                rhs,
                            },
                        },
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Expr(Expr {
                                span: stmt.span,
                                ty: Ty::unit(stmt.span),
                                kind: ExprKind::Call {
                                    callee: "sstore".into(),
                                    args: vec![
                                        slot_ref,
                                        Expr::var(stmt.span, value_temp, Ty::word(stmt.span)),
                                    ],
                                },
                            }),
                        },
                    ];
                }
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Assign {
                        lhs: self.expr(lhs),
                        rhs: self.expr(rhs),
                    },
                }]
            }
            StmtKind::Expr(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Expr(self.expr(expr)),
            }],
            StmtKind::Return(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Return(self.expr(expr)),
            }],
            StmtKind::Block(body) => self.with_scope(|this| {
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Block(this.stmts(body)),
                }]
            }),
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => self.with_scope(|this| {
                let init = this.stmts(init);
                let cond = this.expr(cond);
                let post = this.stmts(post);
                let body = this.stmts(body);
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::For {
                        init,
                        cond,
                        post,
                        body,
                    },
                }]
            }),
            StmtKind::Match {
                target,
                scrutinee,
                alts,
            } => {
                let scrutinee = self.expr(scrutinee);
                let alts = alts
                    .into_iter()
                    .map(|alt| self.alt(alt))
                    .collect::<Vec<_>>();
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Match {
                        target,
                        scrutinee,
                        alts,
                    },
                }]
            }
            kind @ (StmtKind::Assembly(_)
            | StmtKind::Revert(_)
            | StmtKind::Comment(_)
            | StmtKind::Break
            | StmtKind::Continue) => vec![Stmt {
                span: stmt.span,
                kind,
            }],
        }
    }

    fn alt(&mut self, alt: Alt<'db>) -> Alt<'db> {
        self.with_scope(|this| {
            this.shadows
                .last_mut()
                .insert(alt.binder.as_str().to_owned());
            Alt {
                span: alt.span,
                pat: alt.pat,
                binder: alt.binder,
                body: this.stmts(alt.body),
            }
        })
    }

    fn expr(&mut self, expr: Expr<'db>) -> Expr<'db> {
        match expr.kind {
            ExprKind::Var(name) => {
                if let Some(slot) = self.direct_field(name.as_str()).map(|field| field.slot) {
                    Expr {
                        span: expr.span,
                        ty: expr.ty,
                        kind: ExprKind::Call {
                            callee: "sload".into(),
                            args: vec![Expr::word(expr.span, slot.to_string())],
                        },
                    }
                } else if let Some(slot) = self.mapping_field(name.as_str()).map(|field| field.slot)
                {
                    // A whole mapping field read as a value: the reference
                    // compiles this via `CanStore.load`, which is an
                    // `unimplemented()` runtime trap returning the base slot.
                    self.mapping_value_helper_used = true;
                    Expr {
                        span: expr.span,
                        ty: expr.ty,
                        kind: ExprKind::Call {
                            callee: STORAGE_MAPPING_VALUE_HELPER.into(),
                            args: vec![Expr::word(expr.span, slot.to_string())],
                        },
                    }
                } else {
                    Expr {
                        span: expr.span,
                        ty: expr.ty,
                        kind: ExprKind::Var(name),
                    }
                }
            }
            ExprKind::Call { callee, args }
                if callee.as_str() == STORAGE_INDEX_READ && args.len() == 1 =>
            {
                let mut args = args.into_iter();
                let slot = self.expr(args.next().expect("checked len"));
                Expr {
                    span: expr.span,
                    ty: expr.ty,
                    kind: ExprKind::Call {
                        callee: "sload".into(),
                        args: vec![slot],
                    },
                }
            }
            ExprKind::Call { callee, args }
                if callee.as_str() == STORAGE_INDEX_SLOT && args.len() == 2 =>
            {
                let mut args = args.into_iter();
                let base = args.next().expect("checked len");
                let index = args.next().expect("checked len");
                self.storage_index_slot_expr(expr.span, expr.ty, base, index)
            }
            ExprKind::Pair(lhs, rhs) => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::Pair(Box::new(self.expr(*lhs)), Box::new(self.expr(*rhs))),
            },
            ExprKind::Fst(inner) => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::Fst(Box::new(self.expr(*inner))),
            },
            ExprKind::Snd(inner) => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::Snd(Box::new(self.expr(*inner))),
            },
            ExprKind::Inl { target, value } => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::Inl {
                    target,
                    value: Box::new(self.expr(*value)),
                },
            },
            ExprKind::Inr { target, value } => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::Inr {
                    target,
                    value: Box::new(self.expr(*value)),
                },
            },
            ExprKind::InK {
                index,
                target,
                value,
            } => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::InK {
                    index,
                    target,
                    value: Box::new(self.expr(*value)),
                },
            },
            ExprKind::Call { callee, args } => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::Call {
                    callee,
                    args: args.into_iter().map(|arg| self.expr(arg)).collect(),
                },
            },
            ExprKind::If {
                target,
                cond,
                then_expr,
                else_expr,
            } => Expr {
                span: expr.span,
                ty: expr.ty,
                kind: ExprKind::If {
                    target,
                    cond: Box::new(self.expr(*cond)),
                    then_expr: Box::new(self.expr(*then_expr)),
                    else_expr: Box::new(self.expr(*else_expr)),
                },
            },
            ExprKind::Word(_) | ExprKind::Bool(_) | ExprKind::Unit => expr,
        }
    }

    fn field(&self, name: &str) -> Option<&StorageField> {
        if self.shadows.iter().rev().any(|scope| scope.contains(name)) {
            return None;
        }
        self.fields.get(name)
    }

    fn direct_field(&self, name: &str) -> Option<&StorageField> {
        self.field(name)
            .filter(|field| field.kind == StorageFieldKind::DirectWord)
    }

    fn mapping_field(&self, name: &str) -> Option<&StorageField> {
        self.field(name)
            .filter(|field| field.kind == StorageFieldKind::Mapping)
    }

    fn storage_index_read_slot(&self, expr: &Expr<'db>) -> Option<Expr<'db>> {
        let ExprKind::Call { callee, args } = &expr.kind else {
            return None;
        };
        if callee.as_str() != STORAGE_INDEX_READ || args.len() != 1 {
            return None;
        }
        args.first().cloned()
    }

    fn storage_index_slot_expr(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        base: Expr<'db>,
        index: Expr<'db>,
    ) -> Expr<'db> {
        let base = self.storage_slot_base_expr(base);
        let index = self.expr(index);
        Expr {
            span,
            ty,
            kind: ExprKind::Call {
                callee: self
                    .storage_hash_helper
                    .unwrap_or(STORAGE_HASH2_HELPER)
                    .into(),
                args: vec![base, index],
            },
        }
    }

    fn storage_slot_base_expr(&mut self, base: Expr<'db>) -> Expr<'db> {
        match base.kind {
            ExprKind::Var(name) => {
                if let Some(slot) = self.field(name.as_str()).map(|field| field.slot) {
                    Expr::word(base.span, slot.to_string())
                } else {
                    Expr {
                        span: base.span,
                        ty: base.ty,
                        kind: ExprKind::Var(name),
                    }
                }
            }
            ExprKind::Call { callee, args }
                if callee.as_str() == STORAGE_INDEX_SLOT && args.len() == 2 =>
            {
                let mut args = args.into_iter();
                let nested_base = args.next().expect("checked len");
                let nested_index = args.next().expect("checked len");
                self.storage_index_slot_expr(base.span, base.ty, nested_base, nested_index)
            }
            _ => self.expr(base),
        }
    }

    fn fresh_temp(&mut self, field: &str) -> String {
        let name = format!("storage_store_{field}_{}", self.fresh);
        self.fresh += 1;
        name
    }

    fn with_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.shadows.push(BTreeSet::new());
        let out = f(self);
        let _ = self.shadows.pop();
        out
    }
}

fn replace_storage_index_read_slot<'db>(
    expr: Expr<'db>,
    slot: &Expr<'db>,
    slot_ref: &Expr<'db>,
) -> Expr<'db> {
    if let ExprKind::Call { callee, args } = &expr.kind
        && callee.as_str() == STORAGE_INDEX_READ
        && args.len() == 1
        && args.first() == Some(slot)
    {
        return Expr {
            span: expr.span,
            ty: expr.ty,
            kind: ExprKind::Call {
                callee: "sload".into(),
                args: vec![slot_ref.clone()],
            },
        };
    }

    Expr {
        span: expr.span,
        ty: expr.ty,
        kind: match expr.kind {
            ExprKind::Pair(lhs, rhs) => ExprKind::Pair(
                Box::new(replace_storage_index_read_slot(*lhs, slot, slot_ref)),
                Box::new(replace_storage_index_read_slot(*rhs, slot, slot_ref)),
            ),
            ExprKind::Fst(inner) => ExprKind::Fst(Box::new(replace_storage_index_read_slot(
                *inner, slot, slot_ref,
            ))),
            ExprKind::Snd(inner) => ExprKind::Snd(Box::new(replace_storage_index_read_slot(
                *inner, slot, slot_ref,
            ))),
            ExprKind::Inl { target, value } => ExprKind::Inl {
                target,
                value: Box::new(replace_storage_index_read_slot(*value, slot, slot_ref)),
            },
            ExprKind::Inr { target, value } => ExprKind::Inr {
                target,
                value: Box::new(replace_storage_index_read_slot(*value, slot, slot_ref)),
            },
            ExprKind::InK {
                index,
                target,
                value,
            } => ExprKind::InK {
                index,
                target,
                value: Box::new(replace_storage_index_read_slot(*value, slot, slot_ref)),
            },
            ExprKind::Call { callee, args } => ExprKind::Call {
                callee,
                args: args
                    .into_iter()
                    .map(|arg| replace_storage_index_read_slot(arg, slot, slot_ref))
                    .collect(),
            },
            ExprKind::If {
                target,
                cond,
                then_expr,
                else_expr,
            } => ExprKind::If {
                target,
                cond: Box::new(replace_storage_index_read_slot(*cond, slot, slot_ref)),
                then_expr: Box::new(replace_storage_index_read_slot(*then_expr, slot, slot_ref)),
                else_expr: Box::new(replace_storage_index_read_slot(*else_expr, slot, slot_ref)),
            },
            ExprKind::Word(value) => ExprKind::Word(value),
            ExprKind::Bool(value) => ExprKind::Bool(value),
            ExprKind::Unit => ExprKind::Unit,
            ExprKind::Var(name) => ExprKind::Var(name),
        },
    }
}

fn field_storage_kind<'db>(
    db: &'db dyn HirDb,
    ty: hir::ast::ty::TypeRef<'db>,
) -> Option<StorageFieldKind> {
    let TypeRefKind::Named { name, args, .. } = ty.kind(db) else {
        return None;
    };
    let name = name.atom().text(db);
    if args.atom().is_empty() && matches!(name, "word" | "uint" | "uint256" | "bytes32" | "address")
    {
        return Some(StorageFieldKind::DirectWord);
    }
    if name == "mapping" && args.atom().len() == 2 {
        return Some(StorageFieldKind::Mapping);
    }
    None
}

fn find_contract<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ContractDef<'db>> {
    module.items(db).iter().find_map(|item| match item {
        Item::ContractDef(contract) if contract.def_id_value(db) == def => Some(*contract),
        _ => None,
    })
}
