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
                        callee: "lt".into(),
                        args: vec![
                            Expr {
                                span,
                                ty: Ty::word(span),
                                kind: ExprKind::Call {
                                    callee: "calldatasize".into(),
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
                        binder: self.fresh_alt().into(),
                        body: fallback_body,
                    },
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inl),
                        },
                        binder: self.fresh_alt().into(),
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
                    name: selector_name.clone().into(),
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
                binder: self.fresh_alt().into(),
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
            binder: self.fresh_alt().into(),
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
            .map(StaticAbiLayout::abi_head_slots)
            .sum::<usize>();
        if input_word_count > 0 {
            body.push(self.abi_input_truncated_check(span, input_word_count));
        }

        let mut args = Vec::new();
        let mut head_offset = 0;
        for (arg_index, arg) in function.args.iter().enumerate() {
            let layout = &input_layouts[arg_index];
            let arg_name = format!("dispatch_arg{index}_{arg_index}");
            let rhs = self.decode_dispatch_arg_expr(
                span,
                &format!("{arg_name}_word"),
                head_offset,
                layout,
                &mut body,
            );
            head_offset += layout.abi_head_slots();
            body.push(Stmt {
                span,
                kind: StmtKind::Let {
                    name: arg_name.clone().into(),
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
                        name: ret_name.clone().into(),
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
                if return_layout.has_dynamic_abi() {
                    body.push(self.return_abi_layout(
                        span,
                        &format!("dispatch_ret{index}_abi"),
                        &names,
                        return_layout,
                    ));
                } else {
                    body.push(self.return_abi_words(span, &names, entry.outputs));
                }
            }
        }
        body
    }

    fn decode_dispatch_arg_expr(
        &self,
        span: Span<'db>,
        prefix: &str,
        head_offset: usize,
        layout: &StaticAbiLayout<'db>,
        body: &mut Vec<Stmt<'db>>,
    ) -> Expr<'db> {
        if !layout.has_dynamic_abi() {
            let word_names =
                self.decode_dispatch_abi_words(span, prefix, head_offset, layout, body);
            return abi_words_to_expr(span, layout, &word_names);
        }

        match &layout.kind {
            StaticAbiLayoutKind::BytesLike => {
                let name = numbered_name(prefix, 0, 1);
                body.push(Stmt {
                    span,
                    kind: StmtKind::Let {
                        name: name.clone().into(),
                        ty: Ty::word(span),
                    },
                });
                body.push(self.decode_calldata_bytes_like_arg(span, &name, head_offset));
                let mut expr = Expr::var(span, name, Ty::word(span));
                expr.ty = layout.ty.clone();
                expr
            }
            StaticAbiLayoutKind::Product(layouts) => {
                let mut offset = head_offset;
                let mut elems = Vec::new();
                for (index, component) in layouts.iter().enumerate() {
                    elems.push(self.decode_dispatch_arg_expr(
                        span,
                        &format!("{prefix}_{index}"),
                        offset,
                        component,
                        body,
                    ));
                    offset += component.abi_head_slots();
                }
                product_expr(span, layout.ty.clone(), elems)
            }
            StaticAbiLayoutKind::Unit
            | StaticAbiLayoutKind::Word(_)
            | StaticAbiLayoutKind::Sum { .. } => {
                let word_names =
                    self.decode_dispatch_abi_words(span, prefix, head_offset, layout, body);
                abi_words_to_expr(span, layout, &word_names)
            }
        }
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
                    name: name.clone().into(),
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
                    name: name.clone().into(),
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

    fn decode_calldata_bytes_like_arg(
        &self,
        span: Span<'db>,
        name: &str,
        head_index: usize,
    ) -> Stmt<'db> {
        let head = format!("{name}_head");
        let src = format!("{name}_src");
        let length = format!("{name}_len");
        let total = format!("{name}_total");
        let rounded = format!("{name}_rounded");
        let ptr = format!("{name}_ptr");
        self.assembly_stmt(
            span,
            vec![
                self.yul_let(
                    span,
                    &head,
                    Some(self.yul_call(
                        span,
                        "calldataload",
                        vec![self.yul_number(span, (4 + head_index * 32).to_string())],
                    )),
                ),
                self.yul_let(
                    span,
                    &src,
                    Some(self.yul_call(
                        span,
                        "add",
                        vec![self.yul_number(span, "4"), self.yul_ident_expr(span, &head)],
                    )),
                ),
                self.yul_let(
                    span,
                    &length,
                    Some(self.yul_call(
                        span,
                        "calldataload",
                        vec![self.yul_ident_expr(span, &src)],
                    )),
                ),
                self.yul_let(
                    span,
                    &total,
                    Some(self.yul_call(
                        span,
                        "add",
                        vec![
                            self.yul_ident_expr(span, &length),
                            self.yul_number(span, "32"),
                        ],
                    )),
                ),
                self.yul_let(
                    span,
                    &rounded,
                    Some(self.yul_round_up_to_word(span, self.yul_ident_expr(span, &total))),
                ),
                self.yul_let(
                    span,
                    &ptr,
                    Some(self.yul_call(span, "mload", vec![self.yul_number(span, "0x40")])),
                ),
                self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "mstore",
                        vec![
                            self.yul_number(span, "0x40"),
                            self.yul_call(
                                span,
                                "add",
                                vec![
                                    self.yul_ident_expr(span, &ptr),
                                    self.yul_ident_expr(span, &rounded),
                                ],
                            ),
                        ],
                    ),
                ),
                self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "calldatacopy",
                        vec![
                            self.yul_ident_expr(span, &ptr),
                            self.yul_ident_expr(span, &src),
                            self.yul_ident_expr(span, &total),
                        ],
                    ),
                ),
                self.yul_assign(span, name, self.yul_ident_expr(span, &ptr)),
            ],
        )
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
            let kind = outputs.get(index).map(abi_word_kind);
            let value = self.return_abi_word_value(span, name, kind);
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

    fn return_abi_layout(
        &self,
        span: Span<'db>,
        prefix: &str,
        names: &[String],
        layout: &StaticAbiLayout<'db>,
    ) -> Stmt<'db> {
        let base = format!("{prefix}_base");
        let tail = format!("{prefix}_tail");
        let mut stmts = vec![
            self.yul_let(
                span,
                &base,
                Some(self.yul_call(span, "mload", vec![self.yul_number(span, "0x40")])),
            ),
            self.yul_let(
                span,
                &tail,
                Some(self.yul_call(
                    span,
                    "add",
                    vec![
                        self.yul_ident_expr(span, &base),
                        self.yul_number(span, (layout.abi_head_slots() * 32).to_string()),
                    ],
                )),
            ),
        ];
        self.push_return_abi_layout(span, prefix, names, layout, &base, &tail, 0, &mut stmts);
        stmts.push(self.yul_expr_stmt(
            span,
            self.yul_call(
                span,
                "mstore",
                vec![
                    self.yul_number(span, "0x40"),
                    self.yul_ident_expr(span, &tail),
                ],
            ),
        ));
        stmts.push(self.yul_expr_stmt(
            span,
            self.yul_call(
                span,
                "return",
                vec![
                    self.yul_ident_expr(span, &base),
                    self.yul_call(
                        span,
                        "sub",
                        vec![
                            self.yul_ident_expr(span, &tail),
                            self.yul_ident_expr(span, &base),
                        ],
                    ),
                ],
            ),
        ));
        self.assembly_stmt(span, stmts)
    }

    fn push_return_abi_layout(
        &self,
        span: Span<'db>,
        prefix: &str,
        names: &[String],
        layout: &StaticAbiLayout<'db>,
        base: &str,
        tail: &str,
        head_offset: usize,
        stmts: &mut Vec<YulStmt<'db>>,
    ) {
        match &layout.kind {
            StaticAbiLayoutKind::Unit => {}
            StaticAbiLayoutKind::Word(kind) => {
                stmts.push(self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "mstore",
                        vec![
                            self.yul_head_ptr(span, base, head_offset),
                            self.return_abi_word_value(span, &names[0], Some(*kind)),
                        ],
                    ),
                ));
            }
            StaticAbiLayoutKind::BytesLike => {
                self.push_return_bytes_like_layout(
                    span,
                    &format!("{prefix}_head{head_offset}"),
                    &names[0],
                    base,
                    tail,
                    head_offset,
                    stmts,
                );
            }
            StaticAbiLayoutKind::Product(layouts) => {
                let mut name_offset = 0;
                let mut head = head_offset;
                for (index, component) in layouts.iter().enumerate() {
                    let end = name_offset + component.slots;
                    self.push_return_abi_layout(
                        span,
                        &format!("{prefix}_{index}"),
                        &names[name_offset..end],
                        component,
                        base,
                        tail,
                        head,
                        stmts,
                    );
                    name_offset = end;
                    head += component.abi_head_slots();
                }
            }
            StaticAbiLayoutKind::Sum { .. } => {
                for (slot, name) in names.iter().enumerate() {
                    stmts.push(self.yul_expr_stmt(
                        span,
                        self.yul_call(
                            span,
                            "mstore",
                            vec![
                                self.yul_head_ptr(span, base, head_offset + slot),
                                self.yul_ident_expr(span, name),
                            ],
                        ),
                    ));
                }
            }
        }
    }

    fn push_return_bytes_like_layout(
        &self,
        span: Span<'db>,
        prefix: &str,
        name: &str,
        base: &str,
        tail: &str,
        head_offset: usize,
        stmts: &mut Vec<YulStmt<'db>>,
    ) {
        let length = format!("{prefix}_len");
        let total = format!("{prefix}_total");
        let rounded = format!("{prefix}_rounded");
        let padding = format!("{prefix}_padding");
        let offset = format!("{prefix}_offset");
        stmts.push(self.yul_expr_stmt(
            span,
            self.yul_call(
                span,
                "mstore",
                vec![
                    self.yul_head_ptr(span, base, head_offset),
                    self.yul_call(
                        span,
                        "sub",
                        vec![
                            self.yul_ident_expr(span, tail),
                            self.yul_ident_expr(span, base),
                        ],
                    ),
                ],
            ),
        ));
        stmts.push(self.yul_let(
            span,
            &length,
            Some(self.yul_call(span, "mload", vec![self.yul_ident_expr(span, name)])),
        ));
        stmts.push(self.yul_let(
            span,
            &total,
            Some(self.yul_call(
                span,
                "add",
                vec![
                    self.yul_ident_expr(span, &length),
                    self.yul_number(span, "32"),
                ],
            )),
        ));
        stmts.push(YulStmt {
            span,
            kind: YulStmtKind::For {
                init: vec![self.yul_let(span, &offset, Some(self.yul_number(span, "0")))],
                cond: self.yul_call(
                    span,
                    "lt",
                    vec![
                        self.yul_ident_expr(span, &offset),
                        self.yul_ident_expr(span, &total),
                    ],
                ),
                post: vec![self.yul_assign(
                    span,
                    &offset,
                    self.yul_call(
                        span,
                        "add",
                        vec![
                            self.yul_ident_expr(span, &offset),
                            self.yul_number(span, "32"),
                        ],
                    ),
                )],
                body: vec![self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "mstore",
                        vec![
                            self.yul_call(
                                span,
                                "add",
                                vec![
                                    self.yul_ident_expr(span, tail),
                                    self.yul_ident_expr(span, &offset),
                                ],
                            ),
                            self.yul_call(
                                span,
                                "mload",
                                vec![self.yul_call(
                                    span,
                                    "add",
                                    vec![
                                        self.yul_ident_expr(span, name),
                                        self.yul_ident_expr(span, &offset),
                                    ],
                                )],
                            ),
                        ],
                    ),
                )],
            },
        });
        stmts.push(self.yul_let(
            span,
            &rounded,
            Some(self.yul_round_up_to_word(span, self.yul_ident_expr(span, &total))),
        ));
        stmts.push(self.yul_let(
            span,
            &padding,
            Some(self.yul_call(
                span,
                "sub",
                vec![
                    self.yul_ident_expr(span, &rounded),
                    self.yul_ident_expr(span, &total),
                ],
            )),
        ));
        stmts.push(YulStmt {
            span,
            kind: YulStmtKind::If {
                cond: self.yul_ident_expr(span, &padding),
                body: vec![self.yul_expr_stmt(
                    span,
                    self.yul_call(
                        span,
                        "mstore",
                        vec![
                            self.yul_call(
                                span,
                                "add",
                                vec![
                                    self.yul_ident_expr(span, tail),
                                    self.yul_ident_expr(span, &total),
                                ],
                            ),
                            self.yul_number(span, "0"),
                        ],
                    ),
                )],
            },
        });
        stmts.push(self.yul_assign(
            span,
            tail,
            self.yul_call(
                span,
                "add",
                vec![
                    self.yul_ident_expr(span, tail),
                    self.yul_ident_expr(span, &rounded),
                ],
            ),
        ));
    }

    fn return_abi_word_value(
        &self,
        span: Span<'db>,
        name: &str,
        kind: Option<AbiWordKind>,
    ) -> YulExpr<'db> {
        match kind {
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
        }
    }

    fn yul_head_ptr(&self, span: Span<'db>, base: &str, head_offset: usize) -> YulExpr<'db> {
        if head_offset == 0 {
            self.yul_ident_expr(span, base)
        } else {
            self.yul_call(
                span,
                "add",
                vec![
                    self.yul_ident_expr(span, base),
                    self.yul_number(span, (head_offset * 32).to_string()),
                ],
            )
        }
    }

    fn yul_round_up_to_word(&self, span: Span<'db>, value: YulExpr<'db>) -> YulExpr<'db> {
        self.yul_call(
            span,
            "and",
            vec![
                self.yul_call(span, "add", vec![value, self.yul_number(span, "31")]),
                self.yul_call(span, "not", vec![self.yul_number(span, "31")]),
            ],
        )
    }
}
