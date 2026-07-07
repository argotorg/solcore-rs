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
//!   while unqualified call callees resolve callable terms before fields.

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

/// Name-resolution namespace.
///
/// Type and term are the language namespaces. Field and module are represented
/// separately so diagnostics and import integration can distinguish lookup
/// surfaces that are not duplicate-checked like ordinary declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Namespace {
    /// Type-level names: aliases, ADTs, contracts, classes, type variables.
    Type,
    /// Term-level names: functions, constructors, locals, parameters, methods.
    Term,
    /// Contract field names.
    Field,
    /// Imported module binding names.
    Module,
}

/// Kind of user definition reached by a resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum DefResolutionKind {
    /// Function, method, constructor, or fallback definition.
    Function,
    /// Contract definition.
    Contract,
    /// Algebraic data type definition.
    Adt,
    /// Type alias definition.
    TypeAlias,
    /// Type class definition.
    Class,
    /// Type class instance definition.
    Instance,
}

/// Stable reference to a contract field.
///
/// Fields are identified by their owning contract definition and declaration
/// index, which is stable under unrelated edits inside the contract body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldId<'db> {
    /// Owning contract definition.
    pub contract: DefId<'db>,
    /// Zero-based field declaration index.
    pub index: u32,
}

/// Logical module binding visible in an item scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleRef<'db> {
    /// Module definition that owns the binding.
    pub owner: DefId<'db>,
    /// Surface name used as the module qualifier.
    pub name: String,
}

/// Stable reference to a type variable binder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeVarId<'db> {
    /// Definition that owns the type variable list.
    pub owner: DefId<'db>,
    /// Zero-based binder index in the owner.
    pub index: u32,
    /// Binder name.
    pub name: String,
}

/// Stable reference to a function-body parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParamId<'db> {
    /// Body whose parameter list introduced this parameter.
    pub body: FuncBody<'db>,
    /// Zero-based parameter index.
    pub index: u32,
}

/// Local binding introduced inside a body or type binder list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum LocalBinding<'db> {
    /// Binding introduced by a `let` statement.
    Let {
        /// Body containing the statement.
        body: FuncBody<'db>,
        /// Statement ID that introduced the binding.
        stmt: Id<Stmt<'db>>,
    },
    /// Binding introduced by a pattern.
    Pattern {
        /// Body containing the pattern.
        body: FuncBody<'db>,
        /// Pattern ID that introduced the binding.
        pat: Id<Pat<'db>>,
    },
    /// Type variable binding.
    TypeVar(TypeVarId<'db>),
}

/// Builtin type names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinType {
    /// `word`.
    Word,
    /// `bool`.
    Bool,
    /// `string`.
    String,
    /// Unit type `()`.
    Unit,
    /// Binary product type constructor.
    Pair,
    /// Binary sum type constructor.
    Sum,
    /// Integer type.
    Integer,
}

/// Builtin class names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinClass {
    /// `invokable`.
    Invokable,
    /// `Int`.
    Int,
}

/// Builtin constructor names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinCtor {
    /// Boolean `true`.
    True,
    /// Boolean `false`.
    False,
    /// Unit constructor `()`.
    Unit,
    /// Pair constructor.
    Pair,
    /// Sum left constructor.
    Inl,
    /// Sum right constructor.
    Inr,
}

/// Builtin function names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinFunction {
    /// `invoke`.
    Invoke,
    /// Primitive word addition.
    PrimAddWord,
    /// Primitive word equality.
    PrimEqWord,
    /// Conversion from word to integer.
    WordToInteger,
    /// Conversion from integer to word.
    WordFromInteger,
    /// Integer addition.
    IntegerAdd,
    /// Integer subtraction.
    IntegerSub,
    /// Integer multiplication.
    IntegerMul,
    /// Integer less-than comparison.
    IntegerLt,
    /// Integer equality.
    IntegerEq,
}

/// Builtin class method names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinClassMethod {
    /// `invokable.invoke`.
    InvokableInvoke,
    /// `Int.fromInteger`.
    IntFromInteger,
}

/// Builtin resolution category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinKind {
    /// Builtin type.
    Type(BuiltinType),
    /// Builtin class.
    Class(BuiltinClass),
    /// Builtin constructor.
    Constructor(BuiltinCtor),
    /// Builtin function.
    Function(BuiltinFunction),
    /// Builtin class method.
    ClassMethod(BuiltinClassMethod),
}

/// Result of resolving a name occurrence or binder.
///
/// `Err` records that resolution failed, or that parser/import recovery made
/// the target intentionally unknown and diagnostics were suppressed at the
/// caller boundary.
/// `DotCtorDeferred` is used for leading-dot constructor syntax whose concrete
/// type is determined later by type information.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Resolution<'db> {
    /// User definition.
    Def {
        /// Definition identity.
        def: DefId<'db>,
        /// Definition category.
        kind: DefResolutionKind,
    },
    /// Local binding.
    Local(LocalBinding<'db>),
    /// Function or lambda parameter.
    Param(ParamId<'db>),
    /// Contract field.
    Field(FieldId<'db>),
    /// Data constructor.
    Ctor {
        /// Owning data type.
        ty: DefId<'db>,
        /// Constructor index in the owning data type.
        index: u32,
    },
    /// Type class method.
    ClassMethod {
        /// Owning class.
        class: DefId<'db>,
        /// Method name.
        name: String,
    },
    /// Module qualifier.
    Module(ModuleRef<'db>),
    /// Leading-dot constructor lookup deferred to type checking.
    DotCtorDeferred,
    /// Builtin item.
    Builtin(BuiltinKind),
    /// Failed resolution after diagnostics.
    Err,
}

/// Name exported by an item or imported scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ScopeEntry<'db> {
    /// Surface name in the relevant namespace.
    pub name: String,
    /// Span of the declaration or imported binding.
    pub span: Span<'db>,
    /// Resolution reached by the name.
    pub resolution: Resolution<'db>,
}

/// Constructor entry in a type's constructor list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct CtorEntry<'db> {
    /// Unqualified constructor leaf name.
    pub name: String,
    /// Qualified constructor name, usually `Type.Ctor`.
    pub qualified_name: String,
    /// Span of the constructor declaration.
    pub span: Span<'db>,
    /// Owning data type.
    pub ty: DefId<'db>,
    /// Constructor index in declaration order.
    pub index: u32,
}

/// Constructors associated with one data type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct CtorList<'db> {
    /// Owning data type.
    pub ty: DefId<'db>,
    /// Type name used for qualification.
    pub ty_name: String,
    /// Constructor entries in declaration order.
    pub ctors: Vec<CtorEntry<'db>>,
}

/// Contract field entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldEntry<'db> {
    /// Field name.
    pub name: String,
    /// Span of the field declaration.
    pub span: Span<'db>,
    /// Stable field identity.
    pub field: FieldId<'db>,
}

/// Name scope contributed by a contract body.
///
/// Contract scopes are nested below the module scope. They contain
/// contract-local types, terms, fields, and constructors, and are consulted
/// when resolving code inside that contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ContractScope<'db> {
    /// Contract definition that owns this scope.
    pub contract: DefId<'db>,
    /// Contract name.
    pub name: String,
    /// Contract-local type entries.
    pub types: Vec<ScopeEntry<'db>>,
    /// Contract-local term entries.
    pub terms: Vec<ScopeEntry<'db>>,
    /// Field entries.
    pub fields: Vec<FieldEntry<'db>>,
    /// Constructor lists declared inside the contract.
    pub ctor_lists: Vec<CtorList<'db>>,
}

/// Item-level scope for one module.
///
/// The scope records declarations before body resolution so functions can refer
/// to later items in the same module. Duplicate diagnostics are emitted while
/// building this value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ItemScope<'db> {
    /// Module this scope belongs to.
    pub module: Module<'db>,
    /// Type namespace entries.
    pub types: Vec<ScopeEntry<'db>>,
    /// Term namespace entries.
    pub terms: Vec<ScopeEntry<'db>>,
    /// Module qualifier entries introduced by imports.
    pub modules: Vec<ScopeEntry<'db>>,
    /// Top-level constructor lists.
    pub ctor_lists: Vec<CtorList<'db>>,
    /// Contract-local scopes.
    pub contracts: Vec<ContractScope<'db>>,
    /// Instance definitions in source order.
    pub instances: Vec<InstanceDef<'db>>,
    /// Diagnostics found while building item scopes.
    pub diagnostics: Vec<NameresDiagnostic>,
}

/// Resolution attached to an unresolved type reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeResolution<'db> {
    /// Type reference being resolved.
    pub ty: TypeRef<'db>,
    /// Resolution for the named constructor or `Err`.
    pub resolution: Resolution<'db>,
}

/// Resolution attached to an unresolved predicate reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredResolution<'db> {
    /// Predicate being resolved.
    pub pred: PredRef<'db>,
    /// Resolution for the class name or `Err`.
    pub resolution: Resolution<'db>,
}

/// Type and predicate resolutions for item signatures.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct ItemResolutionMap<'db> {
    /// Resolved type references.
    pub types: Vec<TypeResolution<'db>>,
    /// Resolved predicate references.
    pub preds: Vec<PredResolution<'db>>,
    /// Diagnostics found while resolving item signatures.
    pub diagnostics: Vec<NameresDiagnostic>,
}

/// Resolution attached to an expression occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyExprResolution<'db> {
    /// Body containing the expression.
    pub body: FuncBody<'db>,
    /// Expression ID in the body arena.
    pub expr: Id<Expr<'db>>,
    /// Resolved expression name or sentinel.
    pub resolution: Resolution<'db>,
}

/// Resolution attached to a statement binder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyStmtResolution<'db> {
    /// Body containing the statement.
    pub body: FuncBody<'db>,
    /// Statement ID that introduced the binder.
    pub stmt: Id<Stmt<'db>>,
    /// Local binding resolution for the statement.
    pub resolution: Resolution<'db>,
}

/// Resolution attached to a pattern binder or constructor occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyPatResolution<'db> {
    /// Body containing the pattern.
    pub body: FuncBody<'db>,
    /// Pattern ID in the body arena.
    pub pat: Id<Pat<'db>>,
    /// Pattern resolution.
    pub resolution: Resolution<'db>,
}

/// Name-resolution results for one function body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct BodyResolutionMap<'db> {
    /// Expression resolutions.
    pub exprs: Vec<BodyExprResolution<'db>>,
    /// Statement binder resolutions.
    pub stmt_bindings: Vec<BodyStmtResolution<'db>>,
    /// Pattern resolutions.
    pub pats: Vec<BodyPatResolution<'db>>,
    /// Type references used in the body.
    pub types: Vec<TypeResolution<'db>>,
    /// Predicate references used in the body.
    pub preds: Vec<PredResolution<'db>>,
    /// Diagnostics found while resolving this body.
    pub diagnostics: Vec<NameresDiagnostic>,
}

/// Parameter binding passed into body resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParamBinding<'db> {
    /// Parameter name with source span.
    pub name: SpannedElem<'db, Ident<'db>>,
}

/// Type-variable binding passed into body or item resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeVarBinding<'db> {
    /// Definition that owns the type variable list.
    pub owner: DefId<'db>,
    /// Type variable name with source span.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Zero-based binder index.
    pub index: u32,
}

/// Context required to resolve a function body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyResolutionContext<'db> {
    /// Module containing the body.
    pub module: Module<'db>,
    /// Contract enclosing the body, if any.
    pub enclosing_contract: Option<DefId<'db>>,
    /// Parameters visible at body entry.
    pub params: Vec<ParamBinding<'db>>,
    /// Type variables visible at body entry.
    pub type_vars: Vec<TypeVarBinding<'db>>,
}

/// Complete local resolution result for one module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleResolutionMap<'db> {
    /// Item-level scope built for the module.
    pub item_scope: ItemScope<'db>,
    /// Type and predicate resolutions in item signatures.
    pub item_resolutions: ItemResolutionMap<'db>,
    /// Body resolution maps for functions and methods.
    pub bodies: Vec<BodyResolutionMap<'db>>,
    /// Diagnostics found while resolving this module.
    pub diagnostics: Vec<NameresDiagnostic>,
}

/// Diagnostic emission policy for name resolution.
///
/// Parser recovery can leave `Error` HIR nodes and can also lose declarations.
/// When a source file already has parse diagnostics, callers should still build
/// resolution maps for editor features, but must suppress all nameres
/// diagnostics. This matches the reference behavior of stopping after parse
/// errors and avoids showing cascades from an incomplete recovered HIR. We also
/// suppress `SC0108` duplicate diagnostics in this mode because recovery can
/// distort item boundaries, so even structure-like checks are not guaranteed to
/// be sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameresDiagnosticPolicy {
    /// Emit name-resolution diagnostics normally.
    Emit,
    /// Keep resolution data but clear all name-resolution diagnostics.
    SuppressForParseErrors,
}

impl NameresDiagnosticPolicy {
    fn suppresses_diagnostics(self) -> bool {
        matches!(self, Self::SuppressForParseErrors)
    }
}

/// Provider of names imported from other modules.
///
/// HIR name resolution is parameterized by this trait so the inter-module
/// resolver can inject imported items without making `hir` depend on the module
/// graph crate.
pub trait ImportedNames<'db> {
    /// Looks up an imported name in `namespace`.
    fn imported(
        &self,
        db: &'db dyn Db,
        namespace: Namespace,
        name: &str,
    ) -> Option<Resolution<'db>>;

    /// Returns whether any imported constructor has the given unqualified leaf.
    ///
    /// The default is `false` so purely local resolution can ignore import
    /// constructor ambiguity.
    fn has_constructor_leaf(&self, _db: &'db dyn Db, _leaf: &str) -> bool {
        false
    }

    /// Returns whether an imported parse-broken module may still contain this
    /// unqualified name.
    ///
    /// Import providers with parse errors have an incomplete public interface:
    /// absence from the recovered interface is not evidence that a name is
    /// truly missing. Returning `true` lets HIR resolution produce
    /// [`Resolution::Err`] without an undefined-name diagnostic.
    fn may_contain_unknown_unqualified(
        &self,
        _db: &'db dyn Db,
        _namespace: Namespace,
        _name: &str,
    ) -> bool {
        false
    }

    /// Returns whether a module qualifier targets a parse-broken provider whose
    /// members are therefore unknown.
    fn has_incomplete_module_qualifier(&self, _db: &'db dyn Db, _qualifier: &str) -> bool {
        false
    }
}

/// Empty import provider used by standalone HIR queries.
#[derive(Debug, Clone, Copy)]
pub struct EmptyImportedNames;

impl<'db> ImportedNames<'db> for EmptyImportedNames {
    fn imported(
        &self,
        _db: &'db dyn Db,
        _namespace: Namespace,
        _name: &str,
    ) -> Option<Resolution<'db>> {
        None
    }
}

/// Typed local name-resolution diagnostic.
///
/// The variants mirror the `SC010x` local resolver codes and store
/// lifetime-free label spans. Lowering to the generic user-facing diagnostic is
/// deferred until the driver or another diagnostic edge asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum NameresDiagnostic {
    /// `SC0101`: failed term, field, module, or qualified-name lookup.
    UndefinedName {
        /// Name text as it appeared at the failing lookup.
        name: String,
        /// Source span of the failed lookup.
        span: LabelSpan,
    },
    /// `SC0103`: failed type-constructor lookup.
    UndefinedTypeConstructor {
        /// Type constructor name.
        name: String,
        /// Source span of the failed lookup.
        span: LabelSpan,
    },
    /// `SC0105`: failed class lookup.
    UndefinedClass {
        /// Class name.
        name: String,
        /// Source span of the failed lookup.
        span: LabelSpan,
    },
    /// `SC0106`: constructor used without the required type qualifier.
    UnqualifiedConstructor {
        /// Constructor leaf name.
        name: String,
        /// Source span of the constructor occurrence.
        span: LabelSpan,
    },
    /// `SC0107`: parser recovery produced an invalid pattern shape.
    InvalidPattern {
        /// Source span covering the invalid pattern.
        span: LabelSpan,
    },
    /// `SC0108`: duplicate declaration in a local namespace.
    DuplicateDeclaration {
        /// Namespace where the duplicate was found.
        namespace: Namespace,
        /// Duplicated surface name.
        name: String,
        /// Span of the duplicate declaration.
        span: LabelSpan,
        /// Span of the first declaration.
        previous: LabelSpan,
        /// Optional contextual note, such as the enclosing contract.
        context: Option<String>,
    },
}

impl NameresDiagnostic {
    /// Lowers this typed diagnostic to the generic rendering surface.
    pub fn lower(&self, _db: &dyn Db) -> Diagnostic {
        match self {
            NameresDiagnostic::UndefinedName { name, span } => {
                Diagnostic::error(format!("undefined name: {name}"))
                    .with_code("SC0101")
                    .with_primary_label_span(span.clone(), Some("unknown name"))
            }
            NameresDiagnostic::UndefinedTypeConstructor { name, span } => {
                Diagnostic::error(format!("undefined type constructor: {name}"))
                    .with_code("SC0103")
                    .with_primary_label_span(span.clone(), Some("undefined type constructor"))
            }
            NameresDiagnostic::UndefinedClass { name, span } => {
                Diagnostic::error(format!("undefined class: {name}"))
                    .with_code("SC0105")
                    .with_primary_label_span(span.clone(), Some("undefined class"))
            }
            NameresDiagnostic::UnqualifiedConstructor { name, span } => {
                Diagnostic::error(format!("unqualified constructor: {name}"))
                    .with_code("SC0106")
                    .with_primary_label_span(span.clone(), Some("constructor must be qualified"))
                    .with_note("use Type.Constructor form")
            }
            NameresDiagnostic::InvalidPattern { span } => {
                Diagnostic::error("invalid pattern syntax")
                    .with_code("SC0107")
                    .with_primary_label_span(span.clone(), Some("invalid pattern"))
            }
            NameresDiagnostic::DuplicateDeclaration {
                namespace,
                name,
                span,
                previous,
                context,
            } => {
                let namespace_text = match namespace {
                    Namespace::Type => "type namespace",
                    Namespace::Term => "term namespace",
                    Namespace::Field | Namespace::Module => "namespace",
                };
                let mut diagnostic = Diagnostic::error(format!(
                    "duplicate declaration `{name}` in {namespace_text}"
                ))
                .with_code("SC0108")
                .with_primary_label_span(span.clone(), Some("duplicate declaration"))
                .with_secondary_label_span(previous.clone(), Some("previous declaration"));
                if let Some(context) = context {
                    diagnostic = diagnostic.with_note(format!("context: {context}"));
                }
                diagnostic
            }
        }
    }
}

impl<'db> ItemScope<'db> {
    /// Resolves a type name declared in this module scope.
    pub fn type_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.types
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    /// Resolves a term name declared in this module scope.
    pub fn term_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.terms
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    /// Resolves a module qualifier name introduced by imports.
    pub fn module_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.modules
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    /// Returns the contract-local scope for `contract`.
    pub fn contract_scope(&self, contract: DefId<'db>) -> Option<&ContractScope<'db>> {
        self.contracts
            .iter()
            .find(|scope| scope.contract == contract)
    }

    /// Returns whether any visible constructor has the given leaf name.
    ///
    /// This powers diagnostics for unqualified constructor use and does not
    /// resolve to a concrete constructor by itself.
    pub fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.ctor_lists
            .iter()
            .flat_map(|list| &list.ctors)
            .any(|ctor| ctor.name == leaf)
            || self
                .contracts
                .iter()
                .flat_map(|scope| &scope.ctor_lists)
                .flat_map(|list| &list.ctors)
                .any(|ctor| ctor.name == leaf)
    }
}

impl<'db> ContractScope<'db> {
    fn type_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.types
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    fn term_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.terms
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    fn field_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.fields
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| Resolution::Field(entry.field))
    }

    fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.ctor_lists
            .iter()
            .flat_map(|list| &list.ctors)
            .any(|ctor| ctor.name == leaf)
    }
}

impl<'db> BodyResolutionMap<'db> {
    fn record_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: Resolution<'db>,
    ) {
        self.exprs.push(BodyExprResolution {
            body,
            expr,
            resolution,
        });
    }

    fn record_stmt(
        &mut self,
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
        resolution: Resolution<'db>,
    ) {
        self.stmt_bindings.push(BodyStmtResolution {
            body,
            stmt,
            resolution,
        });
    }

    fn record_pat(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>, resolution: Resolution<'db>) {
        self.pats.push(BodyPatResolution {
            body,
            pat,
            resolution,
        });
    }
}

impl<'db> ItemResolutionMap<'db> {
    fn apply_diagnostic_policy(&mut self, policy: NameresDiagnosticPolicy) {
        if policy.suppresses_diagnostics() {
            self.diagnostics.clear();
        }
    }
}

impl<'db> BodyResolutionMap<'db> {
    fn apply_diagnostic_policy(&mut self, policy: NameresDiagnosticPolicy) {
        if policy.suppresses_diagnostics() {
            self.diagnostics.clear();
        }
    }
}

impl<'db> ModuleResolutionMap<'db> {
    fn apply_diagnostic_policy(&mut self, policy: NameresDiagnosticPolicy) {
        if !policy.suppresses_diagnostics() {
            return;
        }
        self.item_scope.diagnostics.clear();
        self.item_resolutions.apply_diagnostic_policy(policy);
        for body in &mut self.bodies {
            body.apply_diagnostic_policy(policy);
        }
        self.diagnostics.clear();
    }
}

fn record_module_fields<'db>(db: &'db dyn Db, module: Module<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        record_def_fields(db, module.def_id_value(db));
    }
}

fn record_body_fields<'db>(db: &'db dyn Db, body: FuncBody<'db>) {
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

/// Builds the item-level scope for `module`.
///
/// This query collects declarations before resolving bodies so forward
/// references between top-level items are legal. It also emits duplicate-name
/// diagnostics for the type and term namespaces.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn item_scope<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemScope<'db> {
    record_module_fields(db, module);
    let mut builder = ItemScopeBuilder::new(db, module);
    for item in module.items(db) {
        builder.add_item(*item);
    }
    builder.finish()
}

/// Resolves type and predicate references in item signatures without imports.
///
/// This is the standalone HIR query. Inter-module callers should use
/// [`resolve_item_types_with_imports`] so imported names participate in lookup.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_item_types<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemResolutionMap<'db> {
    record_module_fields(db, module);
    let scope = item_scope(db, module);
    let imports = EmptyImportedNames;
    resolve_item_types_with_imports(db, module, &scope, &imports)
}

/// Resolves type and predicate references in item signatures with imported
/// names.
///
/// `scope` must be the item scope for `module`. `imports` is consulted after
/// local item/contract scopes and before builtin names.
pub fn resolve_item_types_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: &ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ItemResolutionMap<'db> {
    let mut resolver = TypeResolver::new(db, scope, imports);
    for item in module.items(db) {
        resolver.item(*item, None, &[]);
    }
    resolver.map
}

/// Resolves one function body without imported names.
///
/// `context` supplies the module, optional enclosing contract, parameters, and
/// inherited type variables. The returned map is silent for parser `Error`
/// nodes; parse diagnostics are produced during lowering.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, body, context),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_body<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: BodyResolutionContext<'db>,
) -> BodyResolutionMap<'db> {
    record_body_fields(db, body);
    let imports = EmptyImportedNames;
    resolve_body_with_imports(db, body, &context, &imports)
}

/// Resolves one function body with imported names.
///
/// This entry point is used by the inter-module resolver. It preserves the
/// local scoping rules documented at module level and consults `imports` only
/// after local/field/item lookup has failed.
pub fn resolve_body_with_imports<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: &BodyResolutionContext<'db>,
    imports: &dyn ImportedNames<'db>,
) -> BodyResolutionMap<'db> {
    resolve_body_with_imports_and_policy(db, body, context, imports, NameresDiagnosticPolicy::Emit)
}

/// Resolves one function body with imported names and an explicit diagnostic
/// policy.
pub fn resolve_body_with_imports_and_policy<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: &BodyResolutionContext<'db>,
    imports: &dyn ImportedNames<'db>,
    policy: NameresDiagnosticPolicy,
) -> BodyResolutionMap<'db> {
    let scope = item_scope(db, context.module);
    let mut resolver = BodyResolver::new(db, &scope, imports, context.enclosing_contract);
    resolver.with_type_vars(&context.type_vars, |resolver| {
        resolver.with_scope(|resolver| {
            for (index, param) in context.params.iter().enumerate() {
                resolver.add_param(body, index as u32, &param.name);
            }
            resolver.body(body);
        });
    });
    let mut map = resolver.map;
    map.apply_diagnostic_policy(policy);
    map
}

/// Resolves all item signatures and function bodies in a module without
/// imports.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_module<'db>(db: &'db dyn Db, module: Module<'db>) -> ModuleResolutionMap<'db> {
    record_module_fields(db, module);
    let scope = item_scope(db, module);
    let imports = EmptyImportedNames;
    resolve_module_with_imports(db, module, scope, &imports)
}

/// Resolves all item signatures and function bodies in a module with imports.
///
/// The supplied `scope` is reused for both item and body resolution so
/// duplicate diagnostics and lookup surfaces are computed once.
pub fn resolve_module_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ModuleResolutionMap<'db> {
    resolve_module_with_imports_and_policy(
        db,
        module,
        scope,
        imports,
        NameresDiagnosticPolicy::Emit,
    )
}

/// Resolves all item signatures and function bodies with an explicit diagnostic
/// policy.
pub fn resolve_module_with_imports_and_policy<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
    policy: NameresDiagnosticPolicy,
) -> ModuleResolutionMap<'db> {
    let item_resolutions = resolve_item_types_with_imports(db, module, &scope, imports);
    let mut bodies = Vec::new();
    for item in module.items(db) {
        collect_item_body_resolutions(db, module, *item, None, &[], imports, &mut bodies);
    }
    let mut diagnostics = scope.diagnostics.clone();
    diagnostics.extend(item_resolutions.diagnostics.iter().cloned());
    for body in &bodies {
        diagnostics.extend(body.diagnostics.iter().cloned());
    }
    let mut map = ModuleResolutionMap {
        item_scope: scope,
        item_resolutions,
        bodies,
        diagnostics,
    };
    map.apply_diagnostic_policy(policy);
    map
}

fn collect_item_body_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item: Item<'db>,
    enclosing_contract: Option<ContractDef<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn ImportedNames<'db>,
    bodies: &mut Vec<BodyResolutionMap<'db>>,
) {
    match item {
        Item::FunctionDef(def) => {
            collect_function_body_resolution(
                db,
                module,
                def,
                enclosing_contract.map(|contract| contract.def_id_value(db)),
                inherited_type_vars,
                imports,
                bodies,
            );
        }
        Item::InstanceDef(def) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                db,
                def.def_id_value(db),
                def.type_var_elems(db),
            ));
            for method in def.methods(db) {
                collect_function_body_resolution(
                    db,
                    module,
                    *method,
                    enclosing_contract.map(|contract| contract.def_id_value(db)),
                    &inherited,
                    imports,
                    bodies,
                );
            }
        }
        Item::ContractDef(def) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                db,
                def.def_id_value(db),
                def.ty_param_elems(db),
            ));
            for item in def.items(db) {
                match *item {
                    ContractItem::FunctionDef(defn) => {
                        collect_function_body_resolution(
                            db,
                            module,
                            defn,
                            Some(def.def_id_value(db)),
                            &inherited,
                            imports,
                            bodies,
                        );
                    }
                    ContractItem::TypeAlias(_)
                    | ContractItem::AdtDef(_)
                    | ContractItem::Error { .. } => {}
                }
            }
        }
        Item::TypeAlias(_)
        | Item::AdtDef(_)
        | Item::ClassDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => {}
    }
}

fn collect_function_body_resolution<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn ImportedNames<'db>,
    bodies: &mut Vec<BodyResolutionMap<'db>>,
) {
    let Some(body) = function.body(db) else {
        return;
    };
    let sig = function.sig(db);
    let mut type_vars = inherited_type_vars.to_vec();
    type_vars.extend(type_var_bindings(
        db,
        function.def_id_value(db),
        &sig.type_vars,
    ));
    let context = BodyResolutionContext {
        module,
        enclosing_contract,
        params: param_bindings(sig.params.atom()),
        type_vars,
    };
    bodies.push(resolve_body_with_imports(db, body, &context, imports));
}

struct ItemScopeBuilder<'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    types: Vec<ScopeEntry<'db>>,
    terms: Vec<ScopeEntry<'db>>,
    modules: Vec<ScopeEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    contracts: Vec<ContractScope<'db>>,
    instances: Vec<InstanceDef<'db>>,
    type_names: FxHashMap<String, Vec<(TypeDeclFamily, Span<'db>)>>,
    term_names: FxHashMap<String, Span<'db>>,
    diagnostics: Vec<NameresDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeDeclFamily {
    Alias,
    Adt,
    Class,
    Contract,
}

impl<'db> ItemScopeBuilder<'db> {
    fn new(db: &'db dyn Db, module: Module<'db>) -> Self {
        Self {
            db,
            module,
            types: Vec::new(),
            terms: Vec::new(),
            modules: Vec::new(),
            ctor_lists: Vec::new(),
            contracts: Vec::new(),
            instances: Vec::new(),
            type_names: FxHashMap::default(),
            term_names: FxHashMap::default(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self) -> ItemScope<'db> {
        ItemScope {
            module: self.module,
            types: self.types,
            terms: self.terms,
            modules: self.modules,
            ctor_lists: self.ctor_lists,
            contracts: self.contracts,
            instances: self.instances,
            diagnostics: self.diagnostics,
        }
    }

    fn add_item(&mut self, item: Item<'db>) {
        match item {
            Item::FunctionDef(def) => self.add_function(def, None),
            Item::TypeAlias(def) => self.add_alias(def, None),
            Item::AdtDef(def) => self.add_adt(def, None),
            Item::ClassDef(def) => self.add_class(def),
            Item::InstanceDef(def) => self.instances.push(def),
            Item::ContractDef(def) => self.add_contract(def),
            Item::Import(def) => {
                self.add_import_modules(def.path_elems(self.db), def.alias_elem(self.db))
            }
            Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
    }

    fn add_type(
        &mut self,
        name: SpannedElem<'db, Ident<'db>>,
        resolution: Resolution<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
        family: TypeDeclFamily,
    ) {
        let text = ident_text(self.db, &name).to_owned();
        if let Some(contract) = contract {
            contract.add_type(text, name.span(self.db), resolution);
            return;
        }
        self.check_type_duplicate(&text, name.span(self.db), family);
        self.types.push(ScopeEntry {
            name: text,
            span: name.span(self.db),
            resolution,
        });
    }

    fn add_term(
        &mut self,
        name: String,
        span: Span<'db>,
        resolution: Resolution<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
        check_duplicate: bool,
    ) {
        if let Some(contract) = contract {
            contract.add_term(name, span, resolution, check_duplicate);
            return;
        }
        if check_duplicate {
            self.check_duplicate(Namespace::Term, &name, span, None);
        }
        self.terms.push(ScopeEntry {
            name,
            span,
            resolution,
        });
    }

    fn add_function(
        &mut self,
        def: FunctionDef<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
    ) {
        let sig = def.sig(self.db);
        self.add_term(
            ident_text(self.db, &sig.name).to_owned(),
            sig.name.span(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Function,
            },
            contract,
            true,
        );
    }

    fn add_alias(&mut self, def: TypeAlias<'db>, contract: Option<&mut ContractScopeBuilder<'db>>) {
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::TypeAlias,
            },
            contract,
            TypeDeclFamily::Alias,
        );
    }

    fn add_adt(&mut self, def: AdtDef<'db>, mut contract: Option<&mut ContractScopeBuilder<'db>>) {
        let ty_name = ident_text(self.db, &def.name_elem(self.db)).to_owned();
        let ty_def = def.def_id_value(self.db);
        let mut ctor_entries = Vec::new();
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: ty_def,
                kind: DefResolutionKind::Adt,
            },
            contract.as_deref_mut(),
            TypeDeclFamily::Adt,
        );
        for (index, ctor) in def.ctors(self.db).iter().enumerate() {
            let ctor_name = ident_text(self.db, &ctor.name).to_owned();
            let qualified = qualify(&ty_name, &ctor_name);
            let entry = CtorEntry {
                name: ctor_name,
                qualified_name: qualified.clone(),
                span: ctor.name.span(self.db),
                ty: ty_def,
                index: index as u32,
            };
            ctor_entries.push(entry);
            self.add_term(
                qualified,
                ctor.name.span(self.db),
                Resolution::Ctor {
                    ty: ty_def,
                    index: index as u32,
                },
                contract.as_deref_mut(),
                true,
            );
        }

        let list = CtorList {
            ty: ty_def,
            ty_name,
            ctors: ctor_entries,
        };
        if let Some(contract) = contract {
            contract.ctor_lists.push(list);
        } else {
            self.ctor_lists.push(list);
        }
    }

    fn add_class(&mut self, def: ClassDef<'db>) {
        let head = def.head(self.db);
        let class_name = head.kind(self.db).class;
        let class_text = ident_text(self.db, &class_name).to_owned();
        self.add_type(
            class_name,
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Class,
            },
            None,
            TypeDeclFamily::Class,
        );
        for method in def.methods(self.db) {
            let method_name = ident_text(self.db, &method.name).to_owned();
            self.add_term(
                qualify(&class_text, &method_name),
                method.name.span(self.db),
                Resolution::ClassMethod {
                    class: def.def_id_value(self.db),
                    name: method_name,
                },
                None,
                false,
            );
        }
    }

    fn add_contract(&mut self, def: ContractDef<'db>) {
        let contract_name = ident_text(self.db, &def.name_elem(self.db)).to_owned();
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Contract,
            },
            None,
            TypeDeclFamily::Contract,
        );
        let mut contract =
            ContractScopeBuilder::new(self.db, def.def_id_value(self.db), contract_name);
        for (index, field) in def.fields(self.db).iter().enumerate() {
            contract.add_field(field, index as u32);
        }
        for item in def.items(self.db) {
            match *item {
                ContractItem::FunctionDef(def) => self.add_function(def, Some(&mut contract)),
                ContractItem::TypeAlias(def) => self.add_alias(def, Some(&mut contract)),
                ContractItem::AdtDef(def) => self.add_adt(def, Some(&mut contract)),
                ContractItem::Error { .. } => {}
            }
        }
        let (contract_scope, diagnostics) = contract.finish();
        self.diagnostics.extend(diagnostics);
        self.contracts.push(contract_scope);
    }

    fn add_import_modules(
        &mut self,
        path: &[SpannedElem<'db, Ident<'db>>],
        alias: Option<SpannedElem<'db, Ident<'db>>>,
    ) {
        if path.is_empty() {
            return;
        }
        if let Some(alias) = alias {
            self.add_module(ident_text(self.db, &alias).to_owned(), alias.span(self.db));
            return;
        }
        let full = path
            .iter()
            .map(|segment| ident_text(self.db, segment))
            .collect::<Vec<_>>()
            .join(".");
        let leaf = path.last().expect("non-empty path");
        self.add_module(ident_text(self.db, leaf).to_owned(), leaf.span(self.db));
        if full != ident_text(self.db, leaf) {
            self.add_module(full, path_span(self.db, path));
        }
    }

    fn add_module(&mut self, name: String, span: Span<'db>) {
        if self.modules.iter().any(|entry| entry.name == name) {
            return;
        }
        self.modules.push(ScopeEntry {
            name: name.clone(),
            span,
            resolution: Resolution::Module(ModuleRef {
                owner: self.module.def_id_value(self.db),
                name,
            }),
        });
    }

    fn check_type_duplicate(&mut self, name: &str, span: Span<'db>, family: TypeDeclFamily) {
        let previous = self.type_names.entry(name.to_owned()).or_default();
        if let Some((_, previous_span)) = previous
            .iter()
            .find(|(previous_family, _)| !type_decl_families_can_share(*previous_family, family))
        {
            self.diagnostics.push(duplicate_diagnostic(
                self.db,
                Namespace::Type,
                name,
                span,
                *previous_span,
                None,
            ));
        }
        previous.push((family, span));
    }

    fn check_duplicate(
        &mut self,
        namespace: Namespace,
        name: &str,
        span: Span<'db>,
        context: Option<&str>,
    ) {
        let map = match namespace {
            Namespace::Term => &mut self.term_names,
            Namespace::Type | Namespace::Field | Namespace::Module => return,
        };
        if let Some(previous) = map.get(name).copied() {
            self.diagnostics.push(duplicate_diagnostic(
                self.db, namespace, name, span, previous, context,
            ));
        } else {
            map.insert(name.to_owned(), span);
        }
    }
}

fn type_decl_families_can_share(left: TypeDeclFamily, right: TypeDeclFamily) -> bool {
    matches!(
        (left, right),
        (TypeDeclFamily::Adt, TypeDeclFamily::Contract)
            | (TypeDeclFamily::Contract, TypeDeclFamily::Adt)
    )
}

struct ContractScopeBuilder<'db> {
    db: &'db dyn Db,
    contract: DefId<'db>,
    name: String,
    types: Vec<ScopeEntry<'db>>,
    terms: Vec<ScopeEntry<'db>>,
    fields: Vec<FieldEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    type_names: FxHashMap<String, Span<'db>>,
    term_names: FxHashMap<String, Span<'db>>,
    diagnostics: Vec<NameresDiagnostic>,
}

impl<'db> ContractScopeBuilder<'db> {
    fn new(db: &'db dyn Db, contract: DefId<'db>, name: String) -> Self {
        Self {
            db,
            contract,
            name,
            types: Vec::new(),
            terms: Vec::new(),
            fields: Vec::new(),
            ctor_lists: Vec::new(),
            type_names: FxHashMap::default(),
            term_names: FxHashMap::default(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self) -> (ContractScope<'db>, Vec<NameresDiagnostic>) {
        (
            ContractScope {
                contract: self.contract,
                name: self.name,
                types: self.types,
                terms: self.terms,
                fields: self.fields,
                ctor_lists: self.ctor_lists,
            },
            self.diagnostics,
        )
    }

    fn add_type(&mut self, name: String, span: Span<'db>, resolution: Resolution<'db>) {
        self.check_duplicate(Namespace::Type, &name, span);
        self.types.push(ScopeEntry {
            name,
            span,
            resolution,
        });
    }

    fn add_term(
        &mut self,
        name: String,
        span: Span<'db>,
        resolution: Resolution<'db>,
        check_duplicate: bool,
    ) {
        if check_duplicate {
            self.check_duplicate(Namespace::Term, &name, span);
        }
        self.terms.push(ScopeEntry {
            name,
            span,
            resolution,
        });
    }

    fn add_field(&mut self, field: &FieldDef<'db>, index: u32) {
        self.fields.push(FieldEntry {
            name: ident_text(self.db, field.name()).to_owned(),
            span: field.name().span(self.db),
            field: FieldId {
                contract: self.contract,
                index,
            },
        });
    }

    fn check_duplicate(&mut self, namespace: Namespace, name: &str, span: Span<'db>) {
        let map = match namespace {
            Namespace::Type => &mut self.type_names,
            Namespace::Term => &mut self.term_names,
            Namespace::Field | Namespace::Module => return,
        };
        if let Some(previous) = map.get(name).copied() {
            let context = format!("contract {}", self.name);
            self.diagnostics.push(duplicate_diagnostic(
                self.db,
                namespace,
                name,
                span,
                previous,
                Some(&context),
            ));
        } else {
            map.insert(name.to_owned(), span);
        }
    }
}

struct TypeResolver<'db, 'a> {
    db: &'db dyn Db,
    scope: &'a ItemScope<'db>,
    imports: &'a dyn ImportedNames<'db>,
    contract: Option<DefId<'db>>,
    type_vars: Vec<TypeVarBinding<'db>>,
    seen_types: FxHashSet<TypeRef<'db>>,
    seen_preds: FxHashSet<PredRef<'db>>,
    map: ItemResolutionMap<'db>,
}

impl<'db, 'a> TypeResolver<'db, 'a> {
    fn new(
        db: &'db dyn Db,
        scope: &'a ItemScope<'db>,
        imports: &'a dyn ImportedNames<'db>,
    ) -> Self {
        Self {
            db,
            scope,
            imports,
            contract: None,
            type_vars: Vec::new(),
            seen_types: FxHashSet::default(),
            seen_preds: FxHashSet::default(),
            map: ItemResolutionMap::default(),
        }
    }

    fn item(
        &mut self,
        item: Item<'db>,
        contract: Option<ContractDef<'db>>,
        inherited_type_vars: &[TypeVarBinding<'db>],
    ) {
        let old_contract = self.contract;
        if let Some(contract) = contract {
            self.contract = Some(contract.def_id_value(self.db));
        }
        let old_len = self.type_vars.len();
        self.type_vars.extend_from_slice(inherited_type_vars);
        match item {
            Item::FunctionDef(def) => self.function(def),
            Item::TypeAlias(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                    |this| {
                        this.ty(def.ty(this.db));
                    },
                );
            }
            Item::AdtDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                    |this| {
                        for ctor in def.ctors(this.db) {
                            this.ty(*ctor.fields.atom());
                        }
                    },
                );
            }
            Item::ClassDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.type_var_elems(self.db),
                    |this| {
                        for pred in def.super_preds(this.db) {
                            this.pred(*pred);
                        }
                        this.pred(def.head(this.db));
                        for method in def.methods(this.db) {
                            this.sig(method);
                        }
                    },
                );
            }
            Item::InstanceDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.type_var_elems(self.db),
                    |this| {
                        for pred in def.preds(this.db) {
                            this.pred(*pred);
                        }
                        this.pred(def.head(this.db));
                        for method in def.methods(this.db) {
                            this.function(*method);
                        }
                    },
                );
            }
            Item::ContractDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                    |this| {
                        for field in def.fields(this.db) {
                            this.ty(field.ty());
                        }
                        for item in def.items(this.db) {
                            match *item {
                                ContractItem::FunctionDef(defn) => {
                                    this.item(Item::FunctionDef(defn), Some(def), &[])
                                }
                                ContractItem::TypeAlias(defn) => {
                                    this.item(Item::TypeAlias(defn), Some(def), &[])
                                }
                                ContractItem::AdtDef(defn) => {
                                    this.item(Item::AdtDef(defn), Some(def), &[])
                                }
                                ContractItem::Error { .. } => {}
                            }
                        }
                    },
                );
            }
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
        self.type_vars.truncate(old_len);
        self.contract = old_contract;
    }

    fn function(&mut self, def: FunctionDef<'db>) {
        let sig = def.sig(self.db);
        self.with_item_type_vars(def.def_id_value(self.db), &sig.type_vars, |this| {
            this.sig(sig)
        });
    }

    fn sig(&mut self, sig: &FuncSig<'db>) {
        for pred in &sig.preds {
            self.pred(*pred);
        }
        for param in sig.params.atom() {
            self.param(param);
        }
        if let Some(ret) = sig.ret {
            self.ty(ret);
        }
    }

    fn param(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.ty(*ty);
        }
    }

    fn pred(&mut self, pred: PredRef<'db>) {
        if !self.seen_preds.insert(pred) {
            return;
        }
        let kind = pred.kind(self.db);
        self.ty(kind.ty);
        for arg in kind.args.atom() {
            self.ty(*arg);
        }
        let name = ident_text(self.db, &kind.class);
        let resolution = self.lookup_class(name).unwrap_or_else(|| {
            self.map
                .diagnostics
                .push(undefined_class(self.db, name, kind.class.span(self.db)));
            Resolution::Err
        });
        self.map.preds.push(PredResolution { pred, resolution });
    }

    fn ty(&mut self, ty: TypeRef<'db>) {
        if !self.seen_types.insert(ty) {
            return;
        }
        match ty.kind(self.db) {
            TypeRefKind::Named {
                qualifier,
                name,
                args,
            } => {
                for arg in args.atom() {
                    self.ty(*arg);
                }
                let resolution = if let Some(qualifier) = qualifier {
                    let qualifier_text = ident_text(self.db, qualifier);
                    let qualified = qualify(qualifier_text, ident_text(self.db, name));
                    self.lookup_type(&qualified).unwrap_or_else(|| {
                        if self
                            .imports
                            .has_incomplete_module_qualifier(self.db, qualifier_text)
                        {
                            return Resolution::Err;
                        }
                        self.map.diagnostics.push(undefined_type_ctor(
                            self.db,
                            &qualified,
                            name.span(self.db),
                        ));
                        Resolution::Err
                    })
                } else {
                    let name_text = ident_text(self.db, name);
                    self.lookup_type(name_text).unwrap_or_else(|| {
                        self.map.diagnostics.push(undefined_type_ctor(
                            self.db,
                            name_text,
                            name.span(self.db),
                        ));
                        Resolution::Err
                    })
                };
                self.map.types.push(TypeResolution { ty, resolution });
            }
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.ty(*param);
                }
                self.ty(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.ty(*inner),
            TypeRefKind::Tuple { elems } => {
                for elem in elems.atom() {
                    self.ty(*elem);
                }
            }
            TypeRefKind::Error { .. } => {
                self.map.types.push(TypeResolution {
                    ty,
                    resolution: Resolution::Err,
                });
            }
        }
    }

    fn with_item_type_vars(
        &mut self,
        owner: DefId<'db>,
        vars: &[SpannedElem<'db, Ident<'db>>],
        f: impl FnOnce(&mut Self),
    ) {
        let old_len = self.type_vars.len();
        self.type_vars
            .extend(type_var_bindings(self.db, owner, vars));
        f(self);
        self.type_vars.truncate(old_len);
    }

    fn lookup_type(&self, name: &str) -> Option<Resolution<'db>> {
        self.type_vars
            .iter()
            .rev()
            .find(|var| ident_text(self.db, &var.name) == name)
            .map(|var| {
                Resolution::Local(LocalBinding::TypeVar(TypeVarId {
                    owner: var.owner,
                    index: var.index,
                    name: name.to_owned(),
                }))
            })
            .or_else(|| {
                self.contract
                    .and_then(|contract| self.scope.contract_scope(contract))
                    .and_then(|contract| contract.type_resolution(name))
            })
            .or_else(|| self.scope.type_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Type, name))
            .or_else(|| builtin_type_or_class(name))
            .or_else(|| {
                self.imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Type, name)
                    .then_some(Resolution::Err)
            })
    }

    fn lookup_class(&self, name: &str) -> Option<Resolution<'db>> {
        match self.lookup_type(name) {
            Some(
                res @ Resolution::Def {
                    kind: DefResolutionKind::Class,
                    ..
                },
            )
            | Some(res @ Resolution::Builtin(BuiltinKind::Class(_)))
            | Some(res @ Resolution::Err) => Some(res),
            Some(_) | None => None,
        }
    }
}

struct BodyResolver<'db, 'a> {
    db: &'db dyn Db,
    scope: &'a ItemScope<'db>,
    imports: &'a dyn ImportedNames<'db>,
    contract: Option<DefId<'db>>,
    local_scopes: Vec<FxHashMap<String, Resolution<'db>>>,
    type_vars: Vec<TypeVarBinding<'db>>,
    map: BodyResolutionMap<'db>,
}

impl<'db, 'a> BodyResolver<'db, 'a> {
    fn new(
        db: &'db dyn Db,
        scope: &'a ItemScope<'db>,
        imports: &'a dyn ImportedNames<'db>,
        contract: Option<DefId<'db>>,
    ) -> Self {
        Self {
            db,
            scope,
            imports,
            contract,
            local_scopes: Vec::new(),
            type_vars: Vec::new(),
            map: BodyResolutionMap::default(),
        }
    }

    fn body(&mut self, body: FuncBody<'db>) {
        for stmt in body.top_level_stmts(self.db) {
            self.stmt(body, *stmt);
        }
    }

    fn stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) {
        let stmt = body.stmts(self.db).get(stmt_id);
        match &stmt.kind {
            StmtKind::Let { name, ty, init, .. } => {
                if let Some(ty) = ty {
                    self.ty(*ty);
                }
                if let Some(init) = init {
                    // Reference semantics: a let initializer is evaluated in
                    // the pre-binder scope, so the new local is inserted after
                    // the initializer has been resolved.
                    self.expr(body, *init);
                }
                let resolution = Resolution::Local(LocalBinding::Let {
                    body,
                    stmt: stmt_id,
                });
                self.add_local(ident_text(self.db, name), resolution.clone());
                self.map.record_stmt(body, stmt_id, resolution);
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(body, *expr);
                }
            }
            StmtKind::Expr(expr) => self.expr(body, *expr),
            StmtKind::Assign { lhs, rhs }
            | StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                self.expr(body, *lhs);
                self.expr(body, *rhs);
            }
            StmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.expr(body, *scrutinee);
                }
                for arm in arms {
                    self.match_arm(body, arm);
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                // `for` does not create a lexical scope; initializer, condition,
                // post statements, and body share the surrounding scope.
                for stmt in init {
                    self.stmt(body, *stmt);
                }
                self.expr(body, *cond);
                for stmt in post {
                    self.stmt(body, *stmt);
                }
                for stmt in for_body {
                    self.stmt(body, *stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.expr(body, *cond);
                for stmt in then_body {
                    self.stmt(body, *stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.stmt(body, *stmt);
                    }
                }
            }
            StmtKind::Block { body: block } => {
                self.with_scope(|resolver| {
                    for stmt in block {
                        resolver.stmt(body, *stmt);
                    }
                });
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn match_arm(&mut self, body: FuncBody<'db>, arm: &MatchArm<'db>) {
        self.with_scope(|resolver| {
            for pat in &arm.pats {
                resolver.pat(body, *pat);
            }
            for stmt in &arm.body {
                resolver.stmt(body, *stmt);
            }
        });
    }

    fn expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Lit(_) => {}
            ExprKind::Error => {
                self.map.record_expr(body, expr_id, Resolution::Err);
            }
            ExprKind::Ident(name) => {
                let resolution = self.resolve_ident(name);
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::DotCtor { name, args, .. } => {
                for arg in args {
                    self.expr(body, *arg);
                }
                let leaf = ident_text(self.db, name);
                let resolution = if self.has_constructor_leaf(leaf) {
                    Resolution::DotCtorDeferred
                } else if self.imports.may_contain_unknown_unqualified(
                    self.db,
                    Namespace::Term,
                    leaf,
                ) {
                    Resolution::Err
                } else {
                    self.map
                        .diagnostics
                        .push(undefined_name(self.db, leaf, name.span(self.db)));
                    Resolution::Err
                };
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::Proxy { ty, .. } => self.ty(*ty),
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => {
                for param in params.atom() {
                    self.param_type(param);
                }
                if let Some(ret) = ret {
                    self.ty(*ret);
                }
                self.with_scope(|resolver| {
                    for (index, param) in params.atom().iter().enumerate() {
                        if let Some(name) = param_name(param) {
                            resolver.add_param(*lambda_body, index as u32, name);
                        }
                    }
                    resolver.body(*lambda_body);
                });
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(body, *lhs);
                self.expr(body, *rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(body, *base);
                self.expr(body, *index);
            }
            ExprKind::Call { callee, args } => {
                self.call_callee(body, *callee);
                for arg in args {
                    self.expr(body, *arg);
                }
            }
            ExprKind::Field { base, field } => {
                if self.is_namespace_qualifier(body, *base) {
                    self.expr_as_qualifier(body, *base);
                } else {
                    self.expr(body, *base);
                }
                if let Some(resolution) = self.resolve_field_expr(body, *base, field) {
                    self.map.record_expr(body, expr_id, resolution);
                }
            }
            ExprKind::TypeAnnot { expr, ty } => {
                self.expr(body, *expr);
                self.ty(*ty);
            }
            ExprKind::UnaryOp { expr, .. } => self.expr(body, *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(body, *cond);
                self.expr(body, *then_expr);
                self.expr(body, *else_expr);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.expr(body, *elem);
                }
            }
        }
    }

    fn pat(&mut self, body: FuncBody<'db>, pat_id: Id<Pat<'db>>) {
        let pat = body.pats(self.db).get(pat_id);
        match &pat.kind {
            PatKind::Wildcard | PatKind::Lit(_) => {}
            PatKind::Error => {
                self.map.record_pat(body, pat_id, Resolution::Err);
            }
            PatKind::Var(name) => {
                let leaf = ident_text(self.db, name);
                let resolution = if let Some(
                    res @ Resolution::Builtin(BuiltinKind::Constructor(
                        BuiltinCtor::True | BuiltinCtor::False,
                    )),
                ) = builtin_term(leaf)
                {
                    res
                } else if let Some(res) = self.same_name_constructor_resolution(leaf) {
                    // A constructor sharing its type's name may be referenced
                    // without a qualifier, mirroring the reference resolver.
                    res
                } else if self.has_user_constructor_leaf(leaf) {
                    // Any other in-scope constructor must be written qualified;
                    // silently binding it as a variable would turn the arm into
                    // a catch-all.
                    self.map.diagnostics.push(unqualified_constructor(
                        self.db,
                        leaf,
                        name.span(self.db),
                    ));
                    Resolution::Err
                } else {
                    let resolution = Resolution::Local(LocalBinding::Pattern { body, pat: pat_id });
                    self.add_local(leaf, resolution.clone());
                    resolution
                };
                self.map.record_pat(body, pat_id, resolution);
            }
            PatKind::Ctor {
                leading_dot,
                qualifier,
                name,
                args,
            } => {
                for arg in args {
                    self.pat(body, *arg);
                }
                let resolution = if leading_dot.is_some() {
                    Resolution::DotCtorDeferred
                } else if let Some(qualifier) = qualifier {
                    let qualifier_text = ident_text(self.db, qualifier);
                    let qualified = qualify(qualifier_text, ident_text(self.db, name));
                    self.lookup_ctor(&qualified).unwrap_or_else(|| {
                        if self
                            .imports
                            .has_incomplete_module_qualifier(self.db, qualifier_text)
                        {
                            return Resolution::Err;
                        }
                        self.map.diagnostics.push(undefined_name(
                            self.db,
                            &qualified,
                            name.span(self.db),
                        ));
                        Resolution::Err
                    })
                } else {
                    let leaf = ident_text(self.db, name);
                    if self
                        .imports
                        .may_contain_unknown_unqualified(self.db, Namespace::Term, leaf)
                    {
                        Resolution::Err
                    } else if self.has_constructor_leaf(leaf) {
                        self.same_name_constructor_resolution(leaf).unwrap_or_else(|| {
                            if matches!(
                                builtin_term(leaf),
                                Some(Resolution::Builtin(BuiltinKind::Constructor(_)))
                            ) {
                                // Primitive constructors (`pair`, `inl`, ...) stay
                                // legal unqualified; their concrete constructor is
                                // picked from the expected type during inference.
                                Resolution::DotCtorDeferred
                            } else {
                                self.map.diagnostics.push(unqualified_constructor(
                                    self.db,
                                    leaf,
                                    name.span(self.db),
                                ));
                                Resolution::Err
                            }
                        })
                    } else if args.is_empty() {
                        let resolution =
                            Resolution::Local(LocalBinding::Pattern { body, pat: pat_id });
                        self.add_local(leaf, resolution.clone());
                        resolution
                    } else {
                        self.map
                            .diagnostics
                            .push(invalid_pattern(self.db, pat.span));
                        Resolution::Err
                    }
                };
                self.map.record_pat(body, pat_id, resolution);
            }
            PatKind::ComptimeLabel { expr, .. } => self.expr(body, *expr),
            PatKind::Tuple { elems } => {
                for elem in elems {
                    self.pat(body, *elem);
                }
            }
        }
    }

    fn ty(&mut self, ty: TypeRef<'db>) {
        match ty.kind(self.db) {
            TypeRefKind::Named {
                qualifier,
                name,
                args,
            } => {
                for arg in args.atom() {
                    self.ty(*arg);
                }
                let resolution = if let Some(qualifier) = qualifier {
                    let qualifier_text = ident_text(self.db, qualifier);
                    let qualified = qualify(qualifier_text, ident_text(self.db, name));
                    self.lookup_type(&qualified).unwrap_or_else(|| {
                        if self
                            .imports
                            .has_incomplete_module_qualifier(self.db, qualifier_text)
                        {
                            return Resolution::Err;
                        }
                        self.map.diagnostics.push(undefined_type_ctor(
                            self.db,
                            &qualified,
                            name.span(self.db),
                        ));
                        Resolution::Err
                    })
                } else {
                    let name_text = ident_text(self.db, name);
                    self.lookup_type(name_text).unwrap_or_else(|| {
                        self.map.diagnostics.push(undefined_type_ctor(
                            self.db,
                            name_text,
                            name.span(self.db),
                        ));
                        Resolution::Err
                    })
                };
                self.map.types.push(TypeResolution { ty, resolution });
            }
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.ty(*param);
                }
                self.ty(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.ty(*inner),
            TypeRefKind::Tuple { elems } => {
                for elem in elems.atom() {
                    self.ty(*elem);
                }
            }
            TypeRefKind::Error { .. } => {
                self.map.types.push(TypeResolution {
                    ty,
                    resolution: Resolution::Err,
                });
            }
        }
    }

    fn param_type(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.ty(*ty);
        }
    }

    fn resolve_ident(&mut self, name: &SpannedElem<'db, Ident<'db>>) -> Resolution<'db> {
        let text = ident_text(self.db, name);
        self.lookup_local(text)
            // Contract fields intentionally beat same-name functions in the
            // contract term surface.
            .or_else(|| self.lookup_field(text))
            .or_else(|| self.lookup_qualified_term(text))
            .or_else(|| self.lookup_unqualified_class_method(text))
            .or_else(|| {
                self.imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Term, text)
                    .then_some(Resolution::Err)
            })
            .or_else(|| self.same_name_constructor_resolution(text))
            .or_else(|| self.lookup_type(text))
            .or_else(|| self.lookup_module(text))
            .unwrap_or_else(|| {
                if self
                    .imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Term, text)
                {
                    return Resolution::Err;
                }
                if self.has_user_constructor_leaf(text) {
                    // The name is visible only as a constructor of some type;
                    // referencing it without its type qualifier is an error.
                    self.map.diagnostics.push(unqualified_constructor(
                        self.db,
                        text,
                        name.span(self.db),
                    ));
                    return Resolution::Err;
                }
                self.map
                    .diagnostics
                    .push(undefined_name(self.db, text, name.span(self.db)));
                Resolution::Err
            })
    }

    fn call_callee(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Ident(name) => {
                let resolution = self.resolve_call_ident(name);
                self.map.record_expr(body, expr_id, resolution);
            }
            _ => self.expr(body, expr_id),
        }
    }

    fn resolve_call_ident(&mut self, name: &SpannedElem<'db, Ident<'db>>) -> Resolution<'db> {
        let text = ident_text(self.db, name);
        self.lookup_local(text)
            .or_else(|| self.lookup_qualified_term(text))
            .or_else(|| self.lookup_field(text))
            .or_else(|| self.lookup_unqualified_class_method(text))
            .or_else(|| self.same_name_constructor_resolution(text))
            .unwrap_or_else(|| self.resolve_ident(name))
    }

    fn expr_as_qualifier(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Ident(name) => {
                let text = ident_text(self.db, name);
                let resolution = self
                    .lookup_type(text)
                    .or_else(|| self.lookup_module(text))
                    .or_else(|| self.lookup_qualified_term(text))
                    .unwrap_or_else(|| {
                        if self.imports.may_contain_unknown_unqualified(
                            self.db,
                            Namespace::Module,
                            text,
                        ) {
                            return Resolution::Err;
                        }
                        self.map.diagnostics.push(undefined_name(
                            self.db,
                            text,
                            name.span(self.db),
                        ));
                        Resolution::Err
                    });
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::Field { base, field } => {
                self.expr_as_qualifier(body, *base);
                if let Some(resolution) = self.resolve_field_expr(body, *base, field) {
                    self.map.record_expr(body, expr_id, resolution);
                }
            }
            _ => self.expr(body, expr_id),
        }
    }

    fn resolve_field_expr(
        &mut self,
        body: FuncBody<'db>,
        base: Id<Expr<'db>>,
        field: &SpannedElem<'db, Ident<'db>>,
    ) -> Option<Resolution<'db>> {
        let path = expr_path(self.db, body, base)?;
        let qualifier = path.join(".");
        let field_text = ident_text(self.db, field);
        let qualified = qualify(&qualifier, field_text);

        if let Some(resolution) = self.lookup_qualified_term(&qualified) {
            return Some(resolution);
        }

        if let Some(resolution) = self.lookup_type(&qualified) {
            return Some(resolution);
        }

        if matches!(
            self.lookup_type(&qualifier),
            Some(
                Resolution::Def {
                    kind: DefResolutionKind::Adt
                        | DefResolutionKind::Contract
                        | DefResolutionKind::Class
                        | DefResolutionKind::TypeAlias,
                    ..
                } | Resolution::Builtin(BuiltinKind::Type(_) | BuiltinKind::Class(_))
            )
        ) {
            self.map
                .diagnostics
                .push(undefined_name(self.db, field_text, field.span(self.db)));
            return Some(Resolution::Err);
        }

        if self.lookup_module(&qualifier).is_some() {
            if self.lookup_module(&qualified).is_none() {
                if self
                    .imports
                    .has_incomplete_module_qualifier(self.db, &qualifier)
                {
                    return Some(Resolution::Err);
                }
                self.map
                    .diagnostics
                    .push(undefined_name(self.db, field_text, field.span(self.db)));
                return Some(Resolution::Err);
            }
            return Some(Resolution::Module(ModuleRef {
                owner: self.scope.module.def_id_value(self.db),
                name: qualified,
            }));
        }

        None
    }

    fn lookup_qualified_term(&self, name: &str) -> Option<Resolution<'db>> {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .and_then(|contract| contract.term_resolution(name))
            .or_else(|| self.scope.term_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Term, name))
            .or_else(|| builtin_term(name))
    }

    fn lookup_unqualified_class_method(&self, name: &str) -> Option<Resolution<'db>> {
        let mut matches = self
            .scope
            .terms
            .iter()
            .filter(|entry| entry.name.rsplit('.').next() == Some(name))
            .filter_map(|entry| match &entry.resolution {
                Resolution::ClassMethod { .. } => Some(entry.resolution.clone()),
                _ => None,
            });
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }

    fn lookup_ctor(&self, name: &str) -> Option<Resolution<'db>> {
        match self.lookup_qualified_term(name) {
            Some(res @ Resolution::Ctor { .. })
            | Some(res @ Resolution::Builtin(BuiltinKind::Constructor(_))) => Some(res),
            _ => None,
        }
    }

    fn lookup_local(&self, name: &str) -> Option<Resolution<'db>> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn lookup_field(&self, name: &str) -> Option<Resolution<'db>> {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .and_then(|contract| contract.field_resolution(name))
    }

    fn lookup_type(&self, name: &str) -> Option<Resolution<'db>> {
        self.type_vars
            .iter()
            .rev()
            .find(|var| ident_text(self.db, &var.name) == name)
            .map(|var| {
                Resolution::Local(LocalBinding::TypeVar(TypeVarId {
                    owner: var.owner,
                    index: var.index,
                    name: name.to_owned(),
                }))
            })
            .or_else(|| {
                self.contract
                    .and_then(|contract| self.scope.contract_scope(contract))
                    .and_then(|contract| contract.type_resolution(name))
            })
            .or_else(|| self.scope.type_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Type, name))
            .or_else(|| builtin_type_or_class(name))
            .or_else(|| {
                self.imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Type, name)
                    .then_some(Resolution::Err)
            })
    }

    fn lookup_module(&self, name: &str) -> Option<Resolution<'db>> {
        self.scope
            .module_resolution(name)
            .or_else(|| self.imports.imported(self.db, Namespace::Module, name))
    }

    fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.has_user_constructor_leaf(leaf)
            || matches!(
                builtin_term(leaf),
                Some(Resolution::Builtin(BuiltinKind::Constructor(_)))
            )
    }

    /// Returns whether any user-declared constructor in scope has this leaf
    /// name, excluding the builtin (primitive) constructors.
    ///
    /// Unqualified references to such constructors are rejected with `SC0106`,
    /// while primitive constructors stay legal unqualified.
    fn has_user_constructor_leaf(&self, leaf: &str) -> bool {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .is_some_and(|contract| contract.has_constructor_leaf(leaf))
            || self.scope.has_constructor_leaf(leaf)
            || self.imports.has_constructor_leaf(self.db, leaf)
    }

    fn same_name_constructor_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.lookup_ctor(&qualify(name, name))
    }

    fn is_namespace_qualifier(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        let Some(path) = expr_path(self.db, body, expr) else {
            return false;
        };
        let Some(first) = path.first() else {
            return false;
        };
        if path.len() == 1
            && (self.lookup_local(first).is_some() || self.lookup_field(first).is_some())
        {
            return false;
        }
        self.lookup_type(first).is_some() || self.lookup_module(first).is_some()
    }

    fn add_local(&mut self, name: &str, resolution: Resolution<'db>) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name.to_owned(), resolution);
        } else {
            let mut scope = FxHashMap::default();
            scope.insert(name.to_owned(), resolution);
            self.local_scopes.push(scope);
        }
    }

    fn add_param(&mut self, body: FuncBody<'db>, index: u32, name: &SpannedElem<'db, Ident<'db>>) {
        self.add_local(
            ident_text(self.db, name),
            Resolution::Param(ParamId { body, index }),
        );
    }

    fn with_scope(&mut self, f: impl FnOnce(&mut Self)) {
        self.local_scopes.push(FxHashMap::default());
        f(self);
        self.local_scopes.pop();
    }

    fn with_type_vars(&mut self, vars: &[TypeVarBinding<'db>], f: impl FnOnce(&mut Self)) {
        let old_len = self.type_vars.len();
        self.type_vars.extend_from_slice(vars);
        f(self);
        self.type_vars.truncate(old_len);
    }
}

fn ident_text<'db>(db: &'db dyn Db, ident: &SpannedElem<'db, Ident<'db>>) -> &'db str {
    (*ident.atom()).text(db)
}

fn qualify(qualifier: &str, name: &str) -> String {
    format!("{qualifier}.{name}")
}

fn path_span<'db>(db: &'db dyn Db, path: &[SpannedElem<'db, Ident<'db>>]) -> Span<'db> {
    let first = path.first().expect("non-empty path");
    let last = path.last().expect("non-empty path");
    first.span(db) + last.span(db)
}

fn expr_path<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> Option<Vec<String>> {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Ident(name) => Some(vec![ident_text(db, name).to_owned()]),
        ExprKind::Field { base, field } => {
            let mut path = expr_path(db, body, *base)?;
            path.push(ident_text(db, field).to_owned());
            Some(path)
        }
        _ => None,
    }
}

fn param_name<'a, 'db>(param: &'a FuncParam<'db>) -> Option<&'a SpannedElem<'db, Ident<'db>>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name),
        FuncParam::Error { .. } => None,
    }
}

fn param_bindings<'db>(params: &[FuncParam<'db>]) -> Vec<ParamBinding<'db>> {
    params
        .iter()
        .filter_map(param_name)
        .map(|name| ParamBinding { name: *name })
        .collect()
}

fn type_var_bindings<'db>(
    _db: &'db dyn Db,
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

fn builtin_type_or_class<'db>(name: &str) -> Option<Resolution<'db>> {
    let kind = match name {
        "word" | "Word" => BuiltinKind::Type(BuiltinType::Word),
        "bool" => BuiltinKind::Type(BuiltinType::Bool),
        "string" => BuiltinKind::Type(BuiltinType::String),
        "()" => BuiltinKind::Type(BuiltinType::Unit),
        "pair" => BuiltinKind::Type(BuiltinType::Pair),
        "sum" => BuiltinKind::Type(BuiltinType::Sum),
        "integer" => BuiltinKind::Type(BuiltinType::Integer),
        "invokable" => BuiltinKind::Class(BuiltinClass::Invokable),
        "Int" => BuiltinKind::Class(BuiltinClass::Int),
        _ => return None,
    };
    Some(Resolution::Builtin(kind))
}

fn builtin_term<'db>(name: &str) -> Option<Resolution<'db>> {
    let kind = match name {
        "true" => BuiltinKind::Constructor(BuiltinCtor::True),
        "false" => BuiltinKind::Constructor(BuiltinCtor::False),
        "()" => BuiltinKind::Constructor(BuiltinCtor::Unit),
        "pair" => BuiltinKind::Constructor(BuiltinCtor::Pair),
        "inl" => BuiltinKind::Constructor(BuiltinCtor::Inl),
        "inr" => BuiltinKind::Constructor(BuiltinCtor::Inr),
        "invoke" => BuiltinKind::Function(BuiltinFunction::Invoke),
        "primAddWord" => BuiltinKind::Function(BuiltinFunction::PrimAddWord),
        "primEqWord" => BuiltinKind::Function(BuiltinFunction::PrimEqWord),
        "wordToInteger" => BuiltinKind::Function(BuiltinFunction::WordToInteger),
        "wordFromInteger" => BuiltinKind::Function(BuiltinFunction::WordFromInteger),
        "integerAdd" => BuiltinKind::Function(BuiltinFunction::IntegerAdd),
        "integerSub" => BuiltinKind::Function(BuiltinFunction::IntegerSub),
        "integerMul" => BuiltinKind::Function(BuiltinFunction::IntegerMul),
        "integerLt" => BuiltinKind::Function(BuiltinFunction::IntegerLt),
        "integerEq" => BuiltinKind::Function(BuiltinFunction::IntegerEq),
        "invokable.invoke" => BuiltinKind::ClassMethod(BuiltinClassMethod::InvokableInvoke),
        "Int.fromInteger" => BuiltinKind::ClassMethod(BuiltinClassMethod::IntFromInteger),
        _ => return None,
    };
    Some(Resolution::Builtin(kind))
}

fn duplicate_diagnostic<'db>(
    db: &'db dyn Db,
    namespace: Namespace,
    name: &str,
    span: Span<'db>,
    previous: Span<'db>,
    context: Option<&str>,
) -> NameresDiagnostic {
    NameresDiagnostic::DuplicateDeclaration {
        namespace,
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        previous: LabelSpan::from_span(db, previous),
        context: context.map(ToOwned::to_owned),
    }
}

fn undefined_name<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) -> NameresDiagnostic {
    NameresDiagnostic::UndefinedName {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

fn undefined_type_ctor<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) -> NameresDiagnostic {
    NameresDiagnostic::UndefinedTypeConstructor {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

fn undefined_class<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) -> NameresDiagnostic {
    NameresDiagnostic::UndefinedClass {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

fn invalid_pattern<'db>(db: &'db dyn Db, span: Span<'db>) -> NameresDiagnostic {
    NameresDiagnostic::InvalidPattern {
        span: LabelSpan::from_span(db, span),
    }
}

fn unqualified_constructor<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) -> NameresDiagnostic {
    NameresDiagnostic::UnqualifiedConstructor {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}
