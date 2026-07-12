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

/// Returns type imports that also expose `constructor_name` through
/// `type_name`.
///
/// The initial type lookup deliberately reuses [`auto_import_candidates`], so
/// constructor fixes inherit its exact-name prefilter, selector-collision
/// checks, parse safety, canonical paths, and deterministic ranking. The
/// second pass only retains the corresponding public item reference when that
/// exact definition identity carries the requested visible constructor.
pub fn auto_import_constructor_candidates<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    type_name: &str,
    constructor_name: &str,
) -> Vec<AutoImportCandidate<'db>> {
    if !parser::is_valid_identifier(type_name) || !parser::is_valid_identifier(constructor_name) {
        return Vec::new();
    }

    auto_import_candidates(db, importing, type_name, Namespace::Type)
        .into_iter()
        .filter(|candidate| {
            public_interface(db, candidate.provider)
                .item_refs
                .iter()
                .any(|item_ref| {
                    item_ref.namespace == Namespace::Type
                        && item_ref.public_name == candidate.public_name
                        && item_ref.origin == candidate.origin
                        && matches!(
                            &item_ref.constructors,
                            ConstructorVisibility::Visible(constructors)
                                if constructors.contains(constructor_name)
                        )
                })
        })
        .collect()
}

/// Returns modules whose default import qualifier is `qualifier` and whose
/// public interface exposes the term `member` immediately below it.
///
/// This intentionally handles only one ordinary identifier as the qualifier.
/// Alias synthesis and nested missing-prefix repair require choosing new local
/// spellings and are left to a higher-level refactoring. Every module binding
/// introduced by the canonical path is checked, including full-path prefixes:
/// `import lib.one.math;` exposes `math`, `one`, and `one.math`. A candidate is
/// suppressed if any of those names would conflict with a local/imported item
/// or another plain module import.
pub fn auto_import_module_candidates<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    qualifier: &str,
    member: &str,
) -> Vec<AutoImportModuleCandidate<'db>> {
    if !parser::is_valid_identifier(qualifier) || !parser::is_valid_identifier(member) {
        return Vec::new();
    }

    let importing_surface = module_import_surface(db, importing);
    let syntactic_module_bindings = existing_plain_module_bindings(db, importing);
    let snapshot = db.module_file_snapshot();
    let mut candidates = Vec::<(bool, AutoImportModuleCandidate<'db>)>::new();
    for key in snapshot.files(db).keys() {
        let provider = module_id_from_key(db, key);
        if provider == importing || module_has_parse_errors(db, provider) {
            continue;
        }
        let Some(import_path) = source_import_path(db, importing, provider) else {
            continue;
        };
        let Some(generated_bindings) = generated_module_bindings(&import_path) else {
            continue;
        };
        if default_import_qualifier(&import_path) != Some(qualifier)
            || module_bindings_conflict(
                &importing_surface,
                &syntactic_module_bindings,
                &generated_bindings,
            )
            || !module_may_export_name(db, provider, member)
        {
            continue;
        }

        let interface = public_interface(db, provider);
        let item_refs = interface
            .item_refs
            .iter()
            .filter(|item_ref| {
                item_ref.public_name == member && item_ref.namespace == Namespace::Term
            })
            .collect::<Vec<_>>();
        if item_refs.is_empty() {
            continue;
        }
        if item_refs
            .iter()
            .any(|item_ref| module_has_parse_errors(db, item_ref.origin.module))
            || has_ambiguous_member_surface(&item_refs)
        {
            continue;
        }

        let is_reexport = !item_refs
            .iter()
            .any(|item_ref| item_ref.origin.module == provider);
        candidates.push((
            is_reexport,
            AutoImportModuleCandidate {
                provider,
                import_path,
                qualifier: qualifier.to_owned(),
                member: member.to_owned(),
            },
        ));
    }

    candidates.sort_by(|(left_reexport, left), (right_reexport, right)| {
        (
            left_reexport,
            &left.import_path,
            &left.qualifier,
            &left.member,
        )
            .cmp(&(
                right_reexport,
                &right.import_path,
                &right.qualifier,
                &right.member,
            ))
    });
    candidates.dedup_by(|(_, left), (_, right)| left == right);
    candidates
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect()
}

fn default_import_qualifier(import_path: &str) -> Option<&str> {
    let leaf = import_path.rsplit('.').next()?;
    Some(leaf.strip_prefix('@').unwrap_or(leaf))
}

/// Computes the names introduced by the plain import that the LSP will emit.
///
/// This mirrors `import_module_qualifiers` followed by `module_prefixes` for
/// canonical paths without allocating a synthetic HIR import. `lib` and a
/// multi-segment external-library root are source routing markers, not visible
/// qualifier segments.
fn generated_module_bindings(import_path: &str) -> Option<Vec<String>> {
    let external = import_path.starts_with('@');
    let path = import_path.strip_prefix('@').unwrap_or(import_path);
    let segments = path.split('.').collect::<Vec<_>>();
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| !parser::is_valid_identifier(segment))
    {
        return None;
    }
    let visible = if (external || segments.first() == Some(&"lib")) && segments.len() > 1 {
        &segments[1..]
    } else {
        &segments[..]
    };
    let leaf = visible.last()?.to_string();
    let full = visible.join(".");
    Some(unique_strings(
        unique_strings([leaf, full])
            .into_iter()
            .flat_map(|qualifier| module_prefixes(&qualifier)),
    ))
}

/// Collects all bindings claimed by existing plain-import syntax, whether or
/// not the target currently resolves. Selective imports intentionally do not
/// participate: adding a separate plain import of the same provider is valid.
fn existing_plain_module_bindings<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
) -> FxHashSet<String> {
    let Some(file) = db.module_file(importing) else {
        return FxHashSet::default();
    };
    let mut bindings = FxHashSet::default();
    for import in module_imports(db, file).imports {
        if import.selector(db).is_some() {
            continue;
        }
        let path = path_ref_from_import(db, import);
        for qualifier in import_module_qualifiers(db, import, &path) {
            bindings.extend(module_prefixes(&qualifier));
        }
    }
    bindings
}

fn module_bindings_conflict(
    surface: &ModuleImportSurface<'_>,
    syntactic_module_bindings: &FxHashSet<String>,
    generated_bindings: &[String],
) -> bool {
    surface.unknown_unqualified_wildcard
        || generated_bindings.iter().any(|name| {
            syntactic_module_bindings.contains(name)
                || surface.modules.contains_key(name)
                || surface.incomplete_modules.contains(name)
                || surface.terms.contains_key(name)
                || surface.types.contains_key(name)
                || surface.unknown_unqualified_names.contains(name)
                || surface.item_scope.as_ref().is_some_and(|scope| {
                    scope.terms.get(name).is_some()
                        || scope.types.get(name).is_some()
                        || scope.contracts.iter().any(|contract| {
                            contract.terms.get(name).is_some()
                                || contract.types.get(name).is_some()
                                || contract
                                    .fields
                                    .iter()
                                    .any(|field| field.name == name.as_str())
                        })
                })
        })
}

fn has_ambiguous_member_surface(item_refs: &[&ItemRef<'_>]) -> bool {
    let mut origins = FxHashMap::<Namespace, FxHashSet<(ModuleId<'_>, DefId<'_>)>>::default();
    for item_ref in item_refs {
        origins
            .entry(item_ref.namespace)
            .or_default()
            .insert((item_ref.origin.module, item_ref.origin.def_id));
    }
    origins.values().any(|origins| origins.len() > 1)
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

#[cfg(test)]
mod tests {
    use super::generated_module_bindings;

    #[test]
    fn canonical_paths_expose_the_same_module_prefixes_as_plain_imports() {
        assert_eq!(
            generated_module_bindings("lib.one.math").as_deref(),
            Some(&["math".to_owned(), "one".to_owned(), "one.math".to_owned()][..])
        );
        assert_eq!(
            generated_module_bindings("std.collections.list").as_deref(),
            Some(
                &[
                    "list".to_owned(),
                    "std".to_owned(),
                    "std.collections".to_owned(),
                    "std.collections.list".to_owned(),
                ][..]
            )
        );
        assert_eq!(
            generated_module_bindings("@pkg.math.api").as_deref(),
            Some(&["api".to_owned(), "math".to_owned(), "math.api".to_owned()][..])
        );
        assert_eq!(
            generated_module_bindings("@pkg").as_deref(),
            Some(&["pkg".to_owned()][..])
        );
    }
}
