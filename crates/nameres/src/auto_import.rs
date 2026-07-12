use super::*;

/// Returns exact public-symbol matches that can be imported into `importing`.
///
/// Providers whose export syntax cannot expose `public_name` are rejected
/// before computing their public interface. This exact-name path matters for
/// editor requests: expanding every embedded standard-library interface can
/// require substantially more stack than a JSON-RPC/WASM caller has available.
/// Wildcard exports remain conservative and are always expanded.
pub fn auto_import_candidates<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    public_name: &str,
    namespace: Namespace,
) -> Vec<AutoImportCandidate<'db>> {
    collect_auto_import_candidates(db, importing, Some((public_name, namespace)))
}

/// Builds the importable public-symbol index visible from `importing`.
///
/// Parse-broken providers or origins and public names with an ambiguous
/// namespace-blind selector surface are omitted. Both cases can expose only a
/// provisional or ambiguous import and therefore cannot produce a safe
/// automatic edit.
#[salsa::tracked]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, importing),
    fields(module = field::Empty, file = field::Empty)
)]
pub fn auto_import_index<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
) -> Vec<AutoImportCandidate<'db>> {
    record_module_field(db, importing);
    collect_auto_import_candidates(db, importing, None)
}

fn collect_auto_import_candidates<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    requested: Option<(&str, Namespace)>,
) -> Vec<AutoImportCandidate<'db>> {
    let snapshot = db.module_file_snapshot();
    let selected_targets = selected_import_targets_by_local_name(db, importing);
    let mut candidates = Vec::new();

    for key in snapshot.files(db).keys() {
        let provider = module_id_from_key(db, key);
        if provider == importing || module_has_parse_errors(db, provider) {
            continue;
        }
        let Some(import_path) = source_import_path(db, importing, provider) else {
            continue;
        };
        if requested
            .is_some_and(|(public_name, _)| !module_may_export_name(db, provider, public_name))
        {
            continue;
        }
        let interface = public_interface(db, provider);
        let mut groups = BTreeMap::<String, Vec<&ItemRef<'db>>>::new();
        for item_ref in &interface.item_refs {
            if requested.is_some_and(|(public_name, _)| item_ref.public_name != public_name) {
                continue;
            }
            groups
                .entry(item_ref.public_name.clone())
                .or_default()
                .push(item_ref);
        }

        for (public_name, item_refs) in groups {
            // Adding an unaliased selector exposes `public_name` locally. The
            // import validator treats that local spelling as ambiguous when
            // selected from different target modules, even if the two refs
            // live in different namespaces. Do not offer an edit that would
            // immediately introduce SC0120. A second selector from the same
            // target remains allowed, matching validation's target identity.
            if selected_targets
                .get(&public_name)
                .is_some_and(|targets| targets.iter().any(|target| *target != provider))
            {
                continue;
            }

            // Selective import syntax is namespace-blind. Require one
            // definition identity across every namespace so choosing a term
            // cannot silently bring in an unrelated same-named type (or vice
            // versa).
            let mut surface_origins = Vec::<(ModuleId<'db>, DefId<'db>, &str)>::new();
            for item_ref in &item_refs {
                let key = (
                    item_ref.origin.module,
                    item_ref.origin.def_id,
                    item_ref.source_name.as_str(),
                );
                if !surface_origins.contains(&key) {
                    surface_origins.push(key);
                }
            }
            if surface_origins.len() != 1
                || item_refs
                    .iter()
                    .any(|item_ref| module_has_parse_errors(db, item_ref.origin.module))
            {
                continue;
            }

            let mut by_namespace = BTreeMap::<u8, Vec<&ItemRef<'db>>>::new();
            for item_ref in item_refs {
                by_namespace
                    .entry(namespace_sort_key(item_ref.namespace))
                    .or_default()
                    .push(item_ref);
            }
            for item_refs in by_namespace.into_values() {
                // `public_interface` merges identical refs. Multiple remaining
                // refs in one namespace cannot identify one safe definition.
                let [item_ref] = item_refs.as_slice() else {
                    continue;
                };
                if requested.is_some_and(|(_, namespace)| item_ref.namespace != namespace) {
                    continue;
                }
                candidates.push(AutoImportCandidate {
                    provider,
                    import_path: import_path.clone(),
                    public_name: public_name.clone(),
                    namespace: item_ref.namespace,
                    origin: item_ref.origin.clone(),
                });
            }
        }
    }

    candidates.sort_by(|left, right| {
        (
            left.is_reexport(),
            &left.import_path,
            namespace_sort_key(left.namespace),
            &left.public_name,
        )
            .cmp(&(
                right.is_reexport(),
                &right.import_path,
                namespace_sort_key(right.namespace),
                &right.public_name,
            ))
    });
    candidates.dedup();
    candidates
}

/// Resolves the local bindings introduced by the importing module's existing
/// selectors to the modules named by those import declarations.
///
/// `select_import_refs` applies aliases, wildcard selection, and hiding, so
/// the resulting key is the actual local spelling that a generated unaliased
/// selector could collide with. Keeping targets rather than item origins is
/// intentional: this mirrors `validate_ambiguous_selected_imports`, including
/// its namespace-blind cross-target check for re-export providers.
fn selected_import_targets_by_local_name<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
) -> FxHashMap<String, FxHashSet<ModuleId<'db>>> {
    let Some(file) = db.module_file(importing) else {
        return FxHashMap::default();
    };
    let mut selected_targets = FxHashMap::<String, FxHashSet<ModuleId<'db>>>::default();
    let mut scratch = Vec::new();
    for import in module_imports(db, file).imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, import);
        let Some(target) = resolve_for_export(
            db,
            importing,
            &path,
            ExportResolutionMode::Lenient,
            &mut scratch,
        ) else {
            continue;
        };
        let interface = public_interface(db, target);
        for item_ref in select_import_refs(db, &interface.item_refs, selector, import.hiding(db)) {
            selected_targets
                .entry(item_ref.public_name)
                .or_default()
                .insert(target);
        }
    }
    selected_targets
}

/// Conservatively decides whether a module's export declarations can expose
/// `public_name` without expanding any imported/re-exported interfaces.
fn module_may_export_name<'db>(db: &'db dyn Db, module: ModuleId<'db>, public_name: &str) -> bool {
    let Some(file) = db.module_file(module) else {
        return false;
    };
    module_imports(db, file).exports.iter().any(|export| {
        let names = match export.kind(db) {
            ExportKind::List(names) | ExportKind::ItemsFrom(_, names) => names,
            // These forms expose only a module qualifier, never an item ref.
            ExportKind::Module(_) | ExportKind::ModuleAs(_, _) => return false,
        };
        names.iter().any(|exported| {
            let name = spanned_name_text(db, &exported.name);
            name == public_name || name == "*" || name.ends_with(".*")
        })
    })
}

/// Produces the canonical source-level module path for importing `provider`
/// from `importing`.
///
/// Main-library paths are absolute `lib.*` paths and are available only within
/// the same workspace/detached-root namespace. Internal namespace segments are
/// stripped from the returned text. Standard and configured external libraries
/// use their global `std.*` and `@name.*` spellings. `None` is returned when a
/// logical path cannot be represented by ordinary source identifiers.
pub fn source_import_path<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    provider: ModuleId<'db>,
) -> Option<String> {
    if importing == provider {
        return None;
    }
    match provider.library(db) {
        LibraryId::Main => {
            if importing.library(db) != &LibraryId::Main {
                return None;
            }
            let importing_prefix = main_workspace_prefix(importing.logical_path(db));
            let provider_prefix = main_workspace_prefix(provider.logical_path(db));
            if importing_prefix != provider_prefix {
                return None;
            }
            let relative = &provider.logical_path(db)[provider_prefix.len()..];
            valid_module_segments(relative).then(|| format!("lib.{}", relative.join(".")))
        }
        LibraryId::Std => {
            let path = provider.logical_path(db);
            if path.as_slice() == ["std"] {
                Some("std".to_owned())
            } else if !valid_module_segments(path) {
                None
            } else {
                Some(format!("std.{}", path.join(".")))
            }
        }
        LibraryId::External(name) => {
            if !parser::is_valid_identifier(name)
                || !db.module_tree().external_roots(db).contains_key(name)
            {
                return None;
            }
            let path = provider.logical_path(db);
            if path.first() == Some(name) && path.len() == 1 {
                Some(format!("@{name}"))
            } else if !valid_module_segments(path) {
                None
            } else {
                Some(format!("@{name}.{}", path.join(".")))
            }
        }
    }
}

fn valid_module_segments(segments: &[String]) -> bool {
    !segments.is_empty()
        && segments
            .iter()
            .all(|segment| parser::is_valid_identifier(segment))
}
