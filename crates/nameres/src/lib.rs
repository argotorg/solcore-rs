//! Inter-module name resolution and public interface construction.
//!
//! This crate sits above parsing and HIR name resolution. It maps logical
//! module paths to source files, gathers imports/exports, builds a reachable
//! module graph, computes each module's public interface, and finally resolves
//! local HIR bodies with imported names available.
//!
//! [`ModuleId`] is logical, not textual or filesystem identity. It is interned
//! from a [`ModuleKey`] containing the library (`main`, `std`, or an external
//! root) plus the module path inside that library. The same source text reached
//! through a different library root is a different module by design.
//!
//! Public interfaces are Salsa tracked with a fixed point:
//! `public_interface_initial` seeds cyclic queries with an empty interface, and
//! `public_interface_cycle` keeps the newer result only when it changes.
//! Starting empty is conservative: during an import/export cycle, no name is
//! assumed visible until a real expansion proves it. Repeated evaluation grows
//! or stabilizes the interface until the cycle converges.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use hir::{
    anchor::{DefId, DefKind},
    ast::{
        Ident,
        function::{FuncBody, FuncParam},
        item::{
            AdtDef, ClassDef, ConstructorSelector, ContractDef, ContractItem, Export, ExportKind,
            ExportedName, FunctionDef, Import, ImportHiddenName, ImportSelector, Item, Module,
            SelectedName, TypeAlias,
        },
    },
    diag::{
        AnyDiagnostic, Diagnostic, DiagnosticCode, LabelSpan, Offset, sort_dedup_query_diagnostics,
    },
    input::SourceFile,
    nameres as hir_nameres,
    span::{AnchorId, Span, Spanned, SpannedElem},
};
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{Level, field};

mod diagnostics;
mod env;
mod graph;
mod instances;
mod interface;
mod item_refs;
mod model;
mod modes;
mod paths;
mod scc;
mod util;
mod validation;

pub use diagnostics::{
    ModuleDiagnostic, body_diagnostics, module_diagnostics, reachable_diagnostics,
};
pub use env::{module_env, module_import_surface, resolve_module_full};
pub use graph::{module_graph, module_imports, reachable_modules, resolve_reachable_full};
pub use instances::{instance_imports, module_instances};
pub use interface::public_interface;
pub use model::{
    ConstructorVisibility, Db, FullResolutionSummary, InstanceImports, Interface, ItemRef,
    LibraryId, ModuleAlias, ModuleEdge, ModuleEnv, ModuleFsSnapshot, ModuleGraph, ModuleId,
    ModuleImportSurface, ModuleImports, ModuleKey, ModulePathRef, ModuleTree, Namespace, Origin,
    ResolvedModulePath, ValidationSummary, VisibleConstructors,
};
pub use paths::{resolve_module_path, resolve_module_path_candidate};
pub use scc::strongly_connected_components;
pub use util::{
    module_file_path, module_id_display, module_id_from_key, module_key_for_path,
    module_path_display,
};
pub use validation::{validate_module, validate_reachable};

use diagnostics::{
    ambiguous_import_diag, conflicting_unqualified_name_diag, duplicate_export_item_diag,
    duplicate_export_module_diag, duplicate_qualifier_diag, duplicate_selector_diag,
    missing_external_root_diag, module_not_found_diag, module_root_span, unknown_import_item_diag,
    unknown_local_ctor_diag, unknown_local_export_diag, unknown_reexport_ctor_diag,
    unknown_reexport_diag,
};
use env::module_has_parse_errors;
use interface::{
    RawInterface, RawItemRef, RawModuleAlias, expand_module_exports, namespace_sort_key,
    resolve_for_export,
};
use item_refs::{
    ConstructorDiagnostic, ConstructorDiagnosticCtx, class_methods_for_ref,
    constructor_entries_for_ref, import_module_qualifiers, local_data_ref_with_constructors,
    local_importable_refs, local_refs_for_name, module_prefixes, path_ref_from_import,
    path_ref_from_segments, path_ref_from_text, path_refs_from_export, qualified_surface_name,
    qualify, resolution_for_item_ref, select_import_refs, selected_imported_refs,
    strip_constructor_visibility, visible_data_ref_with_constructors,
};
use modes::{BodyDiagnosticPolicy, CtorInclusion, ExportResolutionMode};
use paths::{module_path_span, path_segments};
use util::{
    best_name_suggestion, ident_text, namespace_context, private_surface_key, record_body_field,
    record_module_field, record_source_file_field, selector_kind, sorted_namespaces,
    spanned_name_text, trace_import_decision, unique_modules, unique_origins, unique_strings,
};
use validation::{
    default_module_binding_name, interface_names, validate_duplicate_exports, validate_imports,
};
