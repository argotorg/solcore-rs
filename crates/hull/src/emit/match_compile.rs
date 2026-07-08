use super::*;

#[derive(Debug, Clone)]
pub(super) struct AdtLayout<'db> {
    pub(super) name: String,
    pub(super) target: Ty<'db>,
    pub(super) ctors: Vec<CtorLayout<'db>>,
}

#[derive(Debug, Clone)]
pub(super) struct CtorLayout<'db> {
    pub(super) name: String,
    pub(super) payload: Ty<'db>,
    pub(super) fields: Vec<SemTy<'db>>,
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

impl<'db> Emitter<'db> {
    pub(super) fn emit_match(
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
            let span = columns.first().map(|column| column.span).unwrap_or(span);
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
            if default_rows.is_empty() {
                self.push(test.span, EmitDiagnosticKind::NonExhaustiveMatch);
                Some(Box::new(DecisionTree::Fail { span: test.span }))
            } else {
                Some(Box::new(self.compile_match_matrix(
                    span,
                    default_columns,
                    default_rows,
                )))
            }
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
        let default = if default_rows.is_empty() {
            self.push(test.span, EmitDiagnosticKind::NonExhaustiveMatch);
            Some(Box::new(DecisionTree::Fail { span: test.span }))
        } else {
            Some(Box::new(self.compile_match_matrix(
                span,
                default_columns,
                default_rows,
            )))
        };

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
        MonoPatKind::Lit(lit) => MatrixPat::Lit {
            lit: wrap_word_lit_kind(lit),
        },
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
        LitKind::Number(value) | LitKind::Hex(value) => PatKind::IntLit(wrap_lit_text(value)),
        LitKind::String(_) | LitKind::Error => PatKind::Wildcard,
    }
}

fn wrap_word_lit_kind(lit: &LitKind) -> LitKind {
    match lit {
        LitKind::Number(value) => {
            let wrapped = wrap_lit_text(value);
            if wrapped == value.as_str() {
                lit.clone()
            } else {
                LitKind::Number(wrapped)
            }
        }
        LitKind::Hex(value) => {
            let wrapped = wrap_lit_text(value);
            if wrapped == value.as_str() {
                lit.clone()
            } else {
                LitKind::Number(wrapped)
            }
        }
        LitKind::String(_) | LitKind::Error => lit.clone(),
    }
}

pub(super) fn wrap_lit_text(value: &str) -> String {
    wrap_word_literal(value).unwrap_or_else(|_| value.to_owned())
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

pub(super) fn encode_constructor<'db>(
    span: Span<'db>,
    target: Ty<'db>,
    index: usize,
    arity: usize,
    payload: Expr<'db>,
) -> Expr<'db> {
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
        let nested = encode_constructor(span, right, index - 1, arity - 1, payload);
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

pub(super) fn constructor_name_matches(actual: &str, adt: &str, ctor: &str) -> bool {
    actual == ctor || actual == format!("{adt}_{ctor}") || actual.ends_with(&format!("_{ctor}"))
}

fn source_constructor_comment(name: &str) -> String {
    name.rsplit('_').next().unwrap_or(name).to_owned()
}
