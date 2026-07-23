use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn infer_storage_index_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
    ) -> Option<InferTy<'db>> {
        if !self.is_storage_index_expr(body, base) {
            return None;
        }
        let base_ty = self.infer_storage_ref_expr(body, base, true)?;
        let (index_ty, value_ty) = self.storage_mapping_args(base_ty)?;
        let actual_index_ty = self.infer_expr_expected(body, index, Some(index_ty.clone()));
        self.unify_expr(body, index, index_ty, actual_index_ty);
        Some(self.storage_load_ty(body, expr, value_ty))
    }

    pub(super) fn infer_storage_assign(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    ) -> bool {
        let Some(lhs_ty) = self.infer_storage_ref_expr(body, lhs, false) else {
            return false;
        };
        let expected_rhs = self
            .loaded_ty_for_storage_ty(lhs_ty.clone())
            .unwrap_or_else(|| self.engine.fresh_var());
        let rhs_ty = self.infer_expr_expected(body, rhs, Some(expected_rhs.clone()));
        self.unify_expr(body, rhs, expected_rhs, rhs_ty.clone());
        self.push_can_store_obligation(lhs_ty, rhs_ty.clone(), ObligationSource::Scheme);
        self.expr_tys.push((body, lhs, rhs_ty));
        true
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
                let (index_ty, value_ty) = self.storage_mapping_args(base_ty)?;
                let actual_index_ty = self.infer_expr_expected(body, index, Some(index_ty.clone()));
                self.unify_expr(body, index, index_ty, actual_index_ty);
                Some(value_ty)
            }
            ExprKind::Conversion { expr: inner, .. } => {
                self.infer_storage_ref_expr(body, inner, true)
            }
            _ => match self.expr_resolutions.get(&(body, expr)).cloned() {
                Some(hir_nameres::Resolution::Field(field)) => {
                    Some(self.instantiate_field_ref(field, ObligationSource::Scheme))
                }
                _ => None,
            },
        }?;
        if record_current {
            self.expr_tys.push((body, expr, ty.clone()));
        }
        Some(ty)
    }

    pub(super) fn is_storage_index_expr(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        if matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return true;
        }
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Index { base, .. } => self.is_storage_index_expr(body, *base),
            ExprKind::Conversion { expr, .. } => self.is_storage_index_expr(body, *expr),
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

    fn storage_type_ctor(&self) -> Option<TyCtor<'db>> {
        self.lookup_type_resolution("storage")
            .and_then(type_ctor_from_resolution)
    }

    fn memory_type_ctor(&self) -> Option<TyCtor<'db>> {
        self.lookup_type_resolution("memory")
            .and_then(type_ctor_from_resolution)
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
        if self.is_mapping_adt_ty(inner.clone()) {
            return Some(InferTy::Named {
                ctor: storage_ctor,
                args: vec![inner],
            });
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
}
