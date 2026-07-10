use super::*;

impl<'db> Emitter<'db> {
    pub(super) fn emit_contract(
        &mut self,
        contract: &MonoContract<'db>,
        functions: &[Function<'db>],
    ) -> Object<'db> {
        let deployment_main = contract.entries.iter().find_map(|entry| {
            if let MonoEntry::DeploymentMain {
                specialized, span, ..
            } = entry
            {
                Some((specialized, *span))
            } else {
                None
            }
        });
        let runtime_main = contract.entries.iter().find_map(|entry| {
            if let MonoEntry::RuntimeMain {
                specialized, span, ..
            } = entry
            {
                Some((specialized, *span))
            } else {
                None
            }
        });

        let storage_fields = self.contract_storage_fields(contract.def);
        let storage_hash_helper = storage_fields
            .values()
            .any(|field| field.kind == StorageFieldKind::Mapping)
            .then_some(STORAGE_HASH2_HELPER.to_owned());

        let deployment_roots =
            deployment_main.map_or_else(BTreeSet::new, |(name, _)| BTreeSet::from([name.clone()]));
        let runtime_roots =
            runtime_main.map_or_else(BTreeSet::new, |(name, _)| BTreeSet::from([name.clone()]));
        let deployment_names = deployment_closure(self.db, functions, &deployment_roots);
        let runtime_names = deployment_closure(self.db, functions, &runtime_roots);

        let mut mapping_value_helper_used = false;
        let mut deployment_functions = functions
            .iter()
            .filter(|function| deployment_names.contains(function.name.as_str()))
            .cloned()
            .map(|function| {
                self.lower_storage_fields_in_function(
                    function,
                    &storage_fields,
                    storage_hash_helper.as_deref(),
                    &mut mapping_value_helper_used,
                )
            })
            .collect::<Vec<_>>();
        let mut runtime_functions = functions
            .iter()
            .filter(|function| runtime_names.contains(function.name.as_str()))
            .cloned()
            .map(|function| {
                self.lower_storage_fields_in_function(
                    function,
                    &storage_fields,
                    storage_hash_helper.as_deref(),
                    &mut mapping_value_helper_used,
                )
            })
            .collect::<Vec<_>>();
        if let Some(helper) = storage_hash_helper.as_deref() {
            let helper_function = self.storage_hash2_function(contract.span, helper);
            deployment_functions.push(helper_function.clone());
            runtime_functions.push(helper_function);
        }
        if mapping_value_helper_used {
            let helper_function =
                self.storage_mapping_value_function(contract.span, STORAGE_MAPPING_VALUE_HELPER);
            deployment_functions.push(helper_function.clone());
            runtime_functions.push(helper_function);
        }

        let deployer_name = format!("{}Deploy", contract.name);
        let runtime_name = contract.name.clone();
        let mut deploy_stmts = Vec::new();
        if let Some((main, span)) = deployment_main {
            if let Some(call) = entry_call(&deployment_functions, main, span) {
                deploy_stmts.push(call);
            } else {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedDispatchEntry {
                        signature: "constructor".to_owned(),
                        reason: "specialized deployment entry function is missing".to_owned(),
                    },
                );
            }
        } else {
            self.push(
                contract.span,
                EmitDiagnosticKind::UnsupportedDispatchEntry {
                    signature: "constructor".to_owned(),
                    reason: "missing compiler-generated deployment entry".to_owned(),
                },
            );
        }

        let mut runtime_stmts = Vec::new();
        if let Some((main, span)) = runtime_main {
            if let Some(call) = entry_call(&runtime_functions, main, span) {
                runtime_stmts.push(call);
            } else {
                self.push(
                    span,
                    EmitDiagnosticKind::DispatcherDeferred {
                        contract: contract.name.clone(),
                    },
                );
            }
        } else {
            self.push(
                contract.span,
                EmitDiagnosticKind::DispatcherDeferred {
                    contract: contract.name.clone(),
                },
            );
        }

        Object {
            span: contract.span,
            name: deployer_name.into(),
            code: CodeBlock {
                span: contract.span,
                stmts: deploy_stmts,
                functions: deployment_functions,
            },
            inners: vec![Object {
                span: contract.span,
                name: runtime_name.into(),
                code: CodeBlock {
                    span: contract.span,
                    stmts: runtime_stmts,
                    functions: runtime_functions,
                },
                inners: Vec::new(),
            }],
        }
    }
}

fn entry_call<'db>(functions: &[Function<'db>], name: &str, span: Span<'db>) -> Option<Stmt<'db>> {
    let ret = functions
        .iter()
        .find(|function| function.name.as_str() == name)
        .map(|function| function.ret.clone())?;
    Some(Stmt {
        span,
        kind: StmtKind::Expr(Expr {
            span,
            // Entry return values are ignored by the EVM object code, but the
            // call expression must retain the callee's Hull type.
            ty: ret,
            kind: ExprKind::Call {
                callee: name.into(),
                args: Vec::new(),
            },
        }),
    })
}
