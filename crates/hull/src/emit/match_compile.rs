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

struct MatchMatrix<'db> {
    columns: Vec<MatchColumn<'db>>,
    rows: Vec<MatchRow<'db>>,
}

struct MatrixState<'db> {
    test: MatchColumn<'db>,
    rest: Vec<MatchColumn<'db>>,
    rows: Vec<MatchRow<'db>>,
}

impl<'db> MatchMatrix<'db> {
    fn new(columns: Vec<MatchColumn<'db>>, rows: Vec<MatchRow<'db>>) -> Self {
        Self { columns, rows }
    }

    fn rows_is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn fail_span(&self, fallback: Span<'db>) -> Span<'db> {
        self.columns
            .first()
            .map(|column| column.span)
            .unwrap_or(fallback)
    }

    fn columns_is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    fn first_row_is_var_like(&self) -> bool {
        self.rows
            .first()
            .is_some_and(|row| row.pats.iter().all(MatrixPat::is_var_like))
    }

    fn into_first_leaf(self) -> DecisionTree<'db> {
        let row = self.rows.into_iter().next().expect("row exists");
        DecisionTree::Leaf {
            bindings: row.bindings,
            body: row.body,
        }
    }

    fn into_var_like_leaf(self) -> DecisionTree<'db> {
        let row = self.rows.into_iter().next().expect("row exists");
        let MatchRow {
            pats,
            mut bindings,
            body,
        } = row;
        for (pat, column) in pats.into_iter().zip(self.columns) {
            if let MatrixPat::Var { name } = pat {
                bindings.push((name, column.occurrence));
            }
        }
        DecisionTree::Leaf { bindings, body }
    }

    fn into_selected_state(mut self) -> MatrixState<'db> {
        debug_assert!(!self.columns.is_empty());
        let selected = select_match_column(&self.columns, &self.rows);
        move_selected_column_to_front(&mut self.columns, selected);
        move_selected_pat_to_front(&mut self.rows, selected);
        let test = self.columns.remove(0);
        MatrixState {
            test,
            rest: self.columns,
            rows: self.rows,
        }
    }
}

impl<'db> MatrixState<'db> {
    fn first_col(&self) -> Vec<&MatrixPat> {
        self.rows
            .iter()
            .filter_map(|row| row.pats.first())
            .collect()
    }

    fn into_default(self) -> (Vec<MatchRow<'db>>, Vec<MatchColumn<'db>>) {
        default_rows(self.test.occurrence, self.rows, self.rest)
    }
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

        let mut occurrences = columns
            .iter()
            .zip(scrutinee_exprs)
            .map(|(column, expr)| (column.occurrence.clone(), expr))
            .collect::<BTreeMap<_, _>>();
        let tree = self.compile_match_matrix(span, MatchMatrix::new(columns, rows));
        self.tree_to_body(span, &mut occurrences, &tree)
    }

    fn compile_match_matrix(
        &mut self,
        span: Span<'db>,
        matrix: MatchMatrix<'db>,
    ) -> DecisionTree<'db> {
        if matrix.rows_is_empty() {
            let span = matrix.fail_span(span);
            self.push(span, EmitDiagnosticKind::NonExhaustiveMatch);
            return DecisionTree::Fail { span };
        }
        if matrix.columns_is_empty() {
            return matrix.into_first_leaf();
        }
        if matrix.first_row_is_var_like() {
            return matrix.into_var_like_leaf();
        }

        let state = matrix.into_selected_state();
        let first_col = state.first_col();

        if let Some(fields) = self.product_column_fields(&state.test, &first_col) {
            drop(first_col);
            return self.compile_product_column(span, state, fields);
        }

        let head_ctors = head_constructor_indices(
            self.adt_layout_for_sem_ty(state.test.ty, state.test.span)
                .as_ref(),
            &first_col,
        );
        if !head_ctors.is_empty() {
            drop(first_col);
            return self.compile_constructor_switch(span, state, head_ctors);
        }

        let head_lits = head_literals(&first_col);
        if !head_lits.is_empty() {
            drop(first_col);
            return self.compile_atomic_switch(span, state, head_lits);
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

        drop(first_col);
        let (rows, columns) = state.into_default();
        self.compile_match_matrix(span, MatchMatrix::new(columns, rows))
    }

    fn product_column_fields(
        &mut self,
        test: &MatchColumn<'db>,
        first_col: &[&MatrixPat],
    ) -> Option<Vec<SemTy<'db>>> {
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
                let ctor = layout.ctors.into_iter().next()?;
                ctor.fields
            }
            _ => return None,
        };
        Some(fields)
    }

    fn compile_product_column(
        &mut self,
        span: Span<'db>,
        state: MatrixState<'db>,
        fields: Vec<SemTy<'db>>,
    ) -> DecisionTree<'db> {
        let MatrixState { test, rest, rows } = state;

        let child_columns = child_columns(&test.occurrence, &fields, test.span);
        let mut next_columns = child_columns;
        next_columns.extend(rest);
        let mut next_rows = Vec::new();
        for row in rows {
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
        DecisionTree::Product {
            occurrence: test.occurrence,
            fields: field_tys,
            subtree: Box::new(
                self.compile_match_matrix(span, MatchMatrix::new(next_columns, next_rows)),
            ),
        }
    }

    fn compile_constructor_switch(
        &mut self,
        span: Span<'db>,
        state: MatrixState<'db>,
        head_ctors: Vec<usize>,
    ) -> DecisionTree<'db> {
        let MatrixState { test, rest, rows } = state;
        let Some(layout) = self.adt_layout_for_sem_ty(test.ty, test.span) else {
            self.push(
                test.span,
                EmitDiagnosticKind::MissingAdtLayout {
                    adt: test.ty.display(self.db),
                },
            );
            return DecisionTree::Fail { span };
        };
        let include_default = head_ctors.len() != layout.ctors.len();
        let (projected_branches, default_rows) =
            project_constructor_rows(&test, &layout, &head_ctors, rows, include_default);

        let mut branches = Vec::new();
        for (index, next_rows) in head_ctors.iter().copied().zip(projected_branches) {
            let ctor = &layout.ctors[index];
            let child_cols = child_columns(&test.occurrence, &ctor.fields, test.span);
            let mut next_columns = child_cols;
            next_columns.extend_from_slice(&rest);
            branches.push(CtorDecision {
                index,
                tree: self.compile_match_matrix(span, MatchMatrix::new(next_columns, next_rows)),
            });
        }

        let default = if !include_default {
            None
        } else {
            if default_rows.is_empty() {
                self.push(test.span, EmitDiagnosticKind::NonExhaustiveMatch);
                Some(Box::new(DecisionTree::Fail { span: test.span }))
            } else {
                Some(Box::new(self.compile_match_matrix(
                    span,
                    MatchMatrix::new(rest, default_rows),
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
        state: MatrixState<'db>,
        head_lits: Vec<LitKind>,
    ) -> DecisionTree<'db> {
        let MatrixState { test, rest, rows } = state;
        let (projected_branches, default_rows) = project_atomic_rows(&test, &head_lits, rows);

        let mut branches = Vec::new();
        for (lit, next_rows) in head_lits.into_iter().zip(projected_branches) {
            branches.push(AtomicDecision {
                lit,
                tree: self.compile_match_matrix(span, MatchMatrix::new(rest.clone(), next_rows)),
            });
        }

        let default = if default_rows.is_empty() {
            self.push(test.span, EmitDiagnosticKind::NonExhaustiveMatch);
            Some(Box::new(DecisionTree::Fail { span: test.span }))
        } else {
            Some(Box::new(self.compile_match_matrix(
                span,
                MatchMatrix::new(rest, default_rows),
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

fn move_selected_column_to_front<'db>(columns: &mut Vec<MatchColumn<'db>>, selected: usize) {
    if selected < columns.len() {
        let column = columns.remove(selected);
        columns.insert(0, column);
    }
}

fn move_selected_pat_to_front<'db>(rows: &mut [MatchRow<'db>], selected: usize) {
    for row in rows {
        if selected < row.pats.len() {
            let pat = row.pats.remove(selected);
            row.pats.insert(0, pat);
        }
    }
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

fn project_constructor_rows<'db>(
    test: &MatchColumn<'db>,
    layout: &AdtLayout<'db>,
    head_ctors: &[usize],
    rows: Vec<MatchRow<'db>>,
    include_default: bool,
) -> (Vec<Vec<MatchRow<'db>>>, Vec<MatchRow<'db>>) {
    let mut branch_rows = (0..head_ctors.len())
        .map(|_| Vec::new())
        .collect::<Vec<_>>();
    let mut default_rows = Vec::new();

    for row in rows {
        let (first, row_rest) = split_row(row);
        match first {
            MatrixPat::Con {
                ctor: name, args, ..
            } => {
                let matching_branches = head_ctors
                    .iter()
                    .enumerate()
                    .filter_map(|(branch, index)| {
                        constructor_name_matches(&name, &layout.name, &layout.ctors[*index].name)
                            .then_some(branch)
                    })
                    .collect::<Vec<_>>();
                let Some((&last_branch, prefix_branches)) = matching_branches.split_last() else {
                    continue;
                };
                for branch in prefix_branches {
                    branch_rows[*branch].push(row_with_pats(row_rest.clone(), args.clone()));
                }
                branch_rows[last_branch].push(row_with_pats(row_rest, args));
            }
            MatrixPat::Var { name, .. } => {
                push_constructor_var_rows(
                    test,
                    layout,
                    head_ctors,
                    &mut branch_rows,
                    include_default.then_some(&mut default_rows),
                    row_rest,
                    name,
                );
            }
            MatrixPat::Wildcard | MatrixPat::Error => {
                push_constructor_wildcard_rows(
                    test,
                    layout,
                    head_ctors,
                    &mut branch_rows,
                    include_default.then_some(&mut default_rows),
                    row_rest,
                );
            }
            MatrixPat::Tuple { .. } | MatrixPat::Lit { .. } | MatrixPat::ComptimeLabel => {}
        }
    }

    (branch_rows, default_rows)
}

fn push_constructor_var_rows<'db>(
    test: &MatchColumn<'db>,
    layout: &AdtLayout<'db>,
    head_ctors: &[usize],
    branch_rows: &mut [Vec<MatchRow<'db>>],
    default_rows: Option<&mut Vec<MatchRow<'db>>>,
    row_rest: MatchRow<'db>,
    name: String,
) {
    for (branch, index) in head_ctors.iter().copied().enumerate() {
        let count = layout.ctors[index].fields.len();
        branch_rows[branch].push(row_with_binding_and_wildcards(
            row_rest.clone(),
            name.clone(),
            test.occurrence.clone(),
            count,
            test.span,
        ));
    }
    if let Some(default_rows) = default_rows {
        let mut row = row_rest;
        row.bindings.push((name, test.occurrence.clone()));
        default_rows.push(row);
    }
}

fn push_constructor_wildcard_rows<'db>(
    test: &MatchColumn<'db>,
    layout: &AdtLayout<'db>,
    head_ctors: &[usize],
    branch_rows: &mut [Vec<MatchRow<'db>>],
    default_rows: Option<&mut Vec<MatchRow<'db>>>,
    row_rest: MatchRow<'db>,
) {
    for (branch, index) in head_ctors.iter().copied().enumerate() {
        let count = layout.ctors[index].fields.len();
        branch_rows[branch].push(row_with_wildcards(row_rest.clone(), count, test.span));
    }
    if let Some(default_rows) = default_rows {
        default_rows.push(row_rest);
    }
}

fn project_atomic_rows<'db>(
    test: &MatchColumn<'db>,
    head_lits: &[LitKind],
    rows: Vec<MatchRow<'db>>,
) -> (Vec<Vec<MatchRow<'db>>>, Vec<MatchRow<'db>>) {
    let mut branch_rows = (0..head_lits.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut default_rows = Vec::new();

    for row in rows {
        let (first, row_rest) = split_row(row);
        match first {
            MatrixPat::Lit { lit: candidate, .. } => {
                if let Some(branch) = head_lits.iter().position(|lit| lit == &candidate) {
                    branch_rows[branch].push(row_rest);
                }
            }
            MatrixPat::Var { name, .. } => {
                let mut row_rest = row_rest;
                row_rest.bindings.push((name, test.occurrence.clone()));
                push_projected_row(row_rest, &mut branch_rows, Some(&mut default_rows));
            }
            MatrixPat::Wildcard | MatrixPat::Error => {
                push_projected_row(row_rest, &mut branch_rows, Some(&mut default_rows));
            }
            MatrixPat::Con { .. } | MatrixPat::Tuple { .. } | MatrixPat::ComptimeLabel => {}
        }
    }

    (branch_rows, default_rows)
}

fn push_projected_row<'db>(
    row: MatchRow<'db>,
    branch_rows: &mut [Vec<MatchRow<'db>>],
    default_rows: Option<&mut Vec<MatchRow<'db>>>,
) {
    let Some((last_branch, prefix_branches)) = branch_rows.split_last_mut() else {
        if let Some(default_rows) = default_rows {
            default_rows.push(row);
        }
        return;
    };
    for branch in prefix_branches {
        branch.push(row.clone());
    }
    if let Some(default_rows) = default_rows {
        last_branch.push(row.clone());
        default_rows.push(row);
    } else {
        last_branch.push(row);
    }
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
    build_nested_sum_match_from_slice(span, scrutinee, target, &branches)
}

fn build_nested_sum_match_from_slice<'db>(
    span: Span<'db>,
    scrutinee: Expr<'db>,
    target: Ty<'db>,
    branches: &[Branch<'db>],
) -> Stmt<'db> {
    match branches {
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
            let rest_stmt = build_nested_sum_match_from_slice(span, right_expr, right_ty, rest);
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
