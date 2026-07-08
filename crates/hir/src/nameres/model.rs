use super::*;

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

/// Visible candidate for a constructor leaf.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ConstructorTypeCandidate {
    /// Type that owns the constructor.
    pub ty_name: String,
    /// Constructor leaf name.
    pub ctor_name: String,
    /// Span of the constructor declaration.
    pub span: LabelSpan,
}

/// Private imported item found while resolving a qualified module access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PrivateCandidate {
    /// Private item name.
    pub name: String,
    /// Module that declares the private item.
    pub module: String,
    /// Span of the private declaration.
    pub span: LabelSpan,
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
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldIndex(u32);

impl FieldIndex {
    pub const fn from_u32(v: u32) -> Self {
        Self(v)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub fn from_usize(v: usize) -> Self {
        Self(v as u32)
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldId<'db> {
    /// Owning contract definition.
    pub contract: DefId<'db>,
    /// Zero-based field declaration index.
    pub index: FieldIndex,
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
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParamIndex(u32);

impl ParamIndex {
    pub const fn from_u32(v: u32) -> Self {
        Self(v)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub fn from_usize(v: usize) -> Self {
        Self(v as u32)
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct CtorIndex(u32);

impl CtorIndex {
    pub const fn from_u32(v: u32) -> Self {
        Self(v)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub fn from_usize(v: usize) -> Self {
        Self(v as u32)
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParamId<'db> {
    /// Body whose parameter list introduced this parameter.
    pub body: FuncBody<'db>,
    /// Zero-based parameter index.
    pub index: ParamIndex,
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
        index: CtorIndex,
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

/// Ordered namespace entries with an indexed first-name lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct NamespaceTable<'db> {
    entries: Vec<ScopeEntry<'db>>,
    index: std::collections::BTreeMap<String, u32>,
}

impl<'db> NamespaceTable<'db> {
    /// Appends `entry` and records the first entry for its name.
    pub fn push(&mut self, entry: ScopeEntry<'db>) {
        let index =
            u32::try_from(self.entries.len()).expect("namespace table entry count exceeds u32");
        self.index.entry(entry.name.clone()).or_insert(index);
        self.entries.push(entry);
    }

    /// Returns the first entry for `name`.
    pub fn get(&self, name: &str) -> Option<&ScopeEntry<'db>> {
        self.index
            .get(name)
            .and_then(|index| self.entries.get(*index as usize))
    }

    /// Iterates entries in insertion order.
    pub fn iter(&self) -> std::slice::Iter<'_, ScopeEntry<'db>> {
        self.entries.iter()
    }
}

impl<'a, 'db> IntoIterator for &'a NamespaceTable<'db> {
    type Item = &'a ScopeEntry<'db>;
    type IntoIter = std::slice::Iter<'a, ScopeEntry<'db>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
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
    pub index: CtorIndex,
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
    pub types: NamespaceTable<'db>,
    /// Contract-local term entries.
    pub terms: NamespaceTable<'db>,
    /// Field entries.
    pub fields: Vec<FieldEntry<'db>>,
    /// Constructor lists declared inside the contract.
    pub ctor_lists: Vec<CtorList<'db>>,
}

/// Diagnostic side of an item-level scope.
pub type ItemScopeDiagnostics = Vec<NameresDiagnostic>;

/// Item-level lookup facts for one module.
///
/// The scope records declarations before body resolution so functions can refer
/// to later items in the same module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ItemScopeFacts<'db> {
    /// Module this scope belongs to.
    pub module: Module<'db>,
    /// Type namespace entries.
    pub types: NamespaceTable<'db>,
    /// Term namespace entries.
    pub terms: NamespaceTable<'db>,
    /// Module qualifier entries introduced by imports.
    pub modules: NamespaceTable<'db>,
    /// Top-level constructor lists.
    pub ctor_lists: Vec<CtorList<'db>>,
    /// Contract-local scopes.
    pub contracts: Vec<ContractScope<'db>>,
    /// Instance definitions in source order.
    pub instances: Vec<InstanceDef<'db>>,
}

/// Item-level scope for one module.
///
/// This is the compatibility composite used by diagnostic paths. Facts-only
/// consumers should depend on [`ItemScopeFacts`] so diagnostic changes do not
/// invalidate downstream type work.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ItemScope<'db> {
    /// Lookup facts for item and body resolution.
    pub facts: ItemScopeFacts<'db>,
    /// Diagnostics found while building item scopes.
    pub diagnostics: ItemScopeDiagnostics,
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

/// Diagnostic side of item-signature resolution.
pub type ItemResolutionDiagnostics = Vec<NameresDiagnostic>;

/// Type and predicate resolution facts for item signatures.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct ItemResolutionFacts<'db> {
    /// Resolved type references.
    pub types: Vec<TypeResolution<'db>>,
    /// Resolved predicate references.
    pub preds: Vec<PredResolution<'db>>,
}

/// Type and predicate resolutions for item signatures.
///
/// This compatibility composite preserves diagnostics for callers that publish
/// nameres output. Facts-only consumers should depend on
/// [`ItemResolutionFacts`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct ItemResolutionMap<'db> {
    /// Resolution facts used by type lowering and inference.
    pub facts: ItemResolutionFacts<'db>,
    /// Diagnostics found while resolving item signatures.
    pub diagnostics: ItemResolutionDiagnostics,
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

    /// Returns imported names that are visible in `namespace`.
    fn candidate_names(&self, _db: &'db dyn Db, _namespace: Namespace) -> Vec<String> {
        Vec::new()
    }

    /// Returns visible constructor/type pairs with the given constructor leaf.
    fn constructor_type_candidates(
        &self,
        _db: &'db dyn Db,
        _leaf: &str,
    ) -> Vec<ConstructorTypeCandidate> {
        Vec::new()
    }

    /// Returns an exact private item behind a qualified module access, when the
    /// provider can prove the item exists but is not exported.
    fn private_candidate(
        &self,
        _db: &'db dyn Db,
        _namespace: Namespace,
        _qualifier: &str,
        _name: &str,
    ) -> Option<PrivateCandidate> {
        None
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

impl<'db> std::ops::Deref for ItemScope<'db> {
    type Target = ItemScopeFacts<'db>;

    fn deref(&self) -> &Self::Target {
        &self.facts
    }
}

impl<'db> std::ops::DerefMut for ItemScope<'db> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.facts
    }
}

impl<'db> std::ops::Deref for ItemResolutionMap<'db> {
    type Target = ItemResolutionFacts<'db>;

    fn deref(&self) -> &Self::Target {
        &self.facts
    }
}

impl<'db> std::ops::DerefMut for ItemResolutionMap<'db> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.facts
    }
}

impl<'db> ItemScope<'db> {
    /// Returns the lookup facts without diagnostics.
    pub fn facts(&self) -> ItemScopeFacts<'db> {
        self.facts.clone()
    }
}

impl<'db> ItemResolutionMap<'db> {
    /// Returns the resolution facts without diagnostics.
    pub fn facts(&self) -> ItemResolutionFacts<'db> {
        self.facts.clone()
    }
}

impl<'db> ItemScopeFacts<'db> {
    /// Resolves a type name declared in this module scope.
    pub fn type_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.types.get(name).map(|entry| entry.resolution.clone())
    }

    /// Resolves a term name declared in this module scope.
    pub fn term_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.terms.get(name).map(|entry| entry.resolution.clone())
    }

    /// Resolves a module qualifier name introduced by imports.
    pub fn module_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.modules.get(name).map(|entry| entry.resolution.clone())
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
    pub(super) fn type_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.types.get(name).map(|entry| entry.resolution.clone())
    }

    pub(super) fn term_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.terms.get(name).map(|entry| entry.resolution.clone())
    }

    pub(super) fn field_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.fields
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| Resolution::Field(entry.field))
    }

    pub(super) fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.ctor_lists
            .iter()
            .flat_map(|list| &list.ctors)
            .any(|ctor| ctor.name == leaf)
    }
}

impl<'db> BodyResolutionMap<'db> {
    pub(super) fn record_expr(
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

    pub(super) fn record_stmt(
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

    pub(super) fn record_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        resolution: Resolution<'db>,
    ) {
        self.pats.push(BodyPatResolution {
            body,
            pat,
            resolution,
        });
    }
}

impl<'db> ItemResolutionMap<'db> {
    pub(super) fn apply_diagnostic_policy(&mut self, policy: NameresDiagnosticPolicy) {
        if policy.suppresses_diagnostics() {
            self.diagnostics.clear();
        }
    }
}

impl<'db> BodyResolutionMap<'db> {
    pub(super) fn apply_diagnostic_policy(&mut self, policy: NameresDiagnosticPolicy) {
        if policy.suppresses_diagnostics() {
            self.diagnostics.clear();
        }
    }
}

impl<'db> ModuleResolutionMap<'db> {
    pub(super) fn apply_diagnostic_policy(&mut self, policy: NameresDiagnosticPolicy) {
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
