use super::*;

pub(super) fn record_module_fields<'db>(db: &'db dyn Db, module: Module<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        record_def_fields(db, module.def_id_value(db));
    }
}

pub(super) fn record_body_fields<'db>(db: &'db dyn Db, body: FuncBody<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        record_def_fields(db, body.def_id(db));
    }
}

fn record_def_fields<'db>(db: &'db dyn Db, def: DefId<'db>) {
    let span = tracing::Span::current();
    span.record("file", field::display(file_url_tail(db, def.file(db))));
    span.record("def", field::display(def_name(db, def)));
}

fn def_name<'db>(db: &'db dyn Db, def: DefId<'db>) -> String {
    def.name(db)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{:?}", def.kind(db)))
}

fn file_url_tail(db: &dyn Db, file: crate::input::SourceFile) -> String {
    let url = file.url(db);
    if let Some(mut segments) = url.path_segments()
        && let Some(last) = segments.next_back()
        && !last.is_empty()
    {
        return last.to_owned();
    }
    url.as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .unwrap_or(url.as_str())
        .to_owned()
}

pub fn ident_text<'db>(db: &'db dyn Db, ident: &SpannedElem<'db, Ident<'db>>) -> String {
    ident_text_str(db, ident).to_owned()
}

pub(super) fn ident_text_str<'db>(
    db: &'db dyn Db,
    ident: &SpannedElem<'db, Ident<'db>>,
) -> &'db str {
    (*ident.atom()).text(db)
}

pub(super) fn collect_constructor_type_candidates<'db>(
    db: &'db dyn Db,
    lists: &[CtorList<'db>],
    leaf: &str,
    out: &mut Vec<ConstructorTypeCandidate>,
) {
    for list in lists {
        for ctor in &list.ctors {
            if ctor.name == leaf {
                out.push(ConstructorTypeCandidate {
                    ty_name: list.ty_name.clone(),
                    ctor_name: ctor.name.clone(),
                    span: LabelSpan::from_span(db, ctor.span),
                });
            }
        }
    }
}

pub(super) fn unique_constructor_type_candidate(
    candidates: impl IntoIterator<Item = ConstructorTypeCandidate>,
) -> Option<ConstructorTypeCandidate> {
    let mut candidates = candidates.into_iter();
    let first = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some(first)
}

pub(super) fn qualify(qualifier: &str, name: &str) -> String {
    format!("{qualifier}.{name}")
}

pub(super) fn path_span<'db>(db: &'db dyn Db, path: &[SpannedElem<'db, Ident<'db>>]) -> Span<'db> {
    let first = path.first().expect("non-empty path");
    let last = path.last().expect("non-empty path");
    first.span(db) + last.span(db)
}

pub(super) fn expr_path<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> Option<Vec<String>> {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Ident(name) => Some(vec![ident_text_str(db, name).to_owned()]),
        ExprKind::Field { base, field } => {
            let mut path = expr_path(db, body, *base)?;
            path.push(ident_text_str(db, field).to_owned());
            Some(path)
        }
        _ => None,
    }
}

pub(super) fn param_name<'a, 'db>(
    param: &'a FuncParam<'db>,
) -> Option<&'a SpannedElem<'db, Ident<'db>>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name),
        FuncParam::Error { .. } => None,
    }
}

pub fn param_bindings<'db>(params: &[FuncParam<'db>]) -> Vec<ParamBinding<'db>> {
    params
        .iter()
        .filter_map(param_name)
        .map(|name| ParamBinding { name: *name })
        .collect()
}

pub fn type_var_bindings<'db>(
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
}

pub fn is_direct_call_resolution(resolution: &Resolution<'_>) -> bool {
    matches!(
        resolution,
        Resolution::Def {
            kind: DefResolutionKind::Function,
            ..
        } | Resolution::Ctor { .. }
            | Resolution::ClassMethod { .. }
            | Resolution::Builtin(
                BuiltinKind::Constructor(_)
                    | BuiltinKind::Function(_)
                    | BuiltinKind::ClassMethod(_)
            )
    )
}
