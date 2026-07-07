use std::collections::{BTreeMap, BTreeSet};

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        function::{BinOp, LitKind, UnOp},
        item::{AdtDef, ContractDef, ContractItem, Item, Module},
        ty::TypeRefKind,
    },
    span::{Span, SpannedElem},
};
use hir_ty::{BuiltinTyCtor, Ty as SemTy, TyCtor, TyKind as SemTyKind, UserTyCtorKind};
use parser::parse_file_to_hir;
use specialize::{
    MonoAbiParam, MonoArm, MonoCallOrigin, MonoContract, MonoEntry, MonoEntryKind, MonoExpr,
    MonoExprKind, MonoFunction, MonoIntrinsic, MonoItem, MonoModule, MonoPat, MonoPatKind,
    MonoStmt, MonoStmtKind,
};

use hir::ast::function::{YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind};

use crate::ir::{
    Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
    StmtKind, Ty, TyKind,
};

const ADDRESS_MASK: &str = "0xffffffffffffffffffffffffffffffffffffffff";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbiWordKind {
    Plain,
    Address,
    Bool,
}

#[derive(Debug, Clone)]
struct StaticAbiLayout<'db> {
    ty: Ty<'db>,
    slots: usize,
    kind: StaticAbiLayoutKind<'db>,
}

#[derive(Debug, Clone)]
enum StaticAbiLayoutKind<'db> {
    Unit,
    Word(AbiWordKind),
    Product(Vec<StaticAbiLayout<'db>>),
    Sum {
        lhs: Box<StaticAbiLayout<'db>>,
        rhs: Box<StaticAbiLayout<'db>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitOptions {
    pub emit_dispatcher_comments: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            emit_dispatcher_comments: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitOutput<'db> {
    pub program: Program<'db>,
    pub diagnostics: Vec<EmitDiagnostic<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitDiagnostic<'db> {
    pub span: Span<'db>,
    pub kind: EmitDiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitDiagnosticKind {
    UnsupportedType { ty: String },
    UnsupportedLiteral { literal: String },
    UnsupportedMonoConstruct { construct: String },
    MissingAdtLayout { adt: String },
    MissingConstructor { constructor: String, ty: String },
    NonExhaustiveMatch,
    MultiScrutineeMatch { count: usize },
    EmptyMatch,
    DispatcherDeferred { contract: String },
    UnsupportedDispatchEntry { signature: String, reason: String },
}

#[derive(Debug, Clone)]
struct AdtLayout<'db> {
    name: String,
    target: Ty<'db>,
    ctors: Vec<CtorLayout<'db>>,
}

#[derive(Debug, Clone)]
struct CtorLayout<'db> {
    name: String,
    payload: Ty<'db>,
    fields: Vec<SemTy<'db>>,
}

#[derive(Debug, Clone)]
struct Branch<'db> {
    binder: String,
    body: Vec<Stmt<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Occurrence(Vec<usize>);

#[derive(Debug, Clone)]
struct MatchColumn<'db> {
    occurrence: Occurrence,
    ty: SemTy<'db>,
    span: Span<'db>,
}

#[derive(Debug, Clone)]
struct MatchRow<'db> {
    pats: Vec<MatrixPat>,
    bindings: Vec<(String, Occurrence)>,
    body: Vec<MonoStmt<'db>>,
}

#[derive(Debug, Clone)]
enum MatrixPat {
    Wildcard,
    Var { name: String },
    Lit { lit: LitKind },
    Con { ctor: String, args: Vec<MatrixPat> },
    Tuple { elems: Vec<MatrixPat> },
    ComptimeLabel,
    Error,
}

#[derive(Debug, Clone)]
enum DecisionTree<'db> {
    Leaf {
        bindings: Vec<(String, Occurrence)>,
        body: Vec<MonoStmt<'db>>,
    },
    Fail {
        span: Span<'db>,
    },
    Product {
        occurrence: Occurrence,
        fields: Vec<Ty<'db>>,
        subtree: Box<DecisionTree<'db>>,
    },
    Switch {
        occurrence: Occurrence,
        layout: AdtLayout<'db>,
        branches: Vec<CtorDecision<'db>>,
        default: Option<Box<DecisionTree<'db>>>,
    },
    AtomicSwitch {
        occurrence: Occurrence,
        target: Ty<'db>,
        branches: Vec<AtomicDecision<'db>>,
        default: Option<Box<DecisionTree<'db>>>,
    },
}

#[derive(Debug, Clone)]
struct CtorDecision<'db> {
    index: usize,
    tree: DecisionTree<'db>,
}

#[derive(Debug, Clone)]
struct AtomicDecision<'db> {
    lit: LitKind,
    tree: DecisionTree<'db>,
}

#[derive(Debug, Clone)]
struct StorageField {
    slot: usize,
}

struct Emitter<'db> {
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    options: EmitOptions,
    diagnostics: Vec<EmitDiagnostic<'db>>,
    scopes: Vec<BTreeMap<String, Expr<'db>>>,
    function_names: BTreeSet<String>,
    layout_stack: Vec<DefId<'db>>,
    fresh: usize,
}

pub fn emit_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: &MonoModule<'db>,
    options: EmitOptions,
) -> EmitOutput<'db> {
    Emitter::new(db, module, options).emit(module)
}

impl<'db> Emitter<'db> {
    fn new(db: &'db dyn hir_ty::Db, module: &MonoModule<'db>, options: EmitOptions) -> Self {
        let hir_module = parse_file_to_hir(db, module.module.file(db)).module(db);
        Self {
            db,
            module: hir_module,
            options,
            diagnostics: Vec::new(),
            scopes: vec![BTreeMap::new()],
            function_names: BTreeSet::new(),
            layout_stack: Vec::new(),
            fresh: 0,
        }
    }

    fn emit(mut self, module: &MonoModule<'db>) -> EmitOutput<'db> {
        let span = self.module.span(self.db);
        let mut functions = BTreeMap::<String, Function<'db>>::new();
        let mut contracts = Vec::new();
        self.function_names = module
            .items
            .iter()
            .filter_map(|item| match item {
                MonoItem::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect();
        for item in &module.items {
            match item {
                MonoItem::Function(function) => {
                    let function = self.emit_function(function);
                    functions.insert(function.name.clone(), function);
                }
                MonoItem::Contract(contract) => contracts.push(contract.clone()),
                MonoItem::Adt(_) => {}
            }
        }

        let program = if contracts.is_empty() {
            Program {
                span,
                functions: functions.into_values().collect(),
                objects: Vec::new(),
            }
        } else {
            let all_functions = functions.values().cloned().collect::<Vec<_>>();
            let objects = contracts
                .iter()
                .map(|contract| self.emit_contract(contract, &all_functions))
                .collect();
            Program {
                span,
                functions: Vec::new(),
                objects,
            }
        };

        EmitOutput {
            program,
            diagnostics: self.diagnostics,
        }
    }

    fn emit_contract(
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

        let storage_fields = self.contract_word_storage_fields(contract.def);

        let deployment_functions = functions
            .iter()
            .filter(|function| constructor_names.contains(&function.name))
            .cloned()
            .map(|function| self.lower_storage_fields_in_function(function, &storage_fields))
            .map(ensure_unit_function_returns)
            .collect::<Vec<_>>();
        let runtime_functions = functions
            .iter()
            .filter(|function| !constructor_names.contains(&function.name))
            .cloned()
            .map(|function| self.lower_storage_fields_in_function(function, &storage_fields))
            .collect::<Vec<_>>();

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

    fn contract_word_storage_fields(&mut self, def: DefId<'db>) -> BTreeMap<String, StorageField> {
        let module = parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
        let Some(contract) = find_contract(self.db, module, def) else {
            return BTreeMap::new();
        };
        contract
            .fields(self.db)
            .iter()
            .enumerate()
            .filter(|(_, field)| field_type_is_word_slot(self.db, field.ty()))
            .map(|(slot, field)| {
                (
                    field.name().atom().text(self.db).to_owned(),
                    StorageField { slot },
                )
            })
            .collect()
    }

    fn lower_storage_fields_in_function(
        &self,
        mut function: Function<'db>,
        fields: &BTreeMap<String, StorageField>,
    ) -> Function<'db> {
        if fields.is_empty() {
            return function;
        }
        let mut lowerer = StorageLowerer::new(self, fields, &function.args);
        function.body = lowerer.stmts(function.body);
        function
    }

    fn emit_dispatcher(
        &mut self,
        contract: &MonoContract<'db>,
        functions: &[Function<'db>],
    ) -> Vec<Stmt<'db>> {
        let dispatch_entries = contract
            .entries
            .iter()
            .filter(|entry| entry.selector.is_some() && matches!(entry.kind, MonoEntryKind::Method))
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
            let Some(selector) = entry.selector else {
                continue;
            };
            let Some(function) = function_map.get(entry.specialized.as_str()).copied() else {
                self.push_unsupported_dispatch_entry(entry, "missing specialized function");
                continue;
            };
            if function.args.len() != entry.inputs.len() {
                self.push_unsupported_dispatch_entry(entry, "ABI/function arity mismatch");
                continue;
            }
            let Some(input_layouts) = dispatcher_input_layouts(function, entry) else {
                self.push_unsupported_dispatch_entry(entry, "non-word ABI shape");
                continue;
            };
            let Some(return_layout) = dispatcher_return_layout(&function.ret, &entry.outputs)
            else {
                self.push_unsupported_dispatch_entry(entry, "non-word ABI shape");
                continue;
            };
            alts.push(Alt {
                span: entry.span,
                pat: Pat {
                    span: entry.span,
                    kind: PatKind::IntLit(selector_hex(selector)),
                },
                binder: self.fresh_alt(),
                body: self.emit_dispatch_entry(
                    entry,
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

    fn push_unsupported_dispatch_entry(&mut self, entry: &MonoEntry<'db>, reason: &str) {
        self.push(
            entry.span,
            EmitDiagnosticKind::UnsupportedDispatchEntry {
                signature: entry
                    .signature
                    .as_deref()
                    .unwrap_or(entry.name.as_str())
                    .to_owned(),
                reason: reason.to_owned(),
            },
        );
    }

    fn emit_dispatch_entry(
        &mut self,
        entry: &MonoEntry<'db>,
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
                body.push(self.return_abi_words(span, &names, &entry.outputs));
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

    fn push_abi_word_cleaning(
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

    fn nonpayable_check(&self, span: Span<'db>) -> Stmt<'db> {
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

    fn assembly_stmt(&self, span: Span<'db>, body: Vec<YulStmt<'db>>) -> Stmt<'db> {
        Stmt {
            span,
            kind: StmtKind::Assembly(body),
        }
    }

    fn yul_assign(&self, span: Span<'db>, name: &str, value: YulExpr<'db>) -> YulStmt<'db> {
        YulStmt {
            span,
            kind: YulStmtKind::Assign {
                names: vec![self.yul_ident(span, name)],
                value,
            },
        }
    }

    fn yul_let(&self, span: Span<'db>, name: &str, init: Option<YulExpr<'db>>) -> YulStmt<'db> {
        YulStmt {
            span,
            kind: YulStmtKind::Let {
                names: vec![self.yul_ident(span, name)],
                init,
            },
        }
    }

    fn yul_expr_stmt(&self, span: Span<'db>, expr: YulExpr<'db>) -> YulStmt<'db> {
        YulStmt {
            span,
            kind: YulStmtKind::Expr(expr),
        }
    }

    fn yul_call(&self, span: Span<'db>, name: &str, args: Vec<YulExpr<'db>>) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Call {
                name: self.yul_ident(span, name),
                args,
            },
        }
    }

    fn yul_number(&self, span: Span<'db>, value: impl Into<String>) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Lit(YulLitKind::Number(value.into())),
        }
    }

    fn yul_string(&self, span: Span<'db>, value: &str) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Lit(YulLitKind::String(format!(
                "\"{}\"",
                value.replace('\\', "\\\\").replace('"', "\\\"")
            ))),
        }
    }

    fn yul_ident_expr(&self, span: Span<'db>, name: &str) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Ident(self.yul_ident(span, name)),
        }
    }

    fn yul_ident(&self, span: Span<'db>, name: &str) -> SpannedElem<'db, Ident<'db>> {
        SpannedElem::new(Ident::new(self.db, name.to_owned()), span)
    }

    fn emit_function(&mut self, function: &MonoFunction<'db>) -> Function<'db> {
        self.with_scope(|this| {
            let args = function
                .params
                .iter()
                .filter_map(|param| {
                    if param.comptime {
                        this.push(
                            param.span,
                            EmitDiagnosticKind::UnsupportedMonoConstruct {
                                construct: format!("comptime parameter `{}`", param.name),
                            },
                        );
                        return None;
                    }
                    let ty = this.hull_ty(param.ty.ty(), param.span);
                    Some(Arg {
                        span: param.span,
                        name: param.name.clone(),
                        ty,
                    })
                })
                .collect::<Vec<_>>();
            let ret = this.hull_ty(function.ret.ty(), function.span);
            let body = this.emit_stmts(&function.body);
            Function {
                span: function.span,
                name: function.name.clone(),
                args,
                ret,
                body,
            }
        })
    }

    fn emit_stmts(&mut self, stmts: &[MonoStmt<'db>]) -> Vec<Stmt<'db>> {
        stmts.iter().flat_map(|stmt| self.emit_stmt(stmt)).collect()
    }

    fn emit_stmt(&mut self, stmt: &MonoStmt<'db>) -> Vec<Stmt<'db>> {
        match &stmt.kind {
            MonoStmtKind::Let { id, ty, init, .. } => {
                let declared = match ty {
                    Some(ty) => self.hull_ty(ty.ty(), stmt.span),
                    None if init.is_none()
                        && sem_ty_needs_untyped_word_default(self.db, id.ty.ty()) =>
                    {
                        Ty::word(stmt.span)
                    }
                    None => self.hull_ty(id.ty.ty(), stmt.span),
                };
                let mut out = vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Let {
                        name: id.name.clone(),
                        ty: declared.clone(),
                    },
                }];
                if let Some(init) = init {
                    let rhs = self.emit_expr(init);
                    out.push(Stmt {
                        span: stmt.span,
                        kind: StmtKind::Assign {
                            lhs: Expr::var(stmt.span, id.name.clone(), declared.clone()),
                            rhs,
                        },
                    });
                }
                self.bind_expr(
                    id.name.clone(),
                    Expr::var(id.span, id.name.clone(), declared.clone()),
                );
                out
            }
            MonoStmtKind::Return(expr) => {
                let expr = expr
                    .as_ref()
                    .map(|expr| self.emit_expr(expr))
                    .unwrap_or_else(|| Expr::unit(stmt.span));
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Return(expr),
                }]
            }
            MonoStmtKind::Expr(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Expr(self.emit_expr(expr)),
            }],
            MonoStmtKind::Assign { lhs, rhs } => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Assign {
                    lhs: self.emit_expr(lhs),
                    rhs: self.emit_expr(rhs),
                },
            }],
            MonoStmtKind::AddAssign { lhs, rhs } => self.emit_assign_op(stmt.span, lhs, "add", rhs),
            MonoStmtKind::SubAssign { lhs, rhs } => self.emit_assign_op(stmt.span, lhs, "sub", rhs),
            MonoStmtKind::BitXorAssign { lhs, rhs } => {
                self.emit_assign_op(stmt.span, lhs, "xor", rhs)
            }
            MonoStmtKind::BitAndAssign { lhs, rhs } => {
                self.emit_assign_op(stmt.span, lhs, "and", rhs)
            }
            MonoStmtKind::BitOrAssign { lhs, rhs } => {
                self.emit_assign_op(stmt.span, lhs, "or", rhs)
            }
            MonoStmtKind::ModAssign { lhs, rhs } => self.emit_assign_op(stmt.span, lhs, "mod", rhs),
            MonoStmtKind::Match { scrutinees, arms } => {
                self.emit_match(stmt.span, scrutinees, arms)
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => vec![self.emit_if_stmt(stmt.span, cond, then_body, else_body.as_deref())],
            MonoStmtKind::Block(body) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Block(self.with_scope(|this| this.emit_stmts(body))),
            }],
            MonoStmtKind::Assembly(body) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Assembly(body.clone()),
            }],
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::For {
                        init: self.with_scope(|this| this.emit_stmts(init)),
                        cond: self.emit_expr(cond),
                        post: self.with_scope(|this| this.emit_stmts(post)),
                        body: self.with_scope(|this| this.emit_stmts(body)),
                    },
                }]
            }
            MonoStmtKind::Break => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Break,
            }],
            MonoStmtKind::Continue => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Continue,
            }],
            MonoStmtKind::Error => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Revert("error statement".to_owned()),
            }],
        }
    }

    fn emit_assign_op(
        &mut self,
        span: Span<'db>,
        lhs: &MonoExpr<'db>,
        callee: &str,
        rhs: &MonoExpr<'db>,
    ) -> Vec<Stmt<'db>> {
        let lhs_expr = self.emit_expr(lhs);
        let rhs_expr = self.emit_expr(rhs);
        let call = Expr {
            span,
            ty: lhs_expr.ty.clone(),
            kind: ExprKind::Call {
                callee: callee.to_owned(),
                args: vec![lhs_expr.clone(), rhs_expr],
            },
        };
        vec![Stmt {
            span,
            kind: StmtKind::Assign {
                lhs: lhs_expr,
                rhs: call,
            },
        }]
    }

    fn emit_if_stmt(
        &mut self,
        span: Span<'db>,
        cond: &MonoExpr<'db>,
        then_body: &[MonoStmt<'db>],
        else_body: Option<&[MonoStmt<'db>]>,
    ) -> Stmt<'db> {
        let target = self.hull_ty(cond.ty.ty(), cond.span);
        let scrutinee = self.emit_expr(cond);
        let then_stmts = self.with_scope(|this| this.emit_stmts(then_body));
        let else_stmts = else_body
            .map(|body| self.with_scope(|this| this.emit_stmts(body)))
            .unwrap_or_default();
        Stmt {
            span,
            kind: StmtKind::Match {
                target,
                scrutinee,
                alts: vec![
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inr),
                        },
                        binder: self.fresh_alt(),
                        body: then_stmts,
                    },
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inl),
                        },
                        binder: self.fresh_alt(),
                        body: else_stmts,
                    },
                ],
            },
        }
    }

    fn emit_expr(&mut self, expr: &MonoExpr<'db>) -> Expr<'db> {
        if let MonoExprKind::Var(id) = &expr.kind {
            if let Some(expr) = self.lookup_expr(&id.name) {
                return expr;
            }
            let ty = self.hull_ty(expr.ty.ty(), expr.span);
            return Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Var(id.name.clone()),
            };
        }
        let ty = self.hull_ty(expr.ty.ty(), expr.span);
        match &expr.kind {
            MonoExprKind::Var(_) => unreachable!("variable expressions return above"),
            MonoExprKind::Lit(lit) => self.emit_lit(expr.span, lit),
            MonoExprKind::Tuple(elems) => {
                let elems = elems
                    .iter()
                    .map(|elem| self.emit_expr(elem))
                    .collect::<Vec<_>>();
                product_expr(expr.span, ty, elems)
            }
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: call_name(origin, &callee.name),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            },
            MonoExprKind::Con { ctor, args } => self.emit_constructor(expr, &ctor.name, args),
            MonoExprKind::BinOp { lhs, op, rhs } => self.emit_bin_op(expr.span, ty, lhs, *op, rhs),
            MonoExprKind::UnaryOp { op, expr: inner } => {
                self.emit_unary_op(expr.span, ty, *op, inner)
            }
            MonoExprKind::TypeAnnot { expr: inner, .. } => self.emit_expr(inner),
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => Expr {
                span: expr.span,
                ty: ty.clone(),
                kind: ExprKind::If {
                    target: ty,
                    cond: Box::new(self.emit_expr(cond)),
                    then_expr: Box::new(self.emit_expr(then_expr)),
                    else_expr: Box::new(self.emit_expr(else_expr)),
                },
            },
            MonoExprKind::ClosureDispatch { callee, args } => {
                if let Some(callee_name) = self.closure_callee_name(callee) {
                    Expr {
                        span: expr.span,
                        ty,
                        kind: ExprKind::Call {
                            callee: callee_name,
                            args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                        },
                    }
                } else {
                    self.push(
                        expr.span,
                        EmitDiagnosticKind::UnsupportedMonoConstruct {
                            construct: mono_expr_name(&expr.kind).to_owned(),
                        },
                    );
                    Expr {
                        span: expr.span,
                        ty,
                        kind: ExprKind::Call {
                            callee: "unsupported".to_owned(),
                            args: Vec::new(),
                        },
                    }
                }
            }
            MonoExprKind::Field { .. }
            | MonoExprKind::Index { .. }
            | MonoExprKind::Proxy(_)
            | MonoExprKind::Lambda { .. }
            | MonoExprKind::Error => {
                self.push(
                    expr.span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: mono_expr_name(&expr.kind).to_owned(),
                    },
                );
                Expr {
                    span: expr.span,
                    ty,
                    kind: ExprKind::Call {
                        callee: "unsupported".to_owned(),
                        args: Vec::new(),
                    },
                }
            }
        }
    }

    fn closure_callee_name(&self, callee: &MonoExpr<'db>) -> Option<String> {
        let name = match &callee.kind {
            MonoExprKind::Var(id) => &id.name,
            MonoExprKind::Lambda { name } => name,
            MonoExprKind::TypeAnnot { expr, .. } => return self.closure_callee_name(expr),
            _ => return None,
        };
        self.function_names.contains(name).then(|| name.clone())
    }

    fn emit_lit(&mut self, span: Span<'db>, lit: &LitKind) -> Expr<'db> {
        match lit {
            LitKind::Number(value) | LitKind::Hex(value) => Expr::word(span, value.clone()),
            LitKind::String(value) => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedLiteral {
                        literal: value.clone(),
                    },
                );
                Expr::word(span, "0")
            }
            LitKind::Error => Expr::word(span, "0"),
        }
    }

    fn emit_constructor(
        &mut self,
        expr: &MonoExpr<'db>,
        ctor_name: &str,
        args: &[MonoExpr<'db>],
    ) -> Expr<'db> {
        let target = if sem_ty_needs_untyped_word_default(self.db, expr.ty.ty()) {
            Ty::word(expr.span)
        } else {
            self.hull_ty(expr.ty.ty(), expr.span)
        };
        match ctor_name {
            "()" => return Expr::unit(expr.span),
            "pair" => {
                let args = args.iter().map(|arg| self.emit_expr(arg)).collect();
                return product_expr(expr.span, target, args);
            }
            "true" => {
                let payload = Expr::unit(expr.span);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: ExprKind::Inr {
                        target,
                        value: Box::new(payload),
                    },
                };
            }
            "false" => {
                let payload = Expr::unit(expr.span);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: ExprKind::Inl {
                        target,
                        value: Box::new(payload),
                    },
                };
            }
            "inl" | "inr" if args.len() == 1 => {
                let value = self.emit_expr(&args[0]);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: if ctor_name == "inl" {
                        ExprKind::Inl {
                            target,
                            value: Box::new(value),
                        }
                    } else {
                        ExprKind::Inr {
                            target,
                            value: Box::new(value),
                        }
                    },
                };
            }
            "uint256" | "uint" | "bytes32" | "address" if args.len() == 1 => {
                let mut value = self.emit_expr(&args[0]);
                value.ty = if sem_ty_needs_untyped_word_default(self.db, expr.ty.ty()) {
                    Ty::word(expr.span)
                } else {
                    target
                };
                return value;
            }
            _ => {}
        }

        let Some(layout) = self.adt_layout_for_sem_ty(expr.ty.ty(), expr.span) else {
            self.push(
                expr.span,
                EmitDiagnosticKind::MissingAdtLayout {
                    adt: expr.ty.ty().display(self.db),
                },
            );
            return Expr {
                span: expr.span,
                ty: target,
                kind: ExprKind::Call {
                    callee: ctor_name.to_owned(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            };
        };
        let Some(index) = layout
            .ctors
            .iter()
            .position(|ctor| constructor_name_matches(ctor_name, &layout.name, &ctor.name))
        else {
            self.push(
                expr.span,
                EmitDiagnosticKind::MissingConstructor {
                    constructor: ctor_name.to_owned(),
                    ty: layout.name,
                },
            );
            return Expr {
                span: expr.span,
                ty: target,
                kind: ExprKind::Call {
                    callee: ctor_name.to_owned(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            };
        };
        let payload_ty = layout.ctors[index].payload.clone();
        let payload_args = args
            .iter()
            .map(|arg| self.emit_expr(arg))
            .collect::<Vec<_>>();
        let payload = product_expr(expr.span, payload_ty, payload_args);
        encode_constructor(expr.span, layout.target, index, payload)
    }

    fn emit_bin_op(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        lhs: &MonoExpr<'db>,
        op: BinOp,
        rhs: &MonoExpr<'db>,
    ) -> Expr<'db> {
        let Some(callee) = bin_op_name(op) else {
            self.push(
                span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: format!("binary operator {op:?}"),
                },
            );
            return Expr {
                span,
                ty,
                kind: ExprKind::Call {
                    callee: "unsupported".to_owned(),
                    args: Vec::new(),
                },
            };
        };
        Expr {
            span,
            ty,
            kind: ExprKind::Call {
                callee: callee.to_owned(),
                args: vec![self.emit_expr(lhs), self.emit_expr(rhs)],
            },
        }
    }

    fn emit_unary_op(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        op: UnOp,
        expr: &MonoExpr<'db>,
    ) -> Expr<'db> {
        match op {
            UnOp::Not => {
                let false_expr = Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Inl {
                        target: ty.clone(),
                        value: Box::new(Expr::unit(span)),
                    },
                };
                let true_expr = Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Inr {
                        target: ty.clone(),
                        value: Box::new(Expr::unit(span)),
                    },
                };
                Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::If {
                        target: ty,
                        cond: Box::new(self.emit_expr(expr)),
                        then_expr: Box::new(false_expr),
                        else_expr: Box::new(true_expr),
                    },
                }
            }
            UnOp::Error => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: "unary error".to_owned(),
                    },
                );
                Expr {
                    span,
                    ty,
                    kind: ExprKind::Call {
                        callee: "unsupported".to_owned(),
                        args: Vec::new(),
                    },
                }
            }
        }
    }

    fn emit_match(
        &mut self,
        span: Span<'db>,
        scrutinees: &[MonoExpr<'db>],
        arms: &[MonoArm<'db>],
    ) -> Vec<Stmt<'db>> {
        if scrutinees.is_empty() {
            self.push(span, EmitDiagnosticKind::EmptyMatch);
            return vec![Stmt {
                span,
                kind: StmtKind::Revert("empty match".to_owned()),
            }];
        }
        if arms.is_empty() {
            self.push(span, EmitDiagnosticKind::EmptyMatch);
            return vec![Stmt {
                span,
                kind: StmtKind::Revert("empty match".to_owned()),
            }];
        }

        let scrutinee_exprs = scrutinees
            .iter()
            .map(|scrutinee| self.emit_expr(scrutinee))
            .collect::<Vec<_>>();
        let columns = scrutinees
            .iter()
            .enumerate()
            .map(|(index, scrutinee)| MatchColumn {
                occurrence: Occurrence(vec![index]),
                ty: scrutinee.ty.ty(),
                span: scrutinee.span,
            })
            .collect::<Vec<_>>();
        let rows = arms
            .iter()
            .filter_map(|arm| {
                if arm.pats.len() != scrutinees.len() {
                    self.push(
                        arm.span,
                        EmitDiagnosticKind::UnsupportedMonoConstruct {
                            construct: "match arm arity mismatch".to_owned(),
                        },
                    );
                    return None;
                }
                Some(MatchRow {
                    pats: arm.pats.iter().map(matrix_pat).collect(),
                    bindings: Vec::new(),
                    body: arm.body.clone(),
                })
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            self.push(span, EmitDiagnosticKind::EmptyMatch);
            return vec![Stmt {
                span,
                kind: StmtKind::Revert("empty match".to_owned()),
            }];
        }

        let tree = self.compile_match_matrix(span, columns.clone(), rows);
        let mut occurrences = columns
            .into_iter()
            .zip(scrutinee_exprs)
            .map(|(column, expr)| (column.occurrence, expr))
            .collect::<BTreeMap<_, _>>();
        self.tree_to_body(span, &mut occurrences, &tree)
    }

    fn compile_match_matrix(
        &mut self,
        span: Span<'db>,
        columns: Vec<MatchColumn<'db>>,
        rows: Vec<MatchRow<'db>>,
    ) -> DecisionTree<'db> {
        if rows.is_empty() {
            self.push(span, EmitDiagnosticKind::NonExhaustiveMatch);
            return DecisionTree::Fail { span };
        }
        if columns.is_empty() {
            let row = rows.into_iter().next().expect("row exists");
            return DecisionTree::Leaf {
                bindings: row.bindings,
                body: row.body,
            };
        }
        if rows[0].pats.iter().all(MatrixPat::is_var_like) {
            let row = rows.into_iter().next().expect("row exists");
            let mut bindings = row.bindings;
            for (pat, column) in row.pats.iter().zip(&columns) {
                if let MatrixPat::Var { name, .. } = pat {
                    bindings.push((name.clone(), column.occurrence.clone()));
                }
            }
            return DecisionTree::Leaf {
                bindings,
                body: row.body,
            };
        }

        let selected = select_match_column(&columns, &rows);
        let columns = reorder_columns(columns, selected);
        let rows = reorder_rows(rows, selected);
        let test = columns[0].clone();
        let rest = columns[1..].to_vec();
        let first_col = rows
            .iter()
            .filter_map(|row| row.pats.first())
            .collect::<Vec<_>>();

        if let Some(product) = self.compile_product_column(span, &test, &rest, &rows, &first_col) {
            return product;
        }

        let head_ctors = head_constructor_indices(
            self.adt_layout_for_sem_ty(test.ty, test.span).as_ref(),
            &first_col,
        );
        if !head_ctors.is_empty() {
            return self.compile_constructor_switch(span, test, rest, rows, head_ctors);
        }

        let head_lits = head_literals(&first_col);
        if !head_lits.is_empty() {
            return self.compile_atomic_switch(span, test, rest, rows, head_lits);
        }

        if first_col
            .iter()
            .any(|pat| matches!(pat, MatrixPat::ComptimeLabel))
        {
            self.push(
                span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: "unevaluated comptime match label".to_owned(),
                },
            );
            return DecisionTree::Fail { span };
        }

        let (rows, columns) = default_rows(test.occurrence, rows, rest);
        self.compile_match_matrix(span, columns, rows)
    }

    fn compile_product_column(
        &mut self,
        span: Span<'db>,
        test: &MatchColumn<'db>,
        rest: &[MatchColumn<'db>],
        rows: &[MatchRow<'db>],
        first_col: &[&MatrixPat],
    ) -> Option<DecisionTree<'db>> {
        let tuple_fields = first_col
            .iter()
            .any(|pat| matches!(pat, MatrixPat::Tuple { .. }))
            .then(|| sem_product_fields(self.db, test.ty));
        let single_ctor_layout = self
            .adt_layout_for_sem_ty(test.ty, test.span)
            .filter(|layout| layout.ctors.len() == 1);
        let fields = match (tuple_fields, single_ctor_layout) {
            (Some(fields), _) => fields,
            (None, Some(layout))
                if first_col
                    .iter()
                    .any(|pat| matches!(pat, MatrixPat::Con { .. })) =>
            {
                layout.ctors[0].fields.clone()
            }
            _ => return None,
        };

        let child_columns = child_columns(&test.occurrence, &fields, test.span);
        let mut next_columns = child_columns;
        next_columns.extend_from_slice(rest);
        let mut next_rows = Vec::new();
        for row in rows.iter().cloned() {
            let (first, row_rest) = split_row(row);
            match first {
                MatrixPat::Tuple { elems, .. } => {
                    next_rows.push(row_with_pats(row_rest, elems));
                }
                MatrixPat::Con { ctor, args, .. } if self.single_ctor_matches(test.ty, &ctor) => {
                    next_rows.push(row_with_pats(row_rest, args));
                }
                MatrixPat::Var { name, .. } => {
                    next_rows.push(row_with_binding_and_wildcards(
                        row_rest,
                        name,
                        test.occurrence.clone(),
                        fields.len(),
                        test.span,
                    ));
                }
                MatrixPat::Wildcard => {
                    next_rows.push(row_with_wildcards(row_rest, fields.len(), test.span));
                }
                MatrixPat::Error => {
                    next_rows.push(row_with_wildcards(row_rest, fields.len(), test.span));
                }
                MatrixPat::Con { .. } | MatrixPat::Lit { .. } | MatrixPat::ComptimeLabel => {}
            }
        }

        let field_tys = fields
            .iter()
            .map(|field| self.hull_ty(*field, test.span))
            .collect();
        Some(DecisionTree::Product {
            occurrence: test.occurrence.clone(),
            fields: field_tys,
            subtree: Box::new(self.compile_match_matrix(span, next_columns, next_rows)),
        })
    }

    fn compile_constructor_switch(
        &mut self,
        span: Span<'db>,
        test: MatchColumn<'db>,
        rest: Vec<MatchColumn<'db>>,
        rows: Vec<MatchRow<'db>>,
        head_ctors: Vec<usize>,
    ) -> DecisionTree<'db> {
        let Some(layout) = self.adt_layout_for_sem_ty(test.ty, test.span) else {
            self.push(
                test.span,
                EmitDiagnosticKind::MissingAdtLayout {
                    adt: test.ty.display(self.db),
                },
            );
            return DecisionTree::Fail { span };
        };
        let mut branches = Vec::new();
        for index in head_ctors.iter().copied() {
            let ctor = &layout.ctors[index];
            let child_cols = child_columns(&test.occurrence, &ctor.fields, test.span);
            let mut next_columns = child_cols;
            next_columns.extend(rest.clone());
            let mut next_rows = Vec::new();
            for row in rows.iter().cloned() {
                let (first, row_rest) = split_row(row);
                match first {
                    MatrixPat::Con {
                        ctor: name, args, ..
                    } if constructor_name_matches(&name, &layout.name, &ctor.name) => {
                        next_rows.push(row_with_pats(row_rest, args));
                    }
                    MatrixPat::Var { name, .. } => {
                        next_rows.push(row_with_binding_and_wildcards(
                            row_rest,
                            name,
                            test.occurrence.clone(),
                            ctor.fields.len(),
                            test.span,
                        ));
                    }
                    MatrixPat::Wildcard => {
                        next_rows.push(row_with_wildcards(row_rest, ctor.fields.len(), test.span));
                    }
                    MatrixPat::Error => {
                        next_rows.push(row_with_wildcards(row_rest, ctor.fields.len(), test.span));
                    }
                    MatrixPat::Con { .. }
                    | MatrixPat::Tuple { .. }
                    | MatrixPat::Lit { .. }
                    | MatrixPat::ComptimeLabel => {}
                }
            }
            branches.push(CtorDecision {
                index,
                tree: self.compile_match_matrix(span, next_columns, next_rows),
            });
        }

        let default = if head_ctors.len() == layout.ctors.len() {
            None
        } else {
            let (default_rows, default_columns) = default_rows(test.occurrence.clone(), rows, rest);
            Some(Box::new(self.compile_match_matrix(
                span,
                default_columns,
                default_rows,
            )))
        };

        DecisionTree::Switch {
            occurrence: test.occurrence,
            layout,
            branches,
            default,
        }
    }

    fn compile_atomic_switch(
        &mut self,
        span: Span<'db>,
        test: MatchColumn<'db>,
        rest: Vec<MatchColumn<'db>>,
        rows: Vec<MatchRow<'db>>,
        head_lits: Vec<LitKind>,
    ) -> DecisionTree<'db> {
        let mut branches = Vec::new();
        for lit in head_lits {
            let mut next_rows = Vec::new();
            for row in rows.iter().cloned() {
                let (first, row_rest) = split_row(row);
                match first {
                    MatrixPat::Lit { lit: candidate, .. } if candidate == lit => {
                        next_rows.push(row_rest);
                    }
                    MatrixPat::Var { name, .. } => {
                        let mut row_rest = row_rest;
                        row_rest.bindings.push((name, test.occurrence.clone()));
                        next_rows.push(row_rest);
                    }
                    MatrixPat::Wildcard | MatrixPat::Error => {
                        next_rows.push(row_rest);
                    }
                    MatrixPat::Lit { .. }
                    | MatrixPat::Con { .. }
                    | MatrixPat::Tuple { .. }
                    | MatrixPat::ComptimeLabel => {}
                }
            }
            branches.push(AtomicDecision {
                lit,
                tree: self.compile_match_matrix(span, rest.clone(), next_rows),
            });
        }

        let (default_rows, default_columns) = default_rows(test.occurrence.clone(), rows, rest);
        let default = Some(Box::new(self.compile_match_matrix(
            span,
            default_columns,
            default_rows,
        )));

        DecisionTree::AtomicSwitch {
            occurrence: test.occurrence,
            target: self.hull_ty(test.ty, test.span),
            branches,
            default,
        }
    }

    fn single_ctor_matches(&mut self, ty: SemTy<'db>, ctor: &str) -> bool {
        self.adt_layout_for_sem_ty(ty, self.module.span(self.db))
            .filter(|layout| layout.ctors.len() == 1)
            .is_some_and(|layout| {
                constructor_name_matches(ctor, &layout.name, &layout.ctors[0].name)
            })
    }

    fn tree_to_body(
        &mut self,
        span: Span<'db>,
        occurrences: &mut BTreeMap<Occurrence, Expr<'db>>,
        tree: &DecisionTree<'db>,
    ) -> Vec<Stmt<'db>> {
        match tree {
            DecisionTree::Leaf { bindings, body } => self.with_scope(|this| {
                let mut materialized = Vec::new();
                for (name, occurrence) in bindings {
                    if let Some(expr) = occurrences.get(occurrence).cloned() {
                        materialized.push(Stmt {
                            span,
                            kind: StmtKind::Let {
                                name: name.clone(),
                                ty: expr.ty.clone(),
                            },
                        });
                        materialized.push(Stmt {
                            span,
                            kind: StmtKind::Assign {
                                lhs: Expr::var(span, name.clone(), expr.ty.clone()),
                                rhs: expr.clone(),
                            },
                        });
                        this.bind_expr(name.clone(), Expr::var(span, name.clone(), expr.ty));
                    }
                }
                materialized.extend(this.emit_stmts(body));
                materialized
            }),
            DecisionTree::Fail { span } => vec![Stmt {
                span: *span,
                kind: StmtKind::Revert("non-exhaustive match".to_owned()),
            }],
            DecisionTree::Product {
                occurrence,
                fields,
                subtree,
            } => {
                let Some(base) = occurrences.get(occurrence).cloned() else {
                    return vec![Stmt {
                        span,
                        kind: StmtKind::Revert("missing product occurrence".to_owned()),
                    }];
                };
                let mut next = occurrences.clone();
                for (index, expr) in product_field_exprs(base, fields).into_iter().enumerate() {
                    let mut child = occurrence.0.clone();
                    child.push(index);
                    next.insert(Occurrence(child), expr);
                }
                self.tree_to_body(span, &mut next, subtree)
            }
            DecisionTree::Switch {
                occurrence,
                layout,
                branches,
                default,
            } => {
                let stmt = self.switch_tree_to_stmt(
                    span,
                    occurrences,
                    occurrence,
                    layout,
                    branches,
                    default.as_deref(),
                );
                vec![stmt]
            }
            DecisionTree::AtomicSwitch {
                occurrence,
                target,
                branches,
                default,
            } => {
                let stmt = self.atomic_tree_to_stmt(
                    span,
                    occurrences,
                    occurrence,
                    target.clone(),
                    branches,
                    default.as_deref(),
                );
                vec![stmt]
            }
        }
    }

    fn switch_tree_to_stmt(
        &mut self,
        span: Span<'db>,
        occurrences: &BTreeMap<Occurrence, Expr<'db>>,
        occurrence: &Occurrence,
        layout: &AdtLayout<'db>,
        decisions: &[CtorDecision<'db>],
        default: Option<&DecisionTree<'db>>,
    ) -> Stmt<'db> {
        let Some(scrutinee) = occurrences.get(occurrence).cloned() else {
            return Stmt {
                span,
                kind: StmtKind::Revert("missing switch occurrence".to_owned()),
            };
        };
        let mut branches = Vec::new();
        for (index, ctor) in layout.ctors.iter().enumerate() {
            let binder = self.fresh_alt();
            let payload = Expr::var(span, binder.clone(), ctor.payload.clone());
            let body_tree = decisions
                .iter()
                .find(|decision| decision.index == index)
                .map(|decision| &decision.tree)
                .or(default);
            let body = if let Some(tree) = body_tree {
                let mut next = occurrences.clone();
                for (field_index, expr) in product_field_exprs(
                    payload.clone(),
                    &ctor
                        .fields
                        .iter()
                        .map(|field| self.hull_ty(*field, span))
                        .collect::<Vec<_>>(),
                )
                .into_iter()
                .enumerate()
                {
                    let mut child = occurrence.0.clone();
                    child.push(field_index);
                    next.insert(Occurrence(child), expr);
                }
                let mut body = self.tree_to_body(span, &mut next, tree);
                if decisions.iter().any(|decision| decision.index == index) {
                    body.insert(
                        0,
                        Stmt {
                            span,
                            kind: StmtKind::Comment(source_constructor_comment(&ctor.name)),
                        },
                    );
                }
                body
            } else {
                vec![Stmt {
                    span,
                    kind: StmtKind::Revert(format!("unreachable constructor: {}", ctor.name)),
                }]
            };
            branches.push(Branch { binder, body });
        }
        build_nested_sum_match(span, scrutinee, layout.target.clone(), branches)
    }

    fn atomic_tree_to_stmt(
        &mut self,
        span: Span<'db>,
        occurrences: &mut BTreeMap<Occurrence, Expr<'db>>,
        occurrence: &Occurrence,
        target: Ty<'db>,
        branches: &[AtomicDecision<'db>],
        default: Option<&DecisionTree<'db>>,
    ) -> Stmt<'db> {
        let Some(scrutinee) = occurrences.get(occurrence).cloned() else {
            return Stmt {
                span,
                kind: StmtKind::Revert("missing atomic occurrence".to_owned()),
            };
        };
        let mut alts = branches
            .iter()
            .map(|branch| Alt {
                span,
                pat: Pat {
                    span,
                    kind: hull_lit_pat(&branch.lit),
                },
                binder: self.fresh_alt(),
                body: self.tree_to_body(span, occurrences, &branch.tree),
            })
            .collect::<Vec<_>>();
        if let Some(default) = default {
            alts.push(Alt {
                span,
                pat: Pat {
                    span,
                    kind: PatKind::Wildcard,
                },
                binder: self.fresh_alt(),
                body: self.tree_to_body(span, occurrences, default),
            });
        }
        Stmt {
            span,
            kind: StmtKind::Match {
                target,
                scrutinee,
                alts,
            },
        }
    }

    fn hull_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Ty<'db> {
        match self.try_hull_ty(ty, span) {
            Some(ty) => ty,
            None => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedType {
                        ty: ty.display(self.db),
                    },
                );
                Ty::word(span)
            }
        }
    }

    fn try_hull_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Option<Ty<'db>> {
        match ty.kind(self.db) {
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
                args,
            } if args.is_empty() => Some(Ty::word(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => Some(Ty::unit(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(bool_sum_ty(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => Some(Ty::product(
                span,
                self.hull_ty(args[0], span),
                self.hull_ty(args[1], span),
            )),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(Ty::sum(
                span,
                self.hull_ty(args[0], span),
                self.hull_ty(args[1], span),
            )),
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if matches!(user.kind, UserTyCtorKind::Adt) => {
                let layout = self.adt_layout(user.def, args, span)?;
                Some(layout.target)
            }
            SemTyKind::Function { params, ret } => Some(Ty::function(
                span,
                params
                    .iter()
                    .map(|param| self.hull_ty(*param, span))
                    .collect(),
                self.hull_ty(*ret, span),
            )),
            SemTyKind::Tuple(elems) => Some(tuple_ty(
                span,
                elems.iter().map(|elem| self.hull_ty(*elem, span)).collect(),
            )),
            SemTyKind::Comptime(inner) => self.try_hull_ty(*inner, span),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Integer | BuiltinTyCtor::String),
                ..
            }
            | SemTyKind::Named { .. }
            | SemTyKind::BoundVar(_) => None,
            SemTyKind::Error | SemTyKind::Unknown => Some(Ty::word(span)),
        }
    }

    fn adt_layout_for_sem_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Option<AdtLayout<'db>> {
        match ty.kind(self.db) {
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(AdtLayout {
                name: "Bool".to_owned(),
                target: bool_sum_ty(span),
                ctors: vec![
                    CtorLayout {
                        name: "false".to_owned(),
                        payload: Ty::unit(span),
                        fields: Vec::new(),
                    },
                    CtorLayout {
                        name: "true".to_owned(),
                        payload: Ty::unit(span),
                        fields: Vec::new(),
                    },
                ],
            }),
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if matches!(user.kind, UserTyCtorKind::Adt) => self.adt_layout(user.def, args, span),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(AdtLayout {
                name: "sum".to_owned(),
                target: self.hull_ty(ty, span),
                ctors: vec![
                    CtorLayout {
                        name: "inl".to_owned(),
                        payload: self.hull_ty(args[0], span),
                        fields: vec![args[0]],
                    },
                    CtorLayout {
                        name: "inr".to_owned(),
                        payload: self.hull_ty(args[1], span),
                        fields: vec![args[1]],
                    },
                ],
            }),
            _ => None,
        }
    }

    fn adt_layout(
        &mut self,
        def: DefId<'db>,
        args: &[SemTy<'db>],
        span: Span<'db>,
    ) -> Option<AdtLayout<'db>> {
        let module = parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
        let adt = find_adt(self.db, module, def)?;
        let name = def.name(self.db).unwrap_or_else(|| "Adt".to_owned());
        if self.layout_stack.contains(&def) {
            return Some(AdtLayout {
                name: name.clone(),
                target: Ty::named_ref(span, name),
                ctors: Vec::new(),
            });
        }

        self.layout_stack.push(def);
        let Some(plan) = hir_ty::derived_generic_plan(self.db, module, adt) else {
            self.layout_stack.pop();
            return None;
        };
        let rep = subst_sem_ty(self.db, plan.rep, args);
        let inner = self.hull_ty(rep, span);
        let target = Ty::named(span, name.clone(), inner);
        let ctors = plan
            .from_arms
            .iter()
            .map(|arm| CtorLayout {
                name: arm.ctor_name.clone(),
                payload: self.hull_ty(subst_sem_ty(self.db, arm.product_rep, args), span),
                fields: sem_product_fields(self.db, subst_sem_ty(self.db, arm.product_rep, args)),
            })
            .collect();
        self.layout_stack.pop();
        Some(AdtLayout {
            name,
            target,
            ctors,
        })
    }

    fn fresh_alt(&mut self) -> String {
        let name = format!("$alt{}", self.fresh);
        self.fresh += 1;
        name
    }

    fn bind_expr(&mut self, name: String, expr: Expr<'db>) {
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .insert(name, expr);
    }

    fn lookup_expr(&self, name: &str) -> Option<Expr<'db>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn with_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.scopes.push(BTreeMap::new());
        let out = f(self);
        self.scopes.pop();
        out
    }

    fn push(&mut self, span: Span<'db>, kind: EmitDiagnosticKind) {
        self.diagnostics.push(EmitDiagnostic { span, kind });
    }
}

fn sem_ty_needs_untyped_word_default<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>) -> bool {
    matches!(ty.kind(db), SemTyKind::Error | SemTyKind::Unknown)
}

struct StorageLowerer<'a, 'db> {
    emitter: &'a Emitter<'db>,
    fields: &'a BTreeMap<String, StorageField>,
    shadows: Vec<BTreeSet<String>>,
    fresh: usize,
}

impl<'a, 'db> StorageLowerer<'a, 'db> {
    fn new(
        emitter: &'a Emitter<'db>,
        fields: &'a BTreeMap<String, StorageField>,
        args: &[Arg<'db>],
    ) -> Self {
        Self {
            emitter,
            fields,
            shadows: vec![args.iter().map(|arg| arg.name.clone()).collect()],
            fresh: 0,
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
                self.shadows
                    .last_mut()
                    .expect("storage scope stack is never empty")
                    .insert(name.clone());
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Let { name, ty },
                }]
            }
            StmtKind::Assign { lhs, rhs } => {
                if let ExprKind::Var(name) = &lhs.kind
                    && let Some(slot) = self.field(name).map(|field| field.slot)
                {
                    let rhs = self.expr(rhs);
                    let temp = self.fresh_temp(name);
                    return vec![
                        Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: temp.clone(),
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
                .expect("storage scope stack is never empty")
                .insert(alt.binder.clone());
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
                if let Some(slot) = self.field(&name).map(|field| field.slot) {
                    Expr {
                        span: expr.span,
                        ty: expr.ty,
                        kind: ExprKind::Call {
                            callee: "sload".to_owned(),
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

    fn fresh_temp(&mut self, field: &str) -> String {
        let name = format!("storage_store_{field}_{}", self.fresh);
        self.fresh += 1;
        name
    }

    fn with_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.shadows.push(BTreeSet::new());
        let out = f(self);
        self.shadows.pop();
        out
    }
}

fn call_name(origin: &MonoCallOrigin<'_>, name: &str) -> String {
    match origin {
        MonoCallOrigin::Builtin(intrinsic) => intrinsic_name(*intrinsic).to_owned(),
        MonoCallOrigin::Source(_) | MonoCallOrigin::Unknown => name.to_owned(),
    }
}

fn constructor_inputs_are_static_word(contract: &MonoContract<'_>) -> bool {
    contract
        .constructor
        .inputs
        .iter()
        .all(abi_param_is_static_word)
}

fn dispatcher_input_layouts<'db>(
    function: &Function<'db>,
    entry: &MonoEntry<'db>,
) -> Option<Vec<StaticAbiLayout<'db>>> {
    function
        .args
        .iter()
        .zip(&entry.inputs)
        .map(|(arg, param)| static_abi_layout_for_param(&arg.ty, param))
        .collect()
}

fn dispatcher_return_layout<'db>(
    ret: &Ty<'db>,
    outputs: &[MonoAbiParam],
) -> Option<StaticAbiLayout<'db>> {
    match outputs.len() {
        0 if matches!(ret.strip_named().kind, TyKind::Unit) => Some(StaticAbiLayout {
            ty: ret.clone(),
            slots: 0,
            kind: StaticAbiLayoutKind::Unit,
        }),
        0 => None,
        1 => static_abi_layout_for_param(ret, &outputs[0]),
        count => {
            let components = product_component_tys(ret.clone(), count)?;
            let layouts = components
                .iter()
                .zip(outputs)
                .map(|(component, output)| static_abi_layout_for_param(component, output))
                .collect::<Option<Vec<_>>>()?;
            Some(static_abi_product_layout(ret.clone(), layouts))
        }
    }
}

fn static_abi_layout_for_param<'db>(
    ty: &Ty<'db>,
    param: &MonoAbiParam,
) -> Option<StaticAbiLayout<'db>> {
    if abi_param_is_dynamic(param) {
        return None;
    }
    if param.ty == "tuple" {
        return static_abi_tuple_layout(ty, &param.components);
    }
    if !param.components.is_empty() {
        return None;
    }
    if abi_param_is_bool(param) {
        if hull_ty_is_bool_word(ty) {
            return Some(StaticAbiLayout {
                ty: ty.clone(),
                slots: 1,
                kind: StaticAbiLayoutKind::Word(AbiWordKind::Bool),
            });
        }
        return None;
    }
    if abi_param_is_address(param) {
        if hull_ty_word_slots(ty) == Some(1) && !hull_ty_is_bool_word(ty) {
            return Some(StaticAbiLayout {
                ty: ty.clone(),
                slots: 1,
                kind: StaticAbiLayoutKind::Word(AbiWordKind::Address),
            });
        }
        return None;
    }
    static_abi_layout_from_ty(ty)
}

fn static_abi_tuple_layout<'db>(
    ty: &Ty<'db>,
    components: &[MonoAbiParam],
) -> Option<StaticAbiLayout<'db>> {
    let component_tys = product_component_tys(ty.clone(), components.len())?;
    let layouts = component_tys
        .iter()
        .zip(components)
        .map(|(component, param)| static_abi_layout_for_param(component, param))
        .collect::<Option<Vec<_>>>()?;
    Some(static_abi_product_layout(ty.clone(), layouts))
}

fn static_abi_layout_from_ty<'db>(ty: &Ty<'db>) -> Option<StaticAbiLayout<'db>> {
    match &ty.strip_named().kind {
        TyKind::Unit => Some(StaticAbiLayout {
            ty: ty.clone(),
            slots: 0,
            kind: StaticAbiLayoutKind::Unit,
        }),
        TyKind::Word => Some(StaticAbiLayout {
            ty: ty.clone(),
            slots: 1,
            kind: StaticAbiLayoutKind::Word(AbiWordKind::Plain),
        }),
        TyKind::Bool => Some(StaticAbiLayout {
            ty: ty.clone(),
            slots: 1,
            kind: StaticAbiLayoutKind::Word(AbiWordKind::Bool),
        }),
        TyKind::Product(_, _) => {
            let mut layouts = Vec::new();
            collect_static_abi_product_layouts(ty, &mut layouts)?;
            Some(static_abi_product_layout(ty.clone(), layouts))
        }
        TyKind::Sum(lhs, rhs) => {
            let lhs = static_abi_layout_from_ty(lhs)?;
            let rhs = static_abi_layout_from_ty(rhs)?;
            let slots = 1 + lhs.slots.max(rhs.slots);
            Some(StaticAbiLayout {
                ty: ty.clone(),
                slots,
                kind: StaticAbiLayoutKind::Sum {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            })
        }
        TyKind::Named { inner, .. } => static_abi_layout_from_ty(inner),
        TyKind::NamedRef { .. } => None,
        TyKind::Function { .. } => None,
    }
}

fn collect_static_abi_product_layouts<'db>(
    ty: &Ty<'db>,
    out: &mut Vec<StaticAbiLayout<'db>>,
) -> Option<()> {
    match &ty.strip_named().kind {
        TyKind::Product(lhs, rhs) => {
            out.push(static_abi_layout_from_ty(lhs)?);
            collect_static_abi_product_layouts(rhs, out)?;
        }
        _ => out.push(static_abi_layout_from_ty(ty)?),
    }
    Some(())
}

fn static_abi_product_layout<'db>(
    ty: Ty<'db>,
    layouts: Vec<StaticAbiLayout<'db>>,
) -> StaticAbiLayout<'db> {
    let slots = layouts.iter().map(|layout| layout.slots).sum();
    StaticAbiLayout {
        ty,
        slots,
        kind: StaticAbiLayoutKind::Product(layouts),
    }
}

fn hull_ty_is_bool_word(ty: &Ty<'_>) -> bool {
    match &ty.strip_named().kind {
        TyKind::Sum(lhs, rhs) => {
            matches!(lhs.strip_named().kind, TyKind::Unit)
                && matches!(rhs.strip_named().kind, TyKind::Unit)
        }
        _ => false,
    }
}

fn abi_param_is_dynamic(param: &MonoAbiParam) -> bool {
    matches!(param.ty.as_str(), "string" | "bytes")
        || param.components.iter().any(abi_param_is_dynamic)
}

fn hull_ty_word_slots(ty: &Ty<'_>) -> Option<usize> {
    match &ty.strip_named().kind {
        TyKind::Word | TyKind::Bool | TyKind::NamedRef { .. } | TyKind::Function { .. } => Some(1),
        TyKind::Unit => Some(0),
        TyKind::Product(lhs, rhs) => Some(hull_ty_word_slots(lhs)? + hull_ty_word_slots(rhs)?),
        TyKind::Sum(lhs, rhs) => Some(1 + hull_ty_word_slots(lhs)?.max(hull_ty_word_slots(rhs)?)),
        TyKind::Named { inner, .. } => hull_ty_word_slots(inner),
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

fn abi_param_is_static_word(param: &specialize::MonoAbiParam) -> bool {
    param.components.is_empty()
        && matches!(
            param.ty.as_str(),
            "uint256" | "uint" | "word" | "bytes32" | "address" | "bool"
        )
}

fn abi_param_is_address(param: &MonoAbiParam) -> bool {
    param.components.is_empty() && param.ty == "address"
}

fn abi_param_is_bool(param: &MonoAbiParam) -> bool {
    param.components.is_empty() && param.ty == "bool"
}

fn abi_word_kind(param: &MonoAbiParam) -> AbiWordKind {
    if abi_param_is_address(param) {
        AbiWordKind::Address
    } else if abi_param_is_bool(param) {
        AbiWordKind::Bool
    } else {
        AbiWordKind::Plain
    }
}

fn selector_hex(selector: [u8; 4]) -> String {
    format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        selector[0], selector[1], selector[2], selector[3]
    )
}

fn abi_words_to_expr<'db>(
    span: Span<'db>,
    layout: &StaticAbiLayout<'db>,
    names: &[String],
) -> Expr<'db> {
    match &layout.kind {
        StaticAbiLayoutKind::Unit => {
            let mut expr = Expr::unit(span);
            expr.ty = layout.ty.clone();
            expr
        }
        StaticAbiLayoutKind::Word(kind) => {
            let word = Expr::var(span, names[0].clone(), Ty::word(span));
            match kind {
                AbiWordKind::Bool => abi_word_to_bool_expr(span, word, layout.ty.clone()),
                AbiWordKind::Plain | AbiWordKind::Address => {
                    let mut expr = word;
                    expr.ty = layout.ty.clone();
                    expr
                }
            }
        }
        StaticAbiLayoutKind::Product(layouts) => {
            let mut offset = 0;
            let mut elems = Vec::new();
            for component in layouts {
                let end = offset + component.slots;
                elems.push(abi_words_to_expr(span, component, &names[offset..end]));
                offset = end;
            }
            product_expr(span, layout.ty.clone(), elems)
        }
        StaticAbiLayoutKind::Sum { lhs, rhs } => {
            let tag = Expr::var(span, names[0].clone(), Ty::word(span));
            let payload = &names[1..];
            let lhs_expr = abi_words_to_expr(span, lhs, &payload[..lhs.slots]);
            let rhs_expr = abi_words_to_expr(span, rhs, &payload[..rhs.slots]);
            Expr {
                span,
                ty: layout.ty.clone(),
                kind: ExprKind::If {
                    target: layout.ty.clone(),
                    cond: Box::new(Expr {
                        span,
                        ty: bool_sum_ty(span),
                        kind: ExprKind::Call {
                            callee: "primEqWord".to_owned(),
                            args: vec![tag, Expr::word(span, "0")],
                        },
                    }),
                    then_expr: Box::new(Expr {
                        span,
                        ty: layout.ty.clone(),
                        kind: ExprKind::Inl {
                            target: layout.ty.clone(),
                            value: Box::new(lhs_expr),
                        },
                    }),
                    else_expr: Box::new(Expr {
                        span,
                        ty: layout.ty.clone(),
                        kind: ExprKind::Inr {
                            target: layout.ty.clone(),
                            value: Box::new(rhs_expr),
                        },
                    }),
                },
            }
        }
    }
}

fn write_expr_to_abi_slots<'db>(
    span: Span<'db>,
    value: Expr<'db>,
    layout: &StaticAbiLayout<'db>,
    names: &[String],
    body: &mut Vec<Stmt<'db>>,
) {
    match &layout.kind {
        StaticAbiLayoutKind::Unit => {}
        StaticAbiLayoutKind::Word(kind) => {
            let rhs = match kind {
                AbiWordKind::Bool if hull_ty_is_bool_word(&value.ty) => {
                    abi_bool_to_word_expr(span, value)
                }
                AbiWordKind::Plain | AbiWordKind::Address | AbiWordKind::Bool => {
                    let mut value = value;
                    value.ty = Ty::word(span);
                    value
                }
            };
            body.push(assign_abi_word_slot(span, &names[0], rhs));
        }
        StaticAbiLayoutKind::Product(layouts) => {
            let fields = layouts
                .iter()
                .map(|layout| layout.ty.clone())
                .collect::<Vec<_>>();
            let components = product_field_exprs(value, &fields);
            let mut offset = 0;
            for (component, layout) in components.into_iter().zip(layouts) {
                let end = offset + layout.slots;
                write_expr_to_abi_slots(span, component, layout, &names[offset..end], body);
                offset = end;
            }
        }
        StaticAbiLayoutKind::Sum { lhs, rhs } => {
            let tag_name = names[0].clone();
            let payload_names = &names[1..];
            let lhs_binder = format!("{tag_name}_inl");
            let rhs_binder = format!("{tag_name}_inr");

            let mut lhs_body = vec![assign_abi_word_slot(span, &tag_name, Expr::word(span, "0"))];
            write_expr_to_abi_slots(
                span,
                Expr::var(span, lhs_binder.clone(), lhs.ty.clone()),
                lhs,
                &payload_names[..lhs.slots],
                &mut lhs_body,
            );

            let mut rhs_body = vec![assign_abi_word_slot(span, &tag_name, Expr::word(span, "1"))];
            write_expr_to_abi_slots(
                span,
                Expr::var(span, rhs_binder.clone(), rhs.ty.clone()),
                rhs,
                &payload_names[..rhs.slots],
                &mut rhs_body,
            );

            body.push(Stmt {
                span,
                kind: StmtKind::Match {
                    target: layout.ty.clone(),
                    scrutinee: value,
                    alts: vec![
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inl),
                            },
                            binder: lhs_binder,
                            body: lhs_body,
                        },
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inr),
                            },
                            binder: rhs_binder,
                            body: rhs_body,
                        },
                    ],
                },
            });
        }
    }
}

fn assign_abi_word_slot<'db>(span: Span<'db>, name: &str, rhs: Expr<'db>) -> Stmt<'db> {
    Stmt {
        span,
        kind: StmtKind::Assign {
            lhs: Expr::var(span, name.to_owned(), Ty::word(span)),
            rhs,
        },
    }
}

fn abi_layout_slot_kinds(layout: &StaticAbiLayout<'_>) -> Vec<AbiWordKind> {
    match &layout.kind {
        StaticAbiLayoutKind::Unit => Vec::new(),
        StaticAbiLayoutKind::Word(kind) => vec![*kind],
        StaticAbiLayoutKind::Product(layouts) => {
            layouts.iter().flat_map(abi_layout_slot_kinds).collect()
        }
        StaticAbiLayoutKind::Sum { lhs, rhs } => {
            let mut kinds = vec![AbiWordKind::Plain];
            kinds.extend((0..lhs.slots.max(rhs.slots)).map(|_| AbiWordKind::Plain));
            kinds
        }
    }
}

fn numbered_name(prefix: &str, index: usize, count: usize) -> String {
    if count == 1 {
        prefix.to_owned()
    } else {
        format!("{prefix}_{index}")
    }
}

fn abi_word_to_bool_expr<'db>(span: Span<'db>, word: Expr<'db>, target: Ty<'db>) -> Expr<'db> {
    Expr {
        span,
        ty: target.clone(),
        kind: ExprKind::If {
            target: target.clone(),
            cond: Box::new(Expr {
                span,
                ty: bool_sum_ty(span),
                kind: ExprKind::Call {
                    callee: "primEqWord".to_owned(),
                    args: vec![word, Expr::word(span, "0")],
                },
            }),
            then_expr: Box::new(bool_expr(span, target.clone(), false)),
            else_expr: Box::new(bool_expr(span, target, true)),
        },
    }
}

fn abi_bool_to_word_expr<'db>(span: Span<'db>, value: Expr<'db>) -> Expr<'db> {
    Expr {
        span,
        ty: Ty::word(span),
        kind: ExprKind::If {
            target: Ty::word(span),
            cond: Box::new(value),
            then_expr: Box::new(Expr::word(span, "1")),
            else_expr: Box::new(Expr::word(span, "0")),
        },
    }
}

fn bool_expr<'db>(span: Span<'db>, target: Ty<'db>, value: bool) -> Expr<'db> {
    let payload = Expr::unit(span);
    let kind = if value {
        ExprKind::Inr {
            target: target.clone(),
            value: Box::new(payload),
        }
    } else {
        ExprKind::Inl {
            target: target.clone(),
            value: Box::new(payload),
        }
    };
    Expr {
        span,
        ty: target,
        kind,
    }
}

fn product_component_tys<'db>(ty: Ty<'db>, count: usize) -> Option<Vec<Ty<'db>>> {
    if count <= 1 {
        return Some(vec![ty]);
    }
    match ty.strip_named().kind.clone() {
        TyKind::Product(lhs, rhs) => {
            let mut out = vec![*lhs];
            out.extend(product_component_tys(*rhs, count - 1)?);
            Some(out)
        }
        _ => None,
    }
}

fn intrinsic_name(intrinsic: MonoIntrinsic) -> &'static str {
    match intrinsic {
        MonoIntrinsic::PrimAddWord => "primAddWord",
        MonoIntrinsic::PrimEqWord => "primEqWord",
        MonoIntrinsic::SubWord => "subWord",
        MonoIntrinsic::GtWord => "gtWord",
        MonoIntrinsic::BxorWord => "bxorWord",
        MonoIntrinsic::BandWord => "bandWord",
        MonoIntrinsic::BorWord => "borWord",
        MonoIntrinsic::WordToInteger => "wordToInteger",
        MonoIntrinsic::WordFromInteger => "wordFromInteger",
        MonoIntrinsic::IntegerAdd => "integerAdd",
        MonoIntrinsic::IntegerSub => "integerSub",
        MonoIntrinsic::IntegerMul => "integerMul",
        MonoIntrinsic::IntegerLt => "integerLt",
        MonoIntrinsic::IntegerEq => "integerEq",
        MonoIntrinsic::ConcatLit => "concatLit",
        MonoIntrinsic::StrlenLit => "strlenLit",
        MonoIntrinsic::KeccakLit => "keccakLit",
    }
}

fn bin_op_name(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("add"),
        BinOp::Sub => Some("sub"),
        BinOp::Mul => Some("mul"),
        BinOp::Div => Some("div"),
        BinOp::Mod => Some("mod"),
        BinOp::BitAnd => Some("and"),
        BinOp::BitXor => Some("xor"),
        BinOp::BitOr => Some("or"),
        BinOp::Eq => Some("primEqWord"),
        BinOp::Lt => Some("lt"),
        BinOp::Gt => Some("gt"),
        BinOp::NotEq | BinOp::LtEq | BinOp::GtEq | BinOp::And | BinOp::Or | BinOp::Error => None,
    }
}

fn mono_expr_name(kind: &MonoExprKind<'_>) -> &'static str {
    match kind {
        MonoExprKind::Field { .. } => "field access",
        MonoExprKind::Index { .. } => "index access",
        MonoExprKind::Proxy(_) => "proxy expression",
        MonoExprKind::Lambda { .. } => "lambda expression",
        MonoExprKind::ClosureDispatch { .. } => "closure dispatch",
        MonoExprKind::Error => "error expression",
        _ => "expression",
    }
}

impl MatrixPat {
    fn is_var_like(&self) -> bool {
        matches!(
            self,
            MatrixPat::Wildcard | MatrixPat::Var { .. } | MatrixPat::Error
        )
    }
}

fn matrix_pat<'db>(pat: &MonoPat<'db>) -> MatrixPat {
    match &pat.kind {
        MonoPatKind::Wildcard => MatrixPat::Wildcard,
        MonoPatKind::Var(id) => MatrixPat::Var {
            name: id.name.clone(),
        },
        MonoPatKind::Lit(lit) => MatrixPat::Lit { lit: lit.clone() },
        MonoPatKind::Con { ctor, args } => MatrixPat::Con {
            ctor: ctor.name.clone(),
            args: args.iter().map(matrix_pat).collect(),
        },
        MonoPatKind::Tuple(elems) => MatrixPat::Tuple {
            elems: elems.iter().map(matrix_pat).collect(),
        },
        MonoPatKind::ComptimeLabel(_) => MatrixPat::ComptimeLabel,
        MonoPatKind::Error => MatrixPat::Error,
    }
}

fn select_match_column<'db>(columns: &[MatchColumn<'db>], rows: &[MatchRow<'db>]) -> usize {
    let mut best_index = 0;
    let mut best_score = 0;
    let mut best_depth = usize::MAX;
    for (index, column) in columns.iter().enumerate() {
        let score = rows
            .iter()
            .filter(|row| row.pats.get(index).is_some_and(|pat| !pat.is_var_like()))
            .count();
        let depth = column.occurrence.0.len();
        if score > best_score || (score == best_score && depth < best_depth) {
            best_index = index;
            best_score = score;
            best_depth = depth;
        }
    }
    best_index
}

fn reorder_columns<'db>(
    mut columns: Vec<MatchColumn<'db>>,
    selected: usize,
) -> Vec<MatchColumn<'db>> {
    if selected < columns.len() {
        let column = columns.remove(selected);
        columns.insert(0, column);
    }
    columns
}

fn reorder_rows<'db>(mut rows: Vec<MatchRow<'db>>, selected: usize) -> Vec<MatchRow<'db>> {
    for row in &mut rows {
        if selected < row.pats.len() {
            let pat = row.pats.remove(selected);
            row.pats.insert(0, pat);
        }
    }
    rows
}

fn split_row<'db>(mut row: MatchRow<'db>) -> (MatrixPat, MatchRow<'db>) {
    let first = if row.pats.is_empty() {
        MatrixPat::Wildcard
    } else {
        row.pats.remove(0)
    };
    (first, row)
}

fn row_with_pats<'db>(mut row: MatchRow<'db>, mut prefix: Vec<MatrixPat>) -> MatchRow<'db> {
    prefix.extend(row.pats);
    row.pats = prefix;
    row
}

fn row_with_wildcards<'db>(row: MatchRow<'db>, count: usize, _span: Span<'db>) -> MatchRow<'db> {
    let wildcards = (0..count).map(|_| MatrixPat::Wildcard).collect::<Vec<_>>();
    row_with_pats(row, wildcards)
}

fn row_with_binding_and_wildcards<'db>(
    mut row: MatchRow<'db>,
    name: String,
    occurrence: Occurrence,
    count: usize,
    span: Span<'db>,
) -> MatchRow<'db> {
    row.bindings.push((name, occurrence));
    row_with_wildcards(row, count, span)
}

fn default_rows<'db>(
    occurrence: Occurrence,
    rows: Vec<MatchRow<'db>>,
    columns: Vec<MatchColumn<'db>>,
) -> (Vec<MatchRow<'db>>, Vec<MatchColumn<'db>>) {
    let rows = rows
        .into_iter()
        .filter_map(|row| {
            let (first, mut row) = split_row(row);
            match first {
                MatrixPat::Var { name, .. } => {
                    row.bindings.push((name, occurrence.clone()));
                    Some(row)
                }
                MatrixPat::Wildcard | MatrixPat::Error => Some(row),
                MatrixPat::Lit { .. }
                | MatrixPat::Con { .. }
                | MatrixPat::Tuple { .. }
                | MatrixPat::ComptimeLabel => None,
            }
        })
        .collect();
    (rows, columns)
}

fn head_constructor_indices<'db>(
    layout: Option<&AdtLayout<'db>>,
    first_col: &[&MatrixPat],
) -> Vec<usize> {
    let Some(layout) = layout else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pat in first_col {
        let MatrixPat::Con { ctor, .. } = pat else {
            continue;
        };
        let Some(index) = layout
            .ctors
            .iter()
            .position(|candidate| constructor_name_matches(ctor, &layout.name, &candidate.name))
        else {
            continue;
        };
        if !out.contains(&index) {
            out.push(index);
        }
    }
    out
}

fn head_literals(first_col: &[&MatrixPat]) -> Vec<LitKind> {
    let mut out = Vec::new();
    for pat in first_col {
        let MatrixPat::Lit { lit, .. } = pat else {
            continue;
        };
        if !matches!(lit, LitKind::Number(_) | LitKind::Hex(_)) {
            continue;
        }
        if !out.contains(lit) {
            out.push(lit.clone());
        }
    }
    out
}

fn hull_lit_pat(lit: &LitKind) -> PatKind {
    match lit {
        LitKind::Number(value) | LitKind::Hex(value) => PatKind::IntLit(value.clone()),
        LitKind::String(_) | LitKind::Error => PatKind::Wildcard,
    }
}

fn child_columns<'db>(
    occurrence: &Occurrence,
    fields: &[SemTy<'db>],
    span: Span<'db>,
) -> Vec<MatchColumn<'db>> {
    fields
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            let mut child = occurrence.0.clone();
            child.push(index);
            MatchColumn {
                occurrence: Occurrence(child),
                ty: *ty,
                span,
            }
        })
        .collect()
}

fn sem_product_fields<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>) -> Vec<SemTy<'db>> {
    match ty.kind(db) {
        SemTyKind::Tuple(elems) => elems.clone(),
        SemTyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Vec::new(),
        SemTyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            let mut out = vec![args[0]];
            out.extend(sem_product_fields(db, args[1]));
            out
        }
        _ => vec![ty],
    }
}

fn product_field_exprs<'db>(base: Expr<'db>, fields: &[Ty<'db>]) -> Vec<Expr<'db>> {
    match fields {
        [] => Vec::new(),
        [field] => {
            let mut expr = base;
            expr.ty = field.clone();
            vec![expr]
        }
        [head, tail @ ..] => {
            let lhs = Expr {
                span: base.span,
                ty: head.clone(),
                kind: ExprKind::Fst(Box::new(base.clone())),
            };
            let rhs = Expr {
                span: base.span,
                ty: product_right_ty(&base.ty),
                kind: ExprKind::Snd(Box::new(base)),
            };
            let mut out = vec![lhs];
            out.extend(product_field_exprs(rhs, tail));
            out
        }
    }
}

fn product_expr<'db>(span: Span<'db>, ty: Ty<'db>, elems: Vec<Expr<'db>>) -> Expr<'db> {
    match elems.as_slice() {
        [] => Expr::unit(span),
        [one] => {
            let mut one = one.clone();
            one.ty = ty;
            one
        }
        [head, tail @ ..] => {
            let tail_ty = product_right_ty(&ty);
            Expr {
                span,
                ty: ty.clone(),
                kind: ExprKind::Pair(
                    Box::new(head.clone()),
                    Box::new(product_expr(span, tail_ty, tail.to_vec())),
                ),
            }
        }
    }
}

fn tuple_ty<'db>(span: Span<'db>, elems: Vec<Ty<'db>>) -> Ty<'db> {
    match elems.as_slice() {
        [] => Ty::unit(span),
        [one] => one.clone(),
        [head, tail @ ..] => Ty::product(span, head.clone(), tuple_ty(span, tail.to_vec())),
    }
}

fn bool_sum_ty<'db>(span: Span<'db>) -> Ty<'db> {
    Ty::sum(span, Ty::unit(span), Ty::unit(span))
}

fn product_right_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Product(_, rhs) => (**rhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

fn sum_right_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Sum(_, rhs) => (**rhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

fn encode_constructor<'db>(
    span: Span<'db>,
    target: Ty<'db>,
    index: usize,
    payload: Expr<'db>,
) -> Expr<'db> {
    let arity = sum_arity(&target);
    if arity <= 1 {
        let mut payload = payload;
        payload.ty = target;
        return payload;
    }
    if index == 0 {
        Expr {
            span,
            ty: target.clone(),
            kind: ExprKind::Inl {
                target,
                value: Box::new(payload),
            },
        }
    } else {
        let right = sum_right_ty(&target);
        let nested = encode_constructor(span, right, index - 1, payload);
        Expr {
            span,
            ty: target.clone(),
            kind: ExprKind::Inr {
                target,
                value: Box::new(nested),
            },
        }
    }
}

fn build_nested_sum_match<'db>(
    span: Span<'db>,
    scrutinee: Expr<'db>,
    target: Ty<'db>,
    branches: Vec<Branch<'db>>,
) -> Stmt<'db> {
    match branches.as_slice() {
        [] => Stmt {
            span,
            kind: StmtKind::Revert("empty branch list".to_owned()),
        },
        [branch] => Stmt {
            span,
            kind: StmtKind::Block(branch.body.clone()),
        },
        [left, rest @ ..] => {
            let right_ty = sum_right_ty(&target);
            let right_binder = rest
                .first()
                .map(|branch| branch.binder.clone())
                .unwrap_or_else(|| "$alt".to_owned());
            let right_expr = Expr::var(span, right_binder.clone(), right_ty.clone());
            let rest_stmt = build_nested_sum_match(span, right_expr, right_ty, rest.to_vec());
            Stmt {
                span,
                kind: StmtKind::Match {
                    target,
                    scrutinee,
                    alts: vec![
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inl),
                            },
                            binder: left.binder.clone(),
                            body: left.body.clone(),
                        },
                        Alt {
                            span,
                            pat: Pat {
                                span,
                                kind: PatKind::Con(Con::Inr),
                            },
                            binder: right_binder,
                            body: vec![rest_stmt],
                        },
                    ],
                },
            }
        }
    }
}

fn sum_arity(ty: &Ty<'_>) -> usize {
    match &ty.strip_named().kind {
        TyKind::Sum(_, rhs) => 1 + sum_arity(rhs),
        _ => 1,
    }
}

fn constructor_name_matches(actual: &str, adt: &str, ctor: &str) -> bool {
    actual == ctor || actual == format!("{adt}_{ctor}") || actual.ends_with(&format!("_{ctor}"))
}

fn source_constructor_comment(name: &str) -> String {
    name.rsplit('_').next().unwrap_or(name).to_owned()
}

fn field_type_is_word_slot<'db>(db: &'db dyn HirDb, ty: hir::ast::ty::TypeRef<'db>) -> bool {
    let TypeRefKind::Named { name, args, .. } = ty.kind(db) else {
        return false;
    };
    args.atom().is_empty()
        && matches!(
            name.atom().text(db),
            "word" | "uint" | "uint256" | "bytes32" | "address"
        )
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

fn find_adt<'db>(db: &'db dyn HirDb, module: Module<'db>, def: DefId<'db>) -> Option<AdtDef<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_adt_in_item(db, *item, def))
}

fn find_adt_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    match item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(*adt),
            _ => None,
        }),
        _ => None,
    }
}

fn subst_sem_ty<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>, args: &[SemTy<'db>]) -> SemTy<'db> {
    match ty.kind(db) {
        SemTyKind::BoundVar(var) => args.get(var.index as usize).copied().unwrap_or(ty),
        SemTyKind::Named { ctor, args: inner } => SemTy::named(
            db,
            *ctor,
            inner
                .iter()
                .map(|arg| subst_sem_ty(db, *arg, args))
                .collect(),
        ),
        SemTyKind::Function { params, ret } => SemTy::function(
            db,
            params
                .iter()
                .map(|param| subst_sem_ty(db, *param, args))
                .collect(),
            subst_sem_ty(db, *ret, args),
        ),
        SemTyKind::Tuple(elems) => SemTy::tuple(
            db,
            elems
                .iter()
                .map(|elem| subst_sem_ty(db, *elem, args))
                .collect(),
        ),
        SemTyKind::Comptime(inner) => SemTy::comptime(db, subst_sem_ty(db, *inner, args)),
        SemTyKind::Error | SemTyKind::Unknown => ty,
    }
}
