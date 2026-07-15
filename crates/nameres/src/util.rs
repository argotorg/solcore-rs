use std::fmt;

use super::*;

/// Borrowed display adapter for logical module IDs.
#[derive(Clone, Copy)]
pub struct ModuleDisplay<'db> {
    db: &'db dyn Db,
    module: ModuleId<'db>,
}

impl<'db> ModuleDisplay<'db> {
    /// Creates a display adapter for `module`.
    pub fn new(db: &'db dyn Db, module: ModuleId<'db>) -> Self {
        Self { db, module }
    }
}

impl fmt::Display for ModuleDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.module.logical_path(self.db);
        match self.module.library(self.db) {
            LibraryId::Main => write_dot_segments(f, path.iter().map(String::as_str)),
            LibraryId::Std if path.as_slice() == ["std"] => f.write_str("std"),
            LibraryId::Std => {
                f.write_str("std.")?;
                write_dot_segments(f, path.iter().map(String::as_str))
            }
            LibraryId::External(name) => {
                write!(f, "@{name}.")?;
                write_dot_segments(f, path.iter().map(String::as_str))
            }
        }
    }
}

impl fmt::Debug for ModuleDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl PartialEq<&str> for ModuleDisplay<'_> {
    fn eq(&self, other: &&str) -> bool {
        let path = self.module.logical_path(self.db);
        match self.module.library(self.db) {
            LibraryId::Main => dot_segments_eq(path.iter().map(String::as_str), other),
            LibraryId::Std if path.as_slice() == ["std"] => *other == "std",
            LibraryId::Std => other
                .strip_prefix("std.")
                .is_some_and(|tail| dot_segments_eq(path.iter().map(String::as_str), tail)),
            LibraryId::External(name) => other
                .strip_prefix('@')
                .and_then(|tail| tail.strip_prefix(name.as_str()))
                .and_then(|tail| tail.strip_prefix('.'))
                .is_some_and(|tail| dot_segments_eq(path.iter().map(String::as_str), tail)),
        }
    }
}

impl PartialEq<String> for ModuleDisplay<'_> {
    fn eq(&self, other: &String) -> bool {
        PartialEq::<&str>::eq(self, &other.as_str())
    }
}

/// Borrowed display adapter for module paths as written in import/export
/// syntax.
#[derive(Clone, Copy)]
pub struct ModulePathDisplay<'a, 'db> {
    db: &'db dyn Db,
    path: &'a ModulePathRef<'db>,
}

impl<'a, 'db> ModulePathDisplay<'a, 'db> {
    /// Creates a display adapter for `path`.
    pub fn new(db: &'db dyn Db, path: &'a ModulePathRef<'db>) -> Self {
        Self { db, path }
    }
}

impl fmt::Display for ModulePathDisplay<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.external.is_some() {
            f.write_str("@")?;
        }
        write_dot_segments(f, module_path_segment_texts(self.db, self.path))
    }
}

impl fmt::Debug for ModulePathDisplay<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl PartialEq<&str> for ModulePathDisplay<'_, '_> {
    fn eq(&self, other: &&str) -> bool {
        if self.path.external.is_some() {
            other.strip_prefix('@').is_some_and(|tail| {
                dot_segments_eq(module_path_segment_texts(self.db, self.path), tail)
            })
        } else {
            dot_segments_eq(module_path_segment_texts(self.db, self.path), other)
        }
    }
}

impl PartialEq<String> for ModulePathDisplay<'_, '_> {
    fn eq(&self, other: &String) -> bool {
        PartialEq::<&str>::eq(self, &other.as_str())
    }
}

fn write_dot_segments<'a>(
    f: &mut fmt::Formatter<'_>,
    segments: impl IntoIterator<Item = &'a str>,
) -> fmt::Result {
    let mut first = true;
    for segment in segments {
        if first {
            first = false;
        } else {
            f.write_str(".")?;
        }
        f.write_str(segment)?;
    }
    Ok(())
}

fn dot_segments_eq<'a>(segments: impl IntoIterator<Item = &'a str>, text: &str) -> bool {
    let mut tail = text;
    let mut first = true;
    for segment in segments {
        if first {
            first = false;
        } else if let Some(next) = tail.strip_prefix('.') {
            tail = next;
        } else {
            return false;
        }
        let Some(next) = tail.strip_prefix(segment) else {
            return false;
        };
        tail = next;
    }
    tail.is_empty()
}

fn module_path_segment_texts<'a, 'db>(
    db: &'db dyn Db,
    path: &'a ModulePathRef<'db>,
) -> impl Iterator<Item = &'db str> + 'a
where
    'db: 'a,
{
    path.segments
        .iter()
        .map(move |segment| (*segment.atom()).text(db))
}

/// Formats a logical module ID as user-facing text.
///
/// Main modules omit a prefix, standard-library modules use `std`, and external
/// modules use `@name.path` form.
pub fn module_id_display<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> String {
    module.display(db).to_string()
}

/// Formats a module path reference as it appeared in import/export syntax.
pub fn module_path_display<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> String {
    ModulePathDisplay::new(db, path).to_string()
}

/// Returns the internal two-segment LSP workspace scope prefix, if present.
///
/// This deliberately recognizes only the namespace shape emitted by the LSP;
/// ordinary source directories with similar names remain normal module paths.
pub(super) fn main_workspace_prefix(logical_path: &[String]) -> &[String] {
    match logical_path {
        [prefix, namespace, ..]
            if matches!(
                prefix.as_str(),
                "__solcore_workspace__" | "__solcore_detached__"
            ) && namespace.len() >= 16
                && namespace.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            &logical_path[..2]
        }
        _ => &logical_path[..0],
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
        match component {
            std::path::Component::Normal(segment) => {
                logical_path.push(segment.to_str()?.to_owned());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
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

/// Finds the logical module identity corresponding to a source file.
///
/// When roots overlap, an identity whose loaded source is exactly `file` wins.
/// Otherwise this returns the first identity derivable from the configured
/// roots, which also supports compiler-owned HIR overlays that retain the
/// original file URL. Test and in-memory drivers may use the canonical virtual
/// URL shape `memory:///main/...`, `memory:///std/...`, or
/// `memory:///external/<name>/...`; those identities are accepted only when
/// the resulting module is loaded as exactly `file`.
pub fn module_id_for_source_file<'db>(db: &'db dyn Db, file: SourceFile) -> Option<ModuleId<'db>> {
    let tree = db.module_tree();
    let mut candidates = Vec::new();
    if let Some(path) = hir::url_to_file_path(file.url(db)) {
        if let Some(key) = module_key_for_path(LibraryId::Main, tree.main_root(db), &path) {
            candidates.push(module_id_from_key(db, &key));
        }
        if let Some(key) = module_key_for_path(LibraryId::Std, tree.std_root(db), &path) {
            candidates.push(module_id_from_key(db, &key));
        }
        for (name, root) in tree.external_roots(db) {
            if let Some(key) = module_key_for_path(LibraryId::External(name.clone()), root, &path) {
                candidates.push(module_id_from_key(db, &key));
            }
        }
    }
    let rooted = candidates
        .iter()
        .copied()
        .find(|candidate| db.module_file(*candidate) == Some(file))
        .or_else(|| candidates.into_iter().next());
    rooted.or_else(|| virtual_module_id_for_source_file(db, file))
}

fn virtual_module_id_for_source_file<'db>(
    db: &'db dyn Db,
    file: SourceFile,
) -> Option<ModuleId<'db>> {
    let url = file.url(db);
    if url.scheme() != "memory" {
        return None;
    }
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (library, mut logical_path) = match segments.as_slice() {
        [root, rest @ ..] if root == "main" => (LibraryId::Main, rest.to_vec()),
        [root, rest @ ..] if root == "std" => (LibraryId::Std, rest.to_vec()),
        [root, name, rest @ ..] if root == "external" => {
            (LibraryId::External(name.clone()), rest.to_vec())
        }
        _ => return None,
    };
    let last = logical_path.last_mut()?;
    *last = last.strip_suffix(".solc")?.to_owned();
    if last.is_empty() {
        return None;
    }
    let candidate = module_id_from_key(
        db,
        &ModuleKey {
            library,
            logical_path,
        },
    );
    (db.module_file(candidate) == Some(file)).then_some(candidate)
}

pub(super) fn record_source_file_field(db: &dyn Db, file: SourceFile) {
    if tracing::enabled!(target: "nameres::query", Level::DEBUG) {
        tracing::Span::current().record("file", field::display(file_url_tail(db, file)));
    }
}

pub(super) fn record_module_field<'db>(db: &'db dyn Db, module: ModuleId<'db>) {
    if tracing::enabled!(target: "nameres::query", Level::DEBUG) {
        let span = tracing::Span::current();
        span.record("module", field::display(module.display(db)));
        if let Some(file) = db.module_file(module) {
            span.record("file", field::display(file_url_tail(db, file)));
        }
    }
}

pub(super) fn record_body_field<'db>(db: &'db dyn Db, body: FuncBody<'db>) {
    if tracing::enabled!(target: "nameres::query", Level::DEBUG) {
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
        match target {
            Some(target) => {
                tracing::trace!(
                    target: "nameres::imports",
                    module = %importing.display(db),
                    path = %ModulePathDisplay::new(db, path),
                    target = %target.display(db),
                    status,
                    "import resolution decision"
                );
            }
            None => {
                tracing::trace!(
                    target: "nameres::imports",
                    module = %importing.display(db),
                    path = %ModulePathDisplay::new(db, path),
                    target = "<none>",
                    status,
                    "import resolution decision"
                );
            }
        }
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
