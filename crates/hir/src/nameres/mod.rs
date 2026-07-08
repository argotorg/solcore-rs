//! Intra-module name resolution.
//!
//! This resolver builds lexical item/body scopes for one lowered module and
//! records what every type reference, predicate, expression, statement binder,
//! and pattern binder resolves to. Inter-module imports are injected through
//! the `ImportedNames` trait; this crate remains responsible for local language
//! semantics and builtin lookup.
//!
//! Solcore has distinct type and term namespaces. Type aliases, data types,
//! contracts, classes, type variables, and builtin type/class names live in the
//! type namespace. Functions, constructors, class methods, parameters, locals,
//! fields, modules used as qualifiers, and builtin values/functions live in the
//! term/module lookup surface. Constructor leaves are intentionally not
//! accepted unqualified when they would be ambiguous with the type that owns
//! them; callers must use qualified constructor syntax.
//!
//! Body scoping follows the reference semantics:
//! - A `let` initializer is resolved before the `let` binder is inserted, so
//!   the initializer cannot refer to the binding being declared.
//! - `for` statements do not introduce their own lexical scope; their
//!   initializer, condition, post statements, and body share the surrounding
//!   scope.
//! - Inside a contract, fields beat same-name functions for bare references,

use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{Level, field};

use crate::{
    Db,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{
            Expr, ExprKind, FuncBody, FuncParam, FuncSig, MatchArm, Pat, PatKind, Stmt, StmtKind,
        },
        item::{
            AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FunctionDef, InstanceDef, Item,
            Module, TypeAlias,
        },
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    diag::{Diagnostic, LabelSpan},
    span::{Span, Spanned, SpannedElem},
};

mod body_resolver;
mod builtins;
mod diagnostic;
mod model;
mod queries;
mod scope;
mod type_resolver;
mod util;

use body_resolver::BodyResolver;
use builtins::{best_name_suggestion, builtin_term, builtin_type_or_class};
use diagnostic::{
    duplicate_diagnostic, invalid_pattern, undefined_class, undefined_name, undefined_type_ctor,
    unqualified_constructor,
};
use scope::ItemScopeBuilder;
use type_resolver::TypeResolver;
use util::{
    collect_constructor_type_candidates, expr_path, ident_text, param_bindings, param_name,
    path_span, qualify, record_body_fields, record_module_fields, type_var_bindings,
    unique_constructor_type_candidate,
};

pub use diagnostic::NameresDiagnostic;
pub use model::*;
pub use queries::*;
