use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn infer_struct_field_assign(
        &mut self,
        body: FuncBody<'db>,
        op: AssignOp,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    ) -> bool {
        let ExprKind::Field { base, .. } = body.exprs(self.db).get(lhs).kind else {
            return false;
        };
        // Namespace-qualified contract fields still use the ordinary field path.
        if self.expr_resolutions.contains_key(&(body, lhs)) {
            return false;
        }
        let field_ty = self.infer_expr(body, lhs);
        if let Some(op) = op.binary_op() {
            self.infer_bin_op(body, lhs, lhs, op, rhs, Some(field_ty));
        } else {
            self.infer_expr_expected(body, rhs, Some(field_ty));
        }
        if !self.is_contract_field_assign_target_expr(body, base) {
            self.emit_expr_error(
                body,
                lhs,
                TypeckDiagnostic::Mismatch {
                    span: self.expr_label_span(body, lhs),
                    expected: "struct field of a contract storage variable".to_owned(),
                    actual: "non-storage struct field assignment".to_owned(),
                },
            );
            return true;
        }
        if let Some(hir_nameres::Resolution::Field(field)) =
            self.expr_resolutions.get(&(body, base)).cloned()
            && let Some(storage_ty) = self.infer_storage_ref_expr(body, base, false)
        {
            let value_ty = self.instantiate_field(field, ObligationSource::Scheme);
            self.push_assign_obligation(storage_ty, value_ty, ObligationSource::Scheme);
        }
        true
    }

    pub(super) fn infer_storage_index_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
    ) -> Option<InferTy<'db>> {
        let base_ty = self.infer_storage_ref_expr(body, base, true)?;
        let value_ref_ty = self.infer_storage_index_ref_ty(body, index, base_ty)?;
        Some(self.storage_load_ty(body, expr, value_ref_ty))
    }

    pub(super) fn infer_storage_assign(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    ) -> bool {
        if !self.is_storage_assign_target_expr(body, lhs) {
            return false;
        }
        let Some(lhs_ty) = self.infer_storage_ref_expr(body, lhs, false) else {
            return false;
        };
        let is_contract_field = self.is_contract_field_assign_target_expr(body, lhs);
        let uses_assign = is_contract_field && self.lookup_class_id("Assign").is_some();
        if matches!(body.exprs(self.db).get(rhs).kind, ExprKind::Array(_)) && is_contract_field {
            return self.infer_storage_array_literal_assign(body, lhs, rhs, lhs_ty);
        }
        let expected_rhs = if uses_assign {
            self.engine.fresh_var()
        } else {
            self.loaded_ty_for_storage_ty(lhs_ty.clone())
                .unwrap_or_else(|| self.engine.fresh_var())
        };
        let rhs_ty = self.infer_expr_expected(body, rhs, Some(expected_rhs.clone()));
        self.unify_expr(body, rhs, expected_rhs, rhs_ty.clone());
        if uses_assign {
            // A field write lowers through `Assign.assign`.
            // A specific Assign instance may intentionally accept a value
            // different from the field's ordinary CanStore load type.
            self.push_assign_obligation(lhs_ty, rhs_ty.clone(), ObligationSource::Scheme);
        } else {
            self.push_can_store_obligation(lhs_ty, rhs_ty.clone(), ObligationSource::Scheme);
        }
        self.expr_tys.push((body, lhs, rhs_ty));
        true
    }

    fn is_contract_field_assign_target_expr(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> bool {
        if matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return true;
        }
        match body.exprs(self.db).get(expr).kind {
            ExprKind::TypeAnnot { expr, .. } => {
                self.is_contract_field_assign_target_expr(body, expr)
            }
            _ => false,
        }
    }

    fn is_storage_assign_target_expr(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        if matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return true;
        }
        match body.exprs(self.db).get(expr).kind {
            ExprKind::Index { .. } => true,
            ExprKind::TypeAnnot { expr, .. } => self.is_storage_assign_target_expr(body, expr),
            _ => false,
        }
    }

    /// `m[i] = v` on a memory array writes the element in place, so `v` must
    /// have the element type.
    pub(super) fn infer_memory_array_index_assign(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    ) -> bool {
        let Some((_, base, index)) = self.read_only_array_index_parts(body, lhs) else {
            return false;
        };
        let base_ty = self.infer_expr(body, base);
        let Some(elem_ty) = self.memory_dyn_array_elem_ty(base_ty) else {
            return false;
        };
        let index_ty = self.infer_expr(body, index);
        self.push_typedef_word_obligation(index_ty, ObligationSource::Scheme);
        self.push_typedef_word_obligation(elem_ty.clone(), ObligationSource::Scheme);
        let lhs_ty = self.record_read_only_array_index_lhs_ty(body, lhs, elem_ty);
        let rhs_ty = self.infer_expr_expected(body, rhs, Some(lhs_ty.clone()));
        self.unify_expr(body, rhs, lhs_ty, rhs_ty);
        true
    }

    pub(super) fn reject_calldata_array_index_assign(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    ) -> bool {
        let Some((index_expr, base, index)) = self.read_only_array_index_parts(body, lhs) else {
            return false;
        };
        let base_ty = self.infer_expr(body, base);
        if self.calldata_array_elem_ty(base_ty.clone()).is_none() {
            return false;
        }

        let index_result_ty = self
            .infer_calldata_array_index_read(body, index_expr, index, base_ty.clone(), None)
            .unwrap_or_else(|| self.engine.fresh_var());
        let lhs_ty = self.record_read_only_array_index_lhs_ty(body, lhs, index_result_ty);
        self.infer_expr_expected(body, rhs, Some(lhs_ty));
        let actual = self.display_infer_ty(base_ty);
        self.emit_expr_error(
            body,
            lhs,
            TypeckDiagnostic::Mismatch {
                span: self.expr_label_span(body, lhs),
                expected: "assignable storage-backed index".to_owned(),
                actual,
            },
        );
        true
    }

    /// Returns the underlying index and its operands after peeling transparent
    /// type annotations from an assignment target.
    fn read_only_array_index_parts(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> Option<(Id<Expr<'db>>, Id<Expr<'db>>, Id<Expr<'db>>)> {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Index { base, index } => Some((expr, *base, *index)),
            ExprKind::TypeAnnot { expr, .. } => self.read_only_array_index_parts(body, *expr),
            _ => None,
        }
    }

    /// Records the inferred index type through every transparent annotation on
    /// an immutable array assignment target, mirroring ordinary expression
    /// inference closely enough to retain annotation diagnostics and ExprTy
    /// entries even though the statement is rejected before generic typing.
    fn record_read_only_array_index_lhs_ty(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        index_ty: InferTy<'db>,
    ) -> InferTy<'db> {
        let kind = body.exprs(self.db).get(expr).kind.clone();
        match kind {
            ExprKind::Index { .. } => {
                self.expr_tys.push((body, expr, index_ty.clone()));
                index_ty
            }
            ExprKind::TypeAnnot { expr: inner, ty } => {
                let inner_ty = self.record_read_only_array_index_lhs_ty(body, inner, index_ty);
                let annotated_ty = self.lower_type_ref(ty);
                self.unify_expr(body, inner, annotated_ty.clone(), inner_ty);
                self.expr_tys.push((body, expr, annotated_ty.clone()));
                annotated_ty
            }
            _ => unreachable!("read-only array target was checked before recording its type"),
        }
    }

    fn infer_storage_ref_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        record_current: bool,
    ) -> Option<InferTy<'db>> {
        let kind = body.exprs(self.db).get(expr).kind.clone();
        let ty = match kind {
            ExprKind::Index { base, index } => {
                let base_ty = self.infer_storage_ref_expr(body, base, true)?;
                self.infer_storage_index_ref_ty(body, index, base_ty)
            }
            ExprKind::TypeAnnot { expr: inner, ty } => {
                let inner_ty = self.infer_storage_ref_expr(body, inner, true)?;
                let annotated_ty = self.lower_type_ref(ty);
                self.unify_expr(body, expr, annotated_ty.clone(), inner_ty);
                self.expr_tys.push((body, expr, annotated_ty.clone()));
                return Some(annotated_ty);
            }
            _ => match self.expr_resolutions.get(&(body, expr)).cloned() {
                Some(hir_nameres::Resolution::Field(field)) => {
                    Some(self.instantiate_field_ref(field, ObligationSource::Scheme))
                }
                Some(hir_nameres::Resolution::Local(_) | hir_nameres::Resolution::Param(_)) => {
                    let inferred = self.infer_expr(body, expr);
                    if self.is_storage_collection_ref_ty(inferred.clone()) {
                        return Some(inferred);
                    }
                    return None;
                }
                _ => None,
            },
        }?;
        if record_current {
            self.expr_tys.push((body, expr, ty.clone()));
        }
        Some(ty)
    }

    fn infer_storage_index_ref_ty(
        &mut self,
        body: FuncBody<'db>,
        index: Id<Expr<'db>>,
        base_ty: InferTy<'db>,
    ) -> Option<InferTy<'db>> {
        if let Some((index_ty, value_ty)) = self.storage_mapping_args(base_ty.clone()) {
            let actual_index_ty = self.infer_expr_expected(body, index, Some(index_ty.clone()));
            self.unify_expr(body, index, index_ty, actual_index_ty);
            return Some(value_ty);
        }

        let (storage_ctor, elem_ty) = self.storage_array_elem_ty(base_ty)?;
        let index_ty = self.infer_expr(body, index);
        self.push_typedef_word_obligation(index_ty, ObligationSource::Scheme);
        Some(InferTy::Named {
            ctor: storage_ctor,
            args: vec![elem_ty],
        })
    }

    pub(super) fn infer_memory_array_index_read(
        &mut self,
        body: FuncBody<'db>,
        index: Id<Expr<'db>>,
        base_ty: InferTy<'db>,
    ) -> Option<InferTy<'db>> {
        let elem_ty = self.memory_dyn_array_elem_ty(base_ty)?;
        let index_ty = self.infer_expr(body, index);
        self.push_typedef_word_obligation(index_ty, ObligationSource::Scheme);
        self.push_typedef_word_obligation(elem_ty.clone(), ObligationSource::Scheme);
        Some(elem_ty)
    }

    pub(super) fn infer_calldata_array_index_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        base_ty: InferTy<'db>,
        expected: Option<InferTy<'db>>,
    ) -> Option<InferTy<'db>> {
        self.calldata_array_elem_ty(base_ty.clone())?;
        self.infer_class_index_read(body, expr, index, base_ty, expected)
    }

    /// `c[i]` on any other user-defined collection, such as an array slice,
    /// reads through the std `RValueIdxAccess` instance for `(c, i)`, as the
    /// reference `ridx` desugaring does.
    pub(super) fn infer_user_collection_index_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        base_ty: InferTy<'db>,
        expected: Option<InferTy<'db>>,
    ) -> Option<InferTy<'db>> {
        if !matches!(
            self.engine.resolve(base_ty.clone()),
            InferTy::Named {
                ctor: TyCtor::User(_),
                ..
            }
        ) {
            return None;
        }
        self.infer_class_index_read(body, expr, index, base_ty, expected)
    }

    fn infer_class_index_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        base_ty: InferTy<'db>,
        expected: Option<InferTy<'db>>,
    ) -> Option<InferTy<'db>> {
        let class = self.lookup_class_id("RValueIdxAccess")?;
        if !matches!(
            class,
            ClassId::User(def)
                if crate::support::is_canonical_std_def_named(
                    self.db,
                    def,
                    "RValueIdxAccess",
                )
        ) {
            return None;
        }

        let index_ty = self.infer_expr(body, index);
        let result_ty = expected.unwrap_or_else(|| self.engine.fresh_var());
        self.pending.push(PendingObligation {
            class,
            main: product_infer_ty(vec![base_ty, index_ty]),
            args: vec![result_ty.clone()],
            source: ObligationSource::ClassMethod { body, expr },
        });
        Some(result_ty)
    }

    fn infer_storage_array_literal_assign(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        lhs_ty: InferTy<'db>,
    ) -> bool {
        let rhs_ty = self.infer_expr(body, rhs);
        if let Some((storage_ctor, storage_elem_ty)) = self.storage_array_elem_ty(lhs_ty.clone())
            && let Some(value_elem_ty) = self.memory_dyn_array_elem_ty(rhs_ty)
        {
            let storage_elem_ref = InferTy::Named {
                ctor: storage_ctor,
                args: vec![storage_elem_ty],
            };
            self.push_can_store_obligation(
                storage_elem_ref,
                value_elem_ty.clone(),
                ObligationSource::Scheme,
            );
            self.push_typedef_word_obligation(value_elem_ty, ObligationSource::Scheme);
        } else {
            let expected_elem = self.engine.fresh_var();
            if let Some(expected_lhs) = self.storage_array_ty(expected_elem) {
                self.unify_expr(body, lhs, expected_lhs, lhs_ty.clone());
            }
        }
        self.expr_tys.push((body, lhs, lhs_ty));
        true
    }

    pub(super) fn is_storage_index_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> bool {
        if matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return true;
        }
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Index { base, .. } => {
                if self.is_storage_index_expr(body, *base) {
                    true
                } else {
                    let base_ty = self.infer_expr(body, *base);
                    self.is_storage_collection_ref_ty(base_ty)
                }
            }
            ExprKind::TypeAnnot { expr, .. } => self.is_storage_index_expr(body, *expr),
            _ => false,
        }
    }

    fn storage_mapping_args(&mut self, ty: InferTy<'db>) -> Option<(InferTy<'db>, InferTy<'db>)> {
        let storage_ctor = self.storage_type_ctor();
        let ty = self.normalize_aliases(ty);
        let mut resolved = self.engine.resolve(ty);
        if let Some(storage_ctor) = storage_ctor
            && let InferTy::Named { ctor, args } = &resolved
            && *ctor == storage_ctor
            && args.len() == 1
        {
            let inner = self.normalize_aliases(args[0].clone());
            resolved = self.engine.resolve(inner);
        }
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = resolved
        else {
            return None;
        };
        if def.name(self.db).as_deref() != Some("mapping") || args.len() != 2 {
            return None;
        }
        let value = if let Some(storage_ctor) = storage_ctor {
            InferTy::Named {
                ctor: storage_ctor,
                args: vec![args[1].clone()],
            }
        } else {
            args[1].clone()
        };
        Some((args[0].clone(), value))
    }

    fn is_storage_collection_ref_ty(&mut self, ty: InferTy<'db>) -> bool {
        self.storage_mapping_args(ty.clone()).is_some() || self.storage_array_elem_ty(ty).is_some()
    }

    fn storage_type_ctor(&self) -> Option<TyCtor<'db>> {
        self.lookup_type_resolution("storage")
            .and_then(type_ctor_from_resolution)
    }

    fn memory_type_ctor(&self) -> Option<TyCtor<'db>> {
        self.lookup_type_resolution("memory")
            .and_then(type_ctor_from_resolution)
    }

    pub(super) fn memory_dyn_array_ty(&mut self, elem_ty: InferTy<'db>) -> Option<InferTy<'db>> {
        let memory = crate::support::canonical_std_adt_def(self.db, "memory")?;
        let dyn_array = crate::support::canonical_std_adt_def(self.db, "DynArray")?;
        Some(InferTy::Named {
            ctor: TyCtor::User(crate::UserTyCtor {
                def: memory,
                kind: UserTyCtorKind::Adt,
            }),
            args: vec![InferTy::Named {
                ctor: TyCtor::User(crate::UserTyCtor {
                    def: dyn_array,
                    kind: UserTyCtorKind::Adt,
                }),
                args: vec![elem_ty],
            }],
        })
    }

    fn storage_array_ty(&mut self, elem_ty: InferTy<'db>) -> Option<InferTy<'db>> {
        let storage = crate::support::canonical_std_adt_def(self.db, "storage")?;
        let array = crate::support::canonical_std_adt_def(self.db, "array")?;
        Some(InferTy::Named {
            ctor: TyCtor::User(crate::UserTyCtor {
                def: storage,
                kind: UserTyCtorKind::Adt,
            }),
            args: vec![InferTy::Named {
                ctor: TyCtor::User(crate::UserTyCtor {
                    def: array,
                    kind: UserTyCtorKind::Adt,
                }),
                args: vec![elem_ty],
            }],
        })
    }

    pub(super) fn memory_dyn_array_elem_ty(&mut self, ty: InferTy<'db>) -> Option<InferTy<'db>> {
        let memory = crate::support::canonical_std_adt_def(self.db, "memory")?;
        let dyn_array = crate::support::canonical_std_adt_def(self.db, "DynArray")?;
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor: TyCtor::User(memory_ctor),
            args: memory_args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if memory_ctor.def != memory || memory_args.len() != 1 {
            return None;
        }
        let inner = self.normalize_aliases(memory_args[0].clone());
        let InferTy::Named {
            ctor: TyCtor::User(array_ctor),
            args,
        } = self.engine.resolve(inner)
        else {
            return None;
        };
        (array_ctor.def == dyn_array && args.len() == 1).then(|| args[0].clone())
    }

    fn calldata_array_elem_ty(&mut self, ty: InferTy<'db>) -> Option<InferTy<'db>> {
        let calldata = crate::support::canonical_std_adt_def(self.db, "calldata")?;
        let array = crate::support::canonical_std_adt_def(self.db, "array")?;
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor: TyCtor::User(calldata_ctor),
            args: calldata_args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if calldata_ctor.def != calldata || calldata_args.len() != 1 {
            return None;
        }
        let inner = self.normalize_aliases(calldata_args[0].clone());
        let InferTy::Named {
            ctor: TyCtor::User(array_ctor),
            args,
        } = self.engine.resolve(inner)
        else {
            return None;
        };
        (array_ctor.def == array && args.len() == 1).then(|| args[0].clone())
    }

    fn storage_array_elem_ty(&mut self, ty: InferTy<'db>) -> Option<(TyCtor<'db>, InferTy<'db>)> {
        let storage = crate::support::canonical_std_adt_def(self.db, "storage")?;
        let array = crate::support::canonical_std_adt_def(self.db, "array")?;
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor: storage_ctor @ TyCtor::User(storage_user),
            args: storage_args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if storage_user.def != storage || storage_args.len() != 1 {
            return None;
        }
        let inner = self.normalize_aliases(storage_args[0].clone());
        let InferTy::Named {
            ctor: TyCtor::User(array_user),
            args,
        } = self.engine.resolve(inner)
        else {
            return None;
        };
        (array_user.def == array && args.len() == 1).then(|| (storage_ctor, args[0].clone()))
    }

    fn lookup_class_id(&self, name: &str) -> Option<ClassId<'db>> {
        self.lookup_type_resolution(name)
            .and_then(class_id_from_resolution)
    }

    fn lookup_type_resolution(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        if let Some(module_id) = self
            .entry_module
            .or_else(|| module_id_for_hir_module(self.db, self.module))
        {
            let env = nameres::module_import_surface(self.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.type_resolution(name));
            return local.or_else(|| env.types.get(name).cloned());
        }

        hir_nameres::item_scope_facts(self.db, self.module).type_resolution(name)
    }

    fn instantiate_field_ref(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        let ty = self.instantiate_field(field, source);
        if let Some(storage_ctor) = self.storage_type_ctor() {
            InferTy::Named {
                ctor: storage_ctor,
                args: vec![ty],
            }
        } else {
            ty
        }
    }

    pub(super) fn instantiate_field_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        field: hir_nameres::FieldId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        let field_ref = self.instantiate_field_ref(field, source);
        self.storage_load_ty(body, expr, field_ref)
    }

    fn storage_load_ty(
        &mut self,
        _body: FuncBody<'db>,
        _expr: Id<Expr<'db>>,
        storage_ty: InferTy<'db>,
    ) -> InferTy<'db> {
        if self.storage_type_ctor().is_none() {
            return storage_ty;
        }
        let loaded = self
            .loaded_ty_for_storage_ty(storage_ty.clone())
            .unwrap_or_else(|| self.engine.fresh_var());
        // Reading an array field yields its storage handle. Requiring the
        // array's CanStore instance here would also require StorageCopy for
        // the element type, even though that constraint is only needed by a
        // whole-array write.
        if self.storage_array_elem_ty(loaded.clone()).is_some() {
            return loaded;
        }
        self.push_can_store_obligation(storage_ty, loaded.clone(), ObligationSource::Scheme);
        loaded
    }

    fn loaded_ty_for_storage_ty(&mut self, ty: InferTy<'db>) -> Option<InferTy<'db>> {
        let Some(storage_ctor) = self.storage_type_ctor() else {
            return Some(ty);
        };
        let ty = self.normalize_aliases(ty);
        let InferTy::Named { ctor, args } = self.engine.resolve(ty.clone()) else {
            return None;
        };
        if ctor != storage_ctor || args.len() != 1 {
            return None;
        }
        let inner = self.normalize_aliases(args[0].clone());
        let inner = self.engine.resolve(inner);
        if self.is_mapping_adt_ty(inner.clone()) || self.is_storage_array_adt_ty(inner.clone()) {
            return Some(InferTy::Named {
                ctor: storage_ctor,
                args: vec![inner],
            });
        }
        if self.trait_env.is_some() {
            return None;
        }
        if self.is_memory_backed_storage_adt(inner.clone()) {
            let memory_ctor = self.memory_type_ctor()?;
            return Some(InferTy::Named {
                ctor: memory_ctor,
                args: vec![inner],
            });
        }
        Some(inner)
    }

    fn is_mapping_adt_ty(&mut self, ty: InferTy<'db>) -> bool {
        self.is_named_adt_ty(ty, "mapping", Some(2))
    }

    fn is_storage_array_adt_ty(&mut self, ty: InferTy<'db>) -> bool {
        let Some(array) = crate::support::canonical_std_adt_def(self.db, "array") else {
            return false;
        };
        let ty = self.normalize_aliases(ty);
        matches!(
            self.engine.resolve(ty),
            InferTy::Named {
                ctor: TyCtor::User(user),
                args,
            } if user.def == array && args.len() == 1
        )
    }

    fn is_memory_backed_storage_adt(&mut self, ty: InferTy<'db>) -> bool {
        self.is_named_adt_ty(ty.clone(), "string", Some(0))
            || self.is_named_adt_ty(ty, "bytes", Some(0))
    }

    fn is_named_adt_ty(&mut self, ty: InferTy<'db>, name: &str, arity: Option<usize>) -> bool {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return false;
        };
        def.name(self.db).as_deref() == Some(name) && arity.is_none_or(|arity| args.len() == arity)
    }

    fn push_can_store_obligation(
        &mut self,
        storage_ty: InferTy<'db>,
        loaded_ty: InferTy<'db>,
        source: ObligationSource<'db>,
    ) {
        let Some(class) = self.lookup_class_id("CanStore") else {
            return;
        };
        self.pending.push(PendingObligation {
            class,
            main: storage_ty,
            args: vec![loaded_ty],
            source,
        });
    }

    fn push_assign_obligation(
        &mut self,
        storage_ty: InferTy<'db>,
        value_ty: InferTy<'db>,
        source: ObligationSource<'db>,
    ) {
        let Some(class) = self.lookup_class_id("Assign") else {
            return;
        };
        self.pending.push(PendingObligation {
            class,
            main: storage_ty,
            args: vec![value_ty],
            source,
        });
    }

    pub(super) fn push_typedef_word_obligation(
        &mut self,
        value_ty: InferTy<'db>,
        source: ObligationSource<'db>,
    ) {
        let Some(class) = self.lookup_class_id("Typedef") else {
            return;
        };
        let word = self.word();
        self.pending.push(PendingObligation {
            class,
            main: value_ty,
            args: vec![word],
            source,
        });
    }
}
