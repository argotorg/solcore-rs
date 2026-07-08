use super::*;

/// Formats a logical module ID as user-facing text.
///
/// Main modules omit a prefix, standard-library modules use `std`, and external
/// modules use `@name.path` form.
pub fn module_id_display<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> String {
    let path = module.logical_path(db).join(".");
    match module.library(db) {
        LibraryId::Main => path,
        LibraryId::Std if module.logical_path(db).as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}

/// Formats a module path reference as it appeared in import/export syntax.
pub fn module_path_display<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> String {
    let segments = path_segments(db, path).join(".");
    if path.external.is_some() {
        format!("@{segments}")
    } else {
        segments
    }
}

/// Converts a logical module path into the conventional source file path.
///
/// Each logical segment becomes a path component and the file extension is
/// `.solc`.
pub fn module_file_path(logical_path: &[String]) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in logical_path {
        path.push(segment);
    }
    path.set_extension("solc");
    path
}

/// Converts an absolute file path under `root` into a logical module key.
///
/// Returns `None` when `file_path` is outside `root`, contains non-UTF-8 path
/// segments, or maps to an empty logical path.
pub fn module_key_for_path(library: LibraryId, root: &Path, file_path: &Path) -> Option<ModuleKey> {
    let rel = file_path.strip_prefix(root).ok()?;
    let mut logical_path = Vec::new();
    for component in rel.with_extension("").components() {
        let segment = component.as_os_str().to_str()?;
        if !segment.is_empty() {
            logical_path.push(segment.to_owned());
        }
    }
    (!logical_path.is_empty()).then_some(ModuleKey {
        library,
        logical_path,
    })
}

/// Interns a logical module key in the current database.
pub fn module_id_from_key<'db>(db: &'db dyn Db, key: &ModuleKey) -> ModuleId<'db> {
    ModuleId::new(db, key.library.clone(), key.logical_path.clone())
}

pub(super) fn record_source_file_field(db: &dyn Db, file: SourceFile) {
    if tracing::enabled!(Level::DEBUG) {
        tracing::Span::current().record("file", field::display(file_url_tail(db, file)));
    }
}

pub(super) fn record_module_field<'db>(db: &'db dyn Db, module: ModuleId<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        let span = tracing::Span::current();
        span.record("module", field::display(module.display(db)));
        if let Some(file) = db.module_file(module) {
            span.record("file", field::display(file_url_tail(db, file)));
        }
    }
}

pub(super) fn record_body_field<'db>(db: &'db dyn Db, body: FuncBody<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        let def = body.def_id(db);
        let span = tracing::Span::current();
        span.record("file", field::display(file_url_tail(db, def.file(db))));
        span.record("def", field::display(def_name(db, def)));
    }
}

fn def_name<'db>(db: &'db dyn Db, def: DefId<'db>) -> String {
    def.name(db)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{:?}", def.kind(db)))
}

fn file_url_tail(db: &dyn hir::Db, file: SourceFile) -> String {
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

pub(super) fn trace_import_decision<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: &ModulePathRef<'db>,
    target: Option<ModuleId<'db>>,
    status: &'static str,
) {
    if tracing::enabled!(target: "nameres::imports", Level::TRACE) {
        let target = target
            .map(|module| module.display(db))
            .unwrap_or_else(|| "<none>".to_owned());
        tracing::trace!(
            target: "nameres::imports",
            module = %importing.display(db),
            path = %module_path_display(db, path),
            target = %target,
            status,
            "import resolution decision"
        );
    }
}

pub(super) fn selector_kind<'db>(selector: &ImportSelector<'db>) -> &'static str {
    match selector {
        ImportSelector::Wildcard => "wildcard",
        ImportSelector::Names(_) => "names",
    }
}

pub(super) fn ident_text<'db>(db: &'db dyn Db, ident: Ident<'db>) -> String {
    ident.name(db).clone()
}

pub(super) fn spanned_name_text<'db>(
    db: &'db dyn Db,
    name: &SpannedElem<'db, Ident<'db>>,
) -> String {
    ident_text(db, *name.atom())
}

pub(super) fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

pub(super) fn best_name_suggestion(
    name: &str,
    candidates: impl IntoIterator<Item = String>,
) -> Option<String> {
    let mut candidates = candidates
        .into_iter()
        .filter(|candidate| candidate != name)
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();

    let mut best: Option<(usize, String)> = None;
    for candidate in candidates {
        let distance = edit_distance(name, &candidate);
        let limit = suggestion_distance_limit(name, &candidate);
        if distance == 0 || distance > limit {
            continue;
        }
        match &best {
            Some((best_distance, best_candidate))
                if distance > *best_distance
                    || (distance == *best_distance && candidate >= *best_candidate) => {}
            _ => best = Some((distance, candidate)),
        }
    }
    best.map(|(_, candidate)| candidate)
}

fn suggestion_distance_limit(left: &str, right: &str) -> usize {
    let max_len = left.chars().count().max(right.chars().count());
    if max_len <= 4 { 1 } else { 3 }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution = usize::from(left_char != *right_char);
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution);
        }
        previous.clone_from(&current);
    }

    previous[right_chars.len()]
}

pub(super) fn unique_modules<'db>(
    values: impl IntoIterator<Item = ModuleId<'db>>,
) -> Vec<ModuleId<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result
}

pub(super) fn unique_origins<'db>(
    values: impl IntoIterator<Item = Origin<'db>>,
) -> Vec<Origin<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

pub(super) fn sorted_namespaces(values: impl IntoIterator<Item = Namespace>) -> Vec<Namespace> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result.sort_by_key(|namespace| namespace_sort_key(*namespace));
    result
}

fn namespace_name(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::Term => "term",
        Namespace::Type => "type",
        Namespace::Class => "class",
    }
}

pub(super) fn namespace_context(namespaces: &[Namespace]) -> String {
    let names = namespaces
        .iter()
        .map(|namespace| namespace_name(*namespace))
        .collect::<Vec<_>>()
        .join("/");
    if namespaces.len() == 1 {
        format!("in {names} namespace")
    } else {
        format!("across {names} namespaces")
    }
}

pub(super) fn private_surface_key(
    namespace: hir_nameres::Namespace,
    qualifier: &str,
    name: &str,
) -> String {
    let prefix = match namespace {
        hir_nameres::Namespace::Term => "term",
        hir_nameres::Namespace::Type => "type",
        hir_nameres::Namespace::Field => "field",
        hir_nameres::Namespace::Module => "module",
    };
    format!("{prefix}:{qualifier}.{name}")
}
