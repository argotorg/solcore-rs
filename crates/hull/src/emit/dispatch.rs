use super::*;

struct SelectorDispatchEntry<'a, 'db> {
    span: Span<'db>,
    payable: bool,
    outputs: &'a [MonoAbiParam],
}

impl<'db> Emitter<'db> {
    pub(super) fn emit_dispatcher(
        &mut self,
        contract: &MonoContract<'db>,
        functions: &[Function<'db>],
    ) -> Vec<Stmt<'db>> {
        let dispatch_entries = contract
            .entries
            .iter()
            .filter(|entry| matches!(entry, MonoEntry::SelectorMethod { .. }))
            .collect::<Vec<_>>();

        // The reference inserts SAIL `RunContract.exec` before typechecking and
        // lets std/dispatch.solc specialize it. At mono time we already have
        // selectors and specialized callees, so the Rust backend synthesizes the
        // equivalent static-word dispatcher directly in Hull/Yul.
        let function_map = functions
            .iter()
            .map(|function| (function.name.as_str(), function))
            .collect::<BTreeMap<_, _>>();
        let span = contract.span;
        let fallback_body = self.emit_fallback_dispatch(contract, &function_map);
        let mut out = vec![self.memoryguard_stmt(span)];
        if dispatch_entries.is_empty() {
            out.extend(fallback_body);
            return out;
        }

        let method_body = self.emit_selector_dispatch(
            contract,
            &dispatch_entries,
            &function_map,
            fallback_body.clone(),
        );
        out.push(Stmt {
            span,
            kind: StmtKind::Match {
                target: bool_sum_ty(span),
                scrutinee: Expr {
                    span,
                    ty: bool_sum_ty(span),
                    kind: ExprKind::Call {
                        callee: "lt".to_owned(),
                        args: vec![
                            Expr {
                                span,
                                ty: Ty::word(span),
                                kind: ExprKind::Call {
                                    callee: "calldatasize".to_owned(),
                                    args: Vec::new(),
                                },
                            },
                            Expr::word(span, "4"),
                        ],
                    },
                },
                alts: vec![
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inr),
                        },
                        binder: self.fresh_alt(),
                        body: fallback_body,
                    },
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inl),
                        },
                        binder: self.fresh_alt(),
                        body: method_body,
                    },
                ],
            },
        });
        out
    }

    fn emit_selector_dispatch(
        &mut self,
        contract: &MonoContract<'db>,
        dispatch_entries: &[&MonoEntry<'db>],
        function_map: &BTreeMap<&str, &Function<'db>>,
        fallback_body: Vec<Stmt<'db>>,
    ) -> Vec<Stmt<'db>> {
        let span = contract.span;
        let selector_name = format!("{}_dispatch_selector", contract.name);
        let mut out = vec![
            Stmt {
                span,
                kind: StmtKind::Let {
                    name: selector_name.clone(),
                    ty: Ty::word(span),
                },
            },
            self.assembly_stmt(
                span,
                vec![self.yul_assign(
                    span,
                    &selector_name,
                    self.yul_call(
                        span,
                        "shr",
                        vec![
                            self.yul_number(span, "224"),
                            self.yul_call(span, "calldataload", vec![self.yul_number(span, "0")]),
                        ],
                    ),
                )],
            ),
        ];

        let mut alts = Vec::new();
        for (index, entry) in dispatch_entries.iter().enumerate() {
            let MonoEntry::SelectorMethod {
                specialized,
                span: entry_span,
                selector,
                signature,
                payable,
                inputs,
                outputs,
                ..
            } = *entry
            else {
                continue;
            };
            let Some(function) = function_map.get(specialized.as_str()).copied() else {
                self.push_unsupported_dispatch_entry(
                    *entry_span,
                    signature,
                    "missing specialized function",
                );
                continue;
            };
            if function.args.len() != inputs.len() {
                self.push_unsupported_dispatch_entry(
                    *entry_span,
                    signature,
                    "ABI/function arity mismatch",
                );
                continue;
            }
            let Some(input_layouts) = dispatcher_input_layouts(function, inputs) else {
                self.push_unsupported_dispatch_entry(*entry_span, signature, "non-word ABI shape");
                continue;
            };
            let Some(return_layout) = dispatcher_return_layout(&function.ret, outputs) else {
                self.push_unsupported_dispatch_entry(*entry_span, signature, "non-word ABI shape");
                continue;
            };
            alts.push(Alt {
                span: *entry_span,
                pat: Pat {
                    span: *entry_span,
                    kind: PatKind::IntLit(selector_hex(*selector)),
                },
                binder: self.fresh_alt(),
                body: self.emit_dispatch_entry(
                    SelectorDispatchEntry {
                        span: *entry_span,
                        payable: *payable,
                        outputs,
                    },
                    function,
                    index,
                    &input_layouts,
                    &return_layout,
                ),
            });
        }

        alts.push(Alt {
            span,
            pat: Pat {
                span,
                kind: PatKind::Wildcard,
            },
            binder: self.fresh_alt(),
            body: fallback_body,
        });

        out.push(Stmt {
            span,
            kind: StmtKind::Match {
                target: Ty::word(span),
                scrutinee: Expr::var(span, selector_name, Ty::word(span)),
                alts,
            },
        });
        out
    }

    fn push_unsupported_dispatch_entry(&mut self, span: Span<'db>, signature: &str, reason: &str) {
        self.push(
            span,
            EmitDiagnosticKind::UnsupportedDispatchEntry {
                signature: signature.to_owned(),
                reason: reason.to_owned(),
            },
        );
    }

    fn emit_dispatch_entry(
        &mut self,
        entry: SelectorDispatchEntry<'_, 'db>,
        function: &Function<'db>,
        index: usize,
        input_layouts: &[StaticAbiLayout<'db>],
        return_layout: &StaticAbiLayout<'db>,
    ) -> Vec<Stmt<'db>> {
        let span = entry.span;
        let mut body = Vec::new();
        if !entry.payable {
            body.push(self.nonpayable_check(span));
        }
        let input_word_count = input_layouts
            .iter()
            .map(|layout| layout.slots)
            .sum::<usize>();
        if input_word_count > 0 {
            body.push(self.abi_input_truncated_check(span, input_word_count));
        }

        let mut args = Vec::new();
        let mut word_offset = 0;
        for (arg_index, arg) in function.args.iter().enumerate() {
            let layout = &input_layouts[arg_index];
            let arg_name = format!("dispatch_arg{index}_{arg_index}");
            let word_names = self.decode_dispatch_abi_words(
                span,
                &format!("{arg_name}_word"),
                word_offset,
                layout,
                &mut body,
            );
            word_offset += layout.slots;
            let rhs = abi_words_to_expr(span, layout, &word_names);
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
                    rhs,
                },
            });
            args.push(Expr::var(span, arg_name, arg.ty.clone()));
        }

        let call = Expr {
            span,
            ty: function.ret.clone(),
            kind: ExprKind::Call {
                callee: function.name.clone(),
                args,
            },
        };

        match return_layout.slots {
            0 => {
                body.push(Stmt {
                    span,
                    kind: StmtKind::Expr(call),
                });
                body.push(self.return_abi_words(span, &[], &[]));
            }
            _ => {
                let ret_name = format!("dispatch_ret{index}");
                body.push(Stmt {
                    span,
                    kind: StmtKind::Let {
                        name: ret_name.clone(),
                        ty: function.ret.clone(),
                    },
                });
                body.push(Stmt {
                    span,
                    kind: StmtKind::Assign {
                        lhs: Expr::var(span, ret_name.clone(), function.ret.clone()),
                        rhs: call,
                    },
                });
                let ret_expr = Expr::var(span, ret_name, function.ret.clone());
                let names = self.encode_dispatch_return_words(
                    span,
                    &format!("dispatch_ret{index}_word"),
                    ret_expr,
                    return_layout,
                    &mut body,
                );
                body.push(self.return_abi_words(span, &names, entry.outputs));
            }
        }
        body
    }

    fn decode_dispatch_abi_words(
        &self,
        span: Span<'db>,
        prefix: &str,
        word_offset: usize,
        layout: &StaticAbiLayout<'db>,
        body: &mut Vec<Stmt<'db>>,
    ) -> Vec<String> {
        let kinds = abi_layout_slot_kinds(layout);
        let mut names = Vec::new();
        for (slot, kind) in kinds.into_iter().enumerate() {
            let name = numbered_name(prefix, slot, layout.slots);
            body.push(Stmt {
                span,
                kind: StmtKind::Let {
                    name: name.clone(),
                    ty: Ty::word(span),
                },
            });
            body.push(self.decode_calldata_arg(span, &name, word_offset + slot, kind));
            names.push(name);
        }
        names
    }

    fn encode_dispatch_return_words(
        &self,
        span: Span<'db>,
        prefix: &str,
        value: Expr<'db>,
        layout: &StaticAbiLayout<'db>,
        body: &mut Vec<Stmt<'db>>,
    ) -> Vec<String> {
        let mut names = Vec::new();
        for slot in 0..layout.slots {
            let name = numbered_name(prefix, slot, layout.slots);
            body.push(Stmt {
                span,
                kind: StmtKind::Let {
                    name: name.clone(),
                    ty: Ty::word(span),
                },
            });
            body.push(Stmt {
                span,
                kind: StmtKind::Assign {
                    lhs: Expr::var(span, name.clone(), Ty::word(span)),
                    rhs: Expr::word(span, "0"),
                },
            });
            names.push(name);
        }
        write_expr_to_abi_slots(span, value, layout, &names, body);
        names
    }

    fn emit_fallback_dispatch(
        &mut self,
        contract: &MonoContract<'db>,
        function_map: &BTreeMap<&str, &Function<'db>>,
    ) -> Vec<Stmt<'db>> {
        let span = contract.fallback.span;
        let mut body = Vec::new();
        if !contract.fallback.payable {
            body.push(self.nonpayable_check(span));
        }
        let Some(name) = contract.fallback.specialized.as_deref() else {
            body.push(self.default_fallback_revert(span));
            return body;
        };
        let Some(function) = function_map.get(name).copied() else {
            body.push(self.default_fallback_revert(span));
            return body;
        };
        if !contract.fallback.inputs.is_empty()
            || !contract.fallback.outputs.is_empty()
            || !function.args.is_empty()
            || !matches!(function.ret.strip_named().kind, TyKind::Unit)
        {
            self.push(
                contract.fallback.span,
                EmitDiagnosticKind::UnsupportedDispatchEntry {
                    signature: "fallback".to_owned(),
                    reason: "fallback ABI must be unit -> unit".to_owned(),
                },
            );
            body.push(self.default_fallback_revert(span));
            return body;
        }
        let call = Expr {
            span,
            ty: function.ret.clone(),
            kind: ExprKind::Call {
                callee: function.name.clone(),
                args: Vec::new(),
            },
        };
        body.push(Stmt {
            span,
            kind: StmtKind::Expr(call),
        });
        body.push(self.stop_stmt(span));
        body
    }

    fn memoryguard_stmt(&self, span: Span<'db>) -> Stmt<'db> {
        self.assembly_stmt(
            span,
            vec![self.yul_expr_stmt(
                span,
                self.yul_call(
                    span,
                    "mstore",
                    vec![
                        self.yul_number(span, "0x40"),
                        self.yul_call(span, "memoryguard", vec![self.yul_number(span, "128")]),
                    ],
                ),
            )],
        )
    }

    fn abi_input_truncated_check(&self, span: Span<'db>, word_count: usize) -> Stmt<'db> {
        self.assembly_stmt(
            span,
            vec![YulStmt {
                span,
                kind: YulStmtKind::If {
                    cond: self.yul_call(
                        span,
                        "lt",
                        vec![
                            self.yul_call(span, "calldatasize", Vec::new()),
                            self.yul_number(span, (4 + word_count * 32).to_string()),
                        ],
                    ),
                    body: vec![
                        self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "mstore",
                                vec![
                                    self.yul_number(span, "0"),
                                    self.yul_number(span, "0x08638556"),
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
                },
            }],
        )
    }

    fn decode_calldata_arg(
        &self,
        span: Span<'db>,
        name: &str,
        index: usize,
        kind: AbiWordKind,
    ) -> Stmt<'db> {
        let mut stmts = vec![self.yul_assign(
            span,
            name,
            self.yul_call(
                span,
                "calldataload",
                vec![self.yul_number(span, (4 + index * 32).to_string())],
            ),
        )];
        self.push_abi_word_cleaning(span, name, kind, &mut stmts);
        self.assembly_stmt(span, stmts)
    }

    pub(super) fn push_abi_word_cleaning(
        &self,
        span: Span<'db>,
        name: &str,
        kind: AbiWordKind,
        stmts: &mut Vec<YulStmt<'db>>,
    ) {
        match kind {
            AbiWordKind::Plain => {}
            AbiWordKind::Address => self.push_address_cleaning(span, name, stmts),
            AbiWordKind::Bool => self.push_bool_cleaning(span, name, stmts),
        }
    }

    fn push_address_cleaning(&self, span: Span<'db>, name: &str, stmts: &mut Vec<YulStmt<'db>>) {
        // Keep address ABI entries in the supported subset: reject dirty high
        // bits like std.solc and store/return the low 160-bit canonical value.
        stmts.push(YulStmt {
            span,
            kind: YulStmtKind::If {
                cond: self.yul_call(
                    span,
                    "shr",
                    vec![
                        self.yul_number(span, "160"),
                        self.yul_ident_expr(span, name),
                    ],
                ),
                body: vec![
                    self.yul_expr_stmt(
                        span,
                        self.yul_call(
                            span,
                            "mstore",
                            vec![
                                self.yul_number(span, "0"),
                                self.yul_number(span, "0x7cc04fa7"),
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
            },
        });
        stmts.push(self.yul_assign(
            span,
            name,
            self.yul_call(
                span,
                "and",
                vec![
                    self.yul_ident_expr(span, name),
                    self.yul_number(span, ADDRESS_MASK),
                ],
            ),
        ));
    }

    fn push_bool_cleaning(&self, span: Span<'db>, name: &str, stmts: &mut Vec<YulStmt<'db>>) {
        stmts.push(YulStmt {
            span,
            kind: YulStmtKind::If {
                cond: self.yul_call(
                    span,
                    "gt",
                    vec![self.yul_ident_expr(span, name), self.yul_number(span, "1")],
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
        });
    }

    pub(super) fn nonpayable_check(&self, span: Span<'db>) -> Stmt<'db> {
        self.assembly_stmt(
            span,
            vec![YulStmt {
                span,
                kind: YulStmtKind::If {
                    cond: self.yul_call(span, "callvalue", Vec::new()),
                    body: vec![
                        self.yul_expr_stmt(
                            span,
                            self.yul_call(
                                span,
                                "mstore",
                                vec![
                                    self.yul_number(span, "0"),
                                    self.yul_number(span, "0xb5988ea3"),
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
                },
            }],
        )
    }

    fn default_fallback_revert(&self, span: Span<'db>) -> Stmt<'db> {
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
                            self.yul_number(span, "0x4924aef0"),
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
        )
    }

    fn stop_stmt(&self, span: Span<'db>) -> Stmt<'db> {
        self.assembly_stmt(
            span,
            vec![self.yul_expr_stmt(span, self.yul_call(span, "stop", Vec::new()))],
        )
    }

    fn return_abi_words(
        &self,
        span: Span<'db>,
        names: &[String],
        outputs: &[MonoAbiParam],
    ) -> Stmt<'db> {
        let mut stmts = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let value = match outputs.get(index).map(abi_word_kind) {
                Some(AbiWordKind::Address) => self.yul_call(
                    span,
                    "and",
                    vec![
                        self.yul_ident_expr(span, name),
                        self.yul_number(span, ADDRESS_MASK),
                    ],
                ),
                Some(AbiWordKind::Bool) => self.yul_call(
                    span,
                    "iszero",
                    vec![self.yul_call(span, "iszero", vec![self.yul_ident_expr(span, name)])],
                ),
                Some(AbiWordKind::Plain) | None => self.yul_ident_expr(span, name),
            };
            stmts.push(self.yul_expr_stmt(
                span,
                self.yul_call(
                    span,
                    "mstore",
                    vec![self.yul_number(span, (index * 32).to_string()), value],
                ),
            ));
        }
        stmts.push(self.yul_expr_stmt(
            span,
            self.yul_call(
                span,
                "return",
                vec![
                    self.yul_number(span, "0"),
                    self.yul_number(span, (names.len() * 32).to_string()),
                ],
            ),
        ));
        self.assembly_stmt(span, stmts)
    }
}
