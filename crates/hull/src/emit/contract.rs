use super::*;

impl<'db> Emitter<'db> {
    pub(super) fn emit_contract(
        &mut self,
        contract: &MonoContract<'db>,
        functions: &[Function<'db>],
    ) -> Object<'db> {
        let mut constructor_names = BTreeSet::new();
        if let Some(name) = &contract.constructor.specialized {
            constructor_names.insert(name.clone());
        }
        for entry in &contract.entries {
            if matches!(entry.kind, specialize::MonoEntryKind::Constructor) {
                constructor_names.insert(entry.specialized.clone());
            }
        }

        let storage_fields = self.contract_storage_fields(contract.def);
        let storage_hash_helper = storage_fields
            .values()
            .any(|field| field.kind == StorageFieldKind::Mapping)
            .then_some(STORAGE_HASH2_HELPER.to_owned());

        let deployment_names = deployment_closure(self.db, functions, &constructor_names);
        let mut mapping_value_helper_used = false;
        let mut deployment_functions = functions
            .iter()
            .filter(|function| deployment_names.contains(&function.name))
            .cloned()
            .map(|function| {
                self.lower_storage_fields_in_function(
                    function,
                    &storage_fields,
                    storage_hash_helper.as_deref(),
                    &mut mapping_value_helper_used,
                )
            })
            .map(ensure_unit_function_returns)
            .collect::<Vec<_>>();
        let mut runtime_functions = functions
            .iter()
            .filter(|function| !constructor_names.contains(&function.name))
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
        let deploy_stmts = self.emit_deployer(
            contract,
            &deployment_functions,
            &deployer_name,
            &runtime_name,
        );

        let mut runtime_stmts = Vec::new();
        if self.options.emit_dispatcher_comments {
            for entry in &contract.entries {
                if let Some(selector) = entry.selector {
                    runtime_stmts.push(Stmt {
                        span: entry.span,
                        kind: StmtKind::Comment(format!(
                            "selector 0x{:02x}{:02x}{:02x}{:02x} -> {}",
                            selector[0], selector[1], selector[2], selector[3], entry.specialized
                        )),
                    });
                }
            }
        }
        runtime_stmts.extend(self.emit_dispatcher(contract, &runtime_functions));

        Object {
            span: contract.span,
            name: deployer_name,
            code: CodeBlock {
                span: contract.span,
                stmts: deploy_stmts,
                functions: deployment_functions,
            },
            inners: vec![Object {
                span: contract.span,
                name: runtime_name,
                code: CodeBlock {
                    span: contract.span,
                    stmts: runtime_stmts,
                    functions: runtime_functions,
                },
                inners: Vec::new(),
            }],
        }
    }

    fn emit_deployer(
        &mut self,
        contract: &MonoContract<'db>,
        deployment_functions: &[Function<'db>],
        deployer_name: &str,
        runtime_name: &str,
    ) -> Vec<Stmt<'db>> {
        let span = contract.span;
        let mut body =
            vec![self.deployer_setup(span, deployer_name, contract.constructor.inputs.len())];
        if !contract.constructor.payable {
            body.push(self.nonpayable_check(span));
        }

        if let Some(constructor_name) = contract.constructor.specialized.as_deref() {
            let Some(function) = deployment_functions
                .iter()
                .find(|function| function.name == constructor_name)
            else {
                self.push(
                    contract.constructor.span,
                    EmitDiagnosticKind::UnsupportedDispatchEntry {
                        signature: "constructor".to_owned(),
                        reason: "missing specialized constructor function".to_owned(),
                    },
                );
                body.push(self.return_runtime_object(span, runtime_name));
                return body;
            };

            if !constructor_inputs_are_static_word(contract)
                || function.args.len() != contract.constructor.inputs.len()
            {
                self.push(
                    contract.constructor.span,
                    EmitDiagnosticKind::UnsupportedDispatchEntry {
                        signature: "constructor".to_owned(),
                        reason: "unsupported constructor ABI shape".to_owned(),
                    },
                );
                body.push(self.return_runtime_object(span, runtime_name));
                return body;
            }

            let mut args = Vec::new();
            for (index, arg) in function.args.iter().enumerate() {
                let arg_name = format!("constructor_arg{index}");
                let abi_kind = abi_word_kind(&contract.constructor.inputs[index]);
                if matches!(abi_kind, AbiWordKind::Bool) {
                    let raw_name = format!("{arg_name}_word");
                    body.push(Stmt {
                        span,
                        kind: StmtKind::Let {
                            name: raw_name.clone(),
                            ty: Ty::word(span),
                        },
                    });
                    body.push(self.decode_constructor_arg(
                        span,
                        deployer_name,
                        &raw_name,
                        index,
                        abi_kind,
                    ));
                    body.push(Stmt {
                        span,
                        kind: StmtKind::Let {
                            name: arg_name.clone(),
                            ty: arg.ty.clone(),
                        },
                    });
                    body.push(Stmt {
                        span,
                        kind: StmtKind::Assign {
                            lhs: Expr::var(span, arg_name.clone(), arg.ty.clone()),
                            rhs: abi_word_to_bool_expr(
                                span,
                                Expr::var(span, raw_name, Ty::word(span)),
                                arg.ty.clone(),
                            ),
                        },
                    });
                } else {
                    body.push(Stmt {
                        span,
                        kind: StmtKind::Let {
                            name: arg_name.clone(),
                            ty: arg.ty.clone(),
                        },
                    });
                    body.push(self.decode_constructor_arg(
                        span,
                        deployer_name,
                        &arg_name,
                        index,
                        abi_kind,
                    ));
                }
                args.push(Expr::var(span, arg_name, arg.ty.clone()));
            }

            body.push(Stmt {
                span,
                kind: StmtKind::Expr(Expr {
                    span,
                    ty: function.ret.clone(),
                    kind: ExprKind::Call {
                        callee: function.name.clone(),
                        args,
                    },
                }),
            });
        }

        body.push(self.return_runtime_object(span, runtime_name));
        body
    }

    fn deployer_setup(
        &self,
        span: Span<'db>,
        deployer_name: &str,
        constructor_arg_count: usize,
    ) -> Stmt<'db> {
        let deployer_size =
            self.yul_call(span, "datasize", vec![self.yul_string(span, deployer_name)]);
        let minimum_size = if constructor_arg_count == 0 {
            deployer_size
        } else {
            self.yul_call(
                span,
                "add",
                vec![
                    deployer_size,
                    self.yul_number(span, (constructor_arg_count * 32).to_string()),
                ],
            )
        };
        self.assembly_stmt(
            span,
            vec![
                self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "mstore",
                        vec![
                            self.yul_number(span, "64"),
                            self.yul_call(span, "memoryguard", vec![self.yul_number(span, "128")]),
                        ],
                    ),
                ),
                YulStmt {
                    span,
                    kind: YulStmtKind::If {
                        cond: self.yul_call(
                            span,
                            "lt",
                            vec![self.yul_call(span, "codesize", Vec::new()), minimum_size],
                        ),
                        body: vec![self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "revert",
                                vec![self.yul_number(span, "0"), self.yul_number(span, "0")],
                            ),
                        )],
                    },
                },
            ],
        )
    }

    fn return_runtime_object(&self, span: Span<'db>, runtime_name: &str) -> Stmt<'db> {
        self.assembly_stmt(
            span,
            vec![
                self.yul_let(
                    span,
                    "size",
                    Some(self.yul_call(
                        span,
                        "datasize",
                        vec![self.yul_string(span, runtime_name)],
                    )),
                ),
                self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "codecopy",
                        vec![
                            self.yul_number(span, "0"),
                            self.yul_call(
                                span,
                                "dataoffset",
                                vec![self.yul_string(span, runtime_name)],
                            ),
                            self.yul_call(
                                span,
                                "datasize",
                                vec![self.yul_string(span, runtime_name)],
                            ),
                        ],
                    ),
                ),
                self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "return",
                        vec![
                            self.yul_number(span, "0"),
                            self.yul_ident_expr(span, "size"),
                        ],
                    ),
                ),
            ],
        )
    }

    fn decode_constructor_arg(
        &self,
        span: Span<'db>,
        deployer_name: &str,
        name: &str,
        index: usize,
        kind: AbiWordKind,
    ) -> Stmt<'db> {
        let offset = if index == 0 {
            self.yul_call(span, "datasize", vec![self.yul_string(span, deployer_name)])
        } else {
            self.yul_call(
                span,
                "add",
                vec![
                    self.yul_call(span, "datasize", vec![self.yul_string(span, deployer_name)]),
                    self.yul_number(span, (index * 32).to_string()),
                ],
            )
        };
        let mut stmts = vec![
            self.yul_expr_stmt(
                span,
                self.yul_call(
                    span,
                    "codecopy",
                    vec![
                        self.yul_number(span, "0"),
                        offset,
                        self.yul_number(span, "32"),
                    ],
                ),
            ),
            self.yul_assign(
                span,
                name,
                self.yul_call(span, "mload", vec![self.yul_number(span, "0")]),
            ),
        ];
        self.push_abi_word_cleaning(span, name, kind, &mut stmts);
        self.assembly_stmt(span, stmts)
    }
}

fn ensure_unit_function_returns<'db>(mut function: Function<'db>) -> Function<'db> {
    if matches!(function.ret.strip_named().kind, TyKind::Unit) {
        function.body.push(Stmt {
            span: function.span,
            kind: StmtKind::Return(Expr::unit(function.span)),
        });
    }
    function
}
