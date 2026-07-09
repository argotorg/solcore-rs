use super::*;

pub(super) struct DiagnosticSourceMap<'a, 'db> {
    db: &'db dyn Db,
    view: BodyDesugarView<'a, 'db>,
}

impl<'a, 'db> DiagnosticSourceMap<'a, 'db> {
    pub(super) fn new(db: &'db dyn Db, plans: &'a [BodyPreTypeckDesugarPlan<'db>]) -> Self {
        Self {
            db,
            view: BodyDesugarView::new(plans),
        }
    }

    pub(super) fn label_span(&self, span: Span<'db>) -> LabelSpan {
        LabelSpan::from_span(self.db, span)
    }

    pub(super) fn stmt_label_span(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> LabelSpan {
        self.label_span(body.stmts(self.db).get(stmt).span(self.db))
    }

    pub(super) fn expr_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        let fallback = body.exprs(self.db).get(expr).span(self.db);
        label_span_for_origin(self.db, self.view.expr_origin(body, expr), fallback)
    }

    pub(super) fn pat_label_span(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> LabelSpan {
        let fallback = body.pats(self.db).get(pat).span(self.db);
        label_span_for_origin(self.db, self.view.pat_origin(body, pat), fallback)
    }

    pub(super) fn type_label_span(&self, ty: TypeRef<'db>) -> LabelSpan {
        label_span_for_origin(self.db, self.view.type_origin(ty), ty.span(self.db))
    }
}

pub(super) fn label_span_for_origin<'db>(
    db: &'db dyn Db,
    origin: Option<SourceOrigin<'db>>,
    fallback: Span<'db>,
) -> LabelSpan {
    LabelSpan::from_span(db, origin.map(|origin| origin.span).unwrap_or(fallback))
}
