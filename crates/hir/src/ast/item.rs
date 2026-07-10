//! Top-level and contract-level item HIR.
//!
//! Items are the named declarations that participate in structural identity,
//! module interfaces, and name resolution. Most item definitions are Salsa
//! tracked structs keyed by a [`crate::anchor::DefId`] so later phases can
//! refer to stable identities while still reading fields incrementally.

use crate::{
    Db,
    anchor::DefId,
    arena::{Arena, Id},
    ast::{
        Ident,
        function::{Expr, FuncBody, FuncSig},
        ty::{PredRef, TypeRef},
    },
    span::{Span, Spanned, SpannedElem},
};

/// Algebraic data type declaration.
///
/// The definition introduces a type name and a set of constructors. Constructor
/// terms are resolved through the owning data type rather than as bare global
/// values.
#[salsa::tracked(debug)]
pub struct AdtDef<'db> {
    /// Stable structural identity of the data type.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Declared type name.
    #[tracked]
    pub name: SpannedElem<'db, Ident<'db>>,

    /// Type parameters in source order.
    #[tracked]
    #[returns(ref)]
    pub ty_params: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Data constructors declared for this ADT.
    #[tracked]
    #[returns(ref)]
    pub ctors: Vec<AdtCtor<'db>>,
}

impl<'db> Spanned<'db> for AdtDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        AdtDef::span(*self, db)
    }
}

impl<'db> AdtDef<'db> {
    /// Returns the stable definition identity for this ADT.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        AdtDef::def_id(*self, db)
    }

    /// Returns the ADT name with its declaration span.
    pub fn name_elem(&self, db: &'db dyn Db) -> SpannedElem<'db, Ident<'db>> {
        AdtDef::name(*self, db)
    }

    /// Returns type parameters with their binder spans.
    pub fn ty_param_elems(&self, db: &'db dyn Db) -> &Vec<SpannedElem<'db, Ident<'db>>> {
        AdtDef::ty_params(*self, db)
    }
}

/// Constructor declared by an algebraic data type.
///
/// Constructor fields are represented as a single tuple-like type reference so
/// nullary, unary, and n-ary constructors share one representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AdtCtor<'db> {
    /// Constructor name.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Constructor field type list and span.
    pub fields: SpannedElem<'db, TypeRef<'db>>,
    /// Number of fields in the source constructor parameter list.
    ///
    /// This is kept separately because the lowered type reference intentionally
    /// erases the outer tuple around a unary field. For example, `Wrap((a, b))`
    /// and `Pair(a, b)` otherwise have the same lowered field type shape.
    pub field_count: usize,
}

impl<'db> AdtCtor<'db> {
    /// Creates an ADT constructor value.
    pub fn new(
        name: SpannedElem<'db, Ident<'db>>,
        fields: SpannedElem<'db, TypeRef<'db>>,
        field_count: usize,
    ) -> Self {
        Self {
            name,
            fields,
            field_count,
        }
    }
}

impl<'db> Spanned<'db> for AdtCtor<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.name.span(db) + self.fields.span(db)
    }
}

/// Kind of callable declaration represented by [`FunctionDef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum FuncKind {
    /// Ordinary function or method declared with `function`.
    Function,
    /// Contract constructor.
    Constructor,
    /// Contract fallback function.
    Fallback,
}

/// Function, method, constructor, or fallback definition.
///
/// The signature is always present; the body is optional to allow signatures in
/// contexts that do not contain executable code.
#[salsa::tracked(debug)]
pub struct FunctionDef<'db> {
    /// Stable structural identity of the function.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the complete definition or declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Callable category.
    #[tracked]
    #[returns(copy)]
    pub kind: FuncKind,

    /// Source-level signature.
    #[tracked]
    #[returns(ref)]
    pub sig: FuncSig<'db>,

    /// Optional lowered body.
    #[tracked]
    #[returns(copy)]
    pub body: Option<FuncBody<'db>>,
}

impl<'db> Spanned<'db> for FunctionDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        FunctionDef::span(*self, db)
    }
}

impl<'db> FunctionDef<'db> {
    /// Returns the stable definition identity for this function.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        FunctionDef::def_id(*self, db)
    }
}

/// Type alias definition: `type Name(T, U) = Type`.
#[salsa::tracked(debug)]
pub struct TypeAlias<'db> {
    /// Stable structural identity of the alias.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full alias declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Alias name.
    #[tracked]
    pub name: SpannedElem<'db, Ident<'db>>,

    /// Type parameters declared by this alias.
    #[tracked]
    #[returns(ref)]
    pub ty_params: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Aliased type.
    #[tracked]
    pub ty: TypeRef<'db>,
}

impl<'db> Spanned<'db> for TypeAlias<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        TypeAlias::span(*self, db)
    }
}

impl<'db> TypeAlias<'db> {
    /// Returns the stable definition identity for this alias.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        TypeAlias::def_id(*self, db)
    }

    /// Returns the alias name with its declaration span.
    pub fn name_elem(&self, db: &'db dyn Db) -> SpannedElem<'db, Ident<'db>> {
        TypeAlias::name(*self, db)
    }

    /// Returns type parameters with their binder spans.
    pub fn ty_param_elems(&self, db: &'db dyn Db) -> &Vec<SpannedElem<'db, Ident<'db>>> {
        TypeAlias::ty_params(*self, db)
    }
}

/// Type class definition.
///
/// Classes introduce a type-namespace name and method names qualified by the
/// class during name resolution.
#[salsa::tracked(debug)]
pub struct ClassDef<'db> {
    /// Stable structural identity of the class.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full class declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Type variables introduced by the class head.
    #[tracked]
    #[returns(ref)]
    pub type_vars: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Superclass predicates.
    #[tracked]
    #[returns(ref)]
    pub super_preds: Vec<PredRef<'db>>,

    /// Class head predicate naming the class.
    #[tracked]
    pub head: PredRef<'db>,

    /// Method signatures declared by the class.
    #[tracked]
    #[returns(ref)]
    pub methods: Vec<FuncSig<'db>>,
}

impl<'db> Spanned<'db> for ClassDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        ClassDef::span(*self, db)
    }
}

impl<'db> ClassDef<'db> {
    /// Returns the stable definition identity for this class.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        ClassDef::def_id(*self, db)
    }

    /// Returns type variables with their binder spans.
    pub fn type_var_elems(&self, db: &'db dyn Db) -> &Vec<SpannedElem<'db, Ident<'db>>> {
        ClassDef::type_vars(*self, db)
    }
}

/// Type class instance definition.
///
/// Instance identity may use a structural fingerprint of its head so multiple
/// instances for the same class can remain distinct without relying on spans.
#[salsa::tracked(debug)]
pub struct InstanceDef<'db> {
    /// Stable structural identity of the instance.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full instance declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Instance type variables.
    #[tracked]
    #[returns(ref)]
    pub type_vars: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Context predicates required by the instance.
    #[tracked]
    #[returns(ref)]
    pub preds: Vec<PredRef<'db>>,

    /// Span of the optional `default` keyword.
    #[tracked]
    #[returns(copy)]
    pub default_kw: Option<Span<'db>>,

    /// Instance head predicate.
    #[tracked]
    pub head: PredRef<'db>,

    /// Method implementations declared in the instance body.
    #[tracked]
    #[returns(ref)]
    pub methods: Vec<FunctionDef<'db>>,
}

impl<'db> Spanned<'db> for InstanceDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        InstanceDef::span(*self, db)
    }
}

impl<'db> InstanceDef<'db> {
    /// Returns the stable definition identity for this instance.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        InstanceDef::def_id(*self, db)
    }

    /// Returns type variables with their binder spans.
    pub fn type_var_elems(&self, db: &'db dyn Db) -> &Vec<SpannedElem<'db, Ident<'db>>> {
        InstanceDef::type_vars(*self, db)
    }
}

/// Contract field declaration.
///
/// Fields are private to their containing contract scope and are represented by
/// declaration order during name resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldInit<'db> {
    /// Span covering the initializer expression.
    pub span: Span<'db>,
    /// Root expression ID in `exprs`.
    pub root: Id<Expr<'db>>,
    /// Arena containing the initializer expression tree.
    pub exprs: Arena<Expr<'db>>,
}

impl<'db> FieldInit<'db> {
    /// Creates a contract field initializer.
    pub fn new(span: Span<'db>, root: Id<Expr<'db>>, exprs: Arena<Expr<'db>>) -> Self {
        Self { span, root, exprs }
    }
}

impl<'db> Spanned<'db> for FieldInit<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldDef<'db> {
    name: SpannedElem<'db, Ident<'db>>,
    ty: TypeRef<'db>,
    init: Option<FieldInit<'db>>,
}

impl<'db> FieldDef<'db> {
    /// Creates a contract field declaration.
    pub fn new(
        name: SpannedElem<'db, Ident<'db>>,
        ty: TypeRef<'db>,
        init: Option<FieldInit<'db>>,
    ) -> Self {
        Self { name, ty, init }
    }

    /// Returns the field name with its binder span.
    pub fn name(&self) -> &SpannedElem<'db, Ident<'db>> {
        &self.name
    }

    /// Returns the unresolved type annotation for the field.
    pub fn ty(&self) -> TypeRef<'db> {
        self.ty
    }

    /// Returns the optional field initializer expression.
    pub fn init(&self) -> Option<&FieldInit<'db>> {
        self.init.as_ref()
    }
}

impl<'db> Spanned<'db> for FieldDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        let span = self.name.span(db) + self.ty.span(db);
        self.init.as_ref().map_or(span, |init| span + init.span(db))
    }
}

/// Items that can appear inside a contract body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum ContractItem<'db> {
    /// Contract-local function, constructor, fallback, or method-like item.
    FunctionDef(FunctionDef<'db>),
    /// Contract-local type alias.
    TypeAlias(TypeAlias<'db>),
    /// Contract-local data type.
    AdtDef(AdtDef<'db>),
    /// Parser recovery placeholder.
    Error {
        /// Span covering the recovered invalid contract item.
        span: Span<'db>,
    },
}

impl<'db> Spanned<'db> for ContractItem<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::FunctionDef(def) => def.span(db),
            Self::TypeAlias(def) => def.span(db),
            Self::AdtDef(def) => def.span(db),
            Self::Error { span } => *span,
        }
    }
}

/// Contract declaration.
///
/// Contracts introduce a type name, fields, and a nested item scope. Name
/// resolution gives fields precedence over same-name functions when resolving
/// terms inside the contract body.
#[salsa::tracked(debug)]
pub struct ContractDef<'db> {
    /// Stable structural identity of the contract.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full contract declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Contract name.
    #[tracked]
    pub name: SpannedElem<'db, Ident<'db>>,

    /// Contract type parameters in source order.
    #[tracked]
    #[returns(ref)]
    pub ty_params: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Field declarations in source order.
    #[tracked]
    #[returns(ref)]
    pub fields: Vec<FieldDef<'db>>,

    /// Nested contract items in source order.
    #[tracked]
    #[returns(ref)]
    pub items: Vec<ContractItem<'db>>,
}

impl<'db> Spanned<'db> for ContractDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        ContractDef::span(*self, db)
    }
}

impl<'db> ContractDef<'db> {
    /// Returns the stable definition identity for this contract.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        ContractDef::def_id(*self, db)
    }

    /// Returns the contract name with its declaration span.
    pub fn name_elem(&self, db: &'db dyn Db) -> SpannedElem<'db, Ident<'db>> {
        ContractDef::name(*self, db)
    }

    /// Returns type parameters with their binder spans.
    pub fn ty_param_elems(&self, db: &'db dyn Db) -> &Vec<SpannedElem<'db, Ident<'db>>> {
        ContractDef::ty_params(*self, db)
    }

    /// Returns whether this contract supplies its own ordinary runtime entry.
    ///
    /// This source-only predicate is shared by import-graph construction and
    /// the compiler overlay so implicit runtime dependencies cannot drift from
    /// the dispatch-generation condition.
    pub fn has_runtime_main(&self, db: &'db dyn Db) -> bool {
        self.items(db).iter().any(|item| {
            matches!(
                item,
                ContractItem::FunctionDef(function)
                    if function.kind(db) == FuncKind::Function
                        && function.sig(db).name.atom().text(db) == "main"
            )
        })
    }
}

impl<'db> Import<'db> {
    /// Returns import path segments with their source spans.
    pub fn path_elems(&self, db: &'db dyn Db) -> &Vec<SpannedElem<'db, Ident<'db>>> {
        Import::path(*self, db)
    }

    /// Returns the optional import alias with its binder span.
    pub fn alias_elem(&self, db: &'db dyn Db) -> Option<SpannedElem<'db, Ident<'db>>> {
        Import::alias(*self, db)
    }
}

/// Constructor selector used by imports and exports.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ConstructorSelector<'db> {
    /// Select every constructor of the named data type.
    All,
    /// Select only the named constructors.
    Named(Vec<SpannedElem<'db, Ident<'db>>>),
}

/// One selected name in an import selector.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SelectedName<'db> {
    /// Imported item name.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Optional local alias.
    pub alias: Option<SpannedElem<'db, Ident<'db>>>,
    /// Optional constructor selection for data types.
    pub constructors: Option<ConstructorSelector<'db>>,
    /// Whether `name` came from an operator selector.
    pub is_operator: bool,
}

/// Name hidden from an import.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ImportHiddenName<'db> {
    /// Hidden item name.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Whether `name` came from an operator selector.
    pub is_operator: bool,
}

/// Import selector.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ImportSelector<'db> {
    /// Import all exported names from the target module.
    Wildcard,
    /// Import only the listed names.
    Names(Vec<SelectedName<'db>>),
}

/// Module import declaration.
///
/// Imports can bind a module name, import selected items, hide names, and
/// reference external library roots through `external`.
#[salsa::tracked(debug)]
pub struct Import<'db> {
    /// Stable structural identity of the import.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full import declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Span of the external-library marker when present.
    #[tracked]
    #[returns(copy)]
    pub external: Option<Span<'db>>,

    /// Module path segments in source order.
    #[tracked]
    #[returns(ref)]
    pub path: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Optional module alias.
    #[tracked]
    pub alias: Option<SpannedElem<'db, Ident<'db>>>,

    /// Optional selected-import list.
    #[tracked]
    #[returns(ref)]
    pub selector: Option<ImportSelector<'db>>,

    /// Names hidden from the import.
    #[tracked]
    #[returns(ref)]
    pub hiding: Vec<ImportHiddenName<'db>>,
}

impl<'db> Spanned<'db> for Import<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Import::span(*self, db)
    }
}

/// One exported name in an export declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExportedName<'db> {
    /// Exported item name.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Optional constructor selection for data types.
    pub constructors: Option<ConstructorSelector<'db>>,
    /// Whether `name` came from an operator selector.
    pub is_operator: bool,
}

/// Export declaration payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ExportKind<'db> {
    /// Explicit export list from the current module.
    List(Vec<ExportedName<'db>>),
    /// Re-export every public item from the named module.
    Module(Vec<SpannedElem<'db, Ident<'db>>>),
    /// Re-export a module under an alias.
    ModuleAs(
        /// Source module path.
        Vec<SpannedElem<'db, Ident<'db>>>,
        /// Exported alias.
        SpannedElem<'db, Ident<'db>>,
    ),
    /// Re-export selected items from a module.
    ItemsFrom(Vec<SpannedElem<'db, Ident<'db>>>, Vec<ExportedName<'db>>),
}

/// Module export declaration.
#[salsa::tracked(debug)]
pub struct Export<'db> {
    /// Stable structural identity of the export.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full export declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Export payload.
    #[tracked]
    #[returns(ref)]
    pub kind: ExportKind<'db>,
}

impl<'db> Spanned<'db> for Export<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Export::span(*self, db)
    }
}

/// Pragma declaration.
///
/// Pragmas are parsed and preserved in HIR so later phases can opt into
/// pragma-specific behavior without reparsing source text.
#[salsa::tracked(debug)]
pub struct Pragma<'db> {
    /// Stable structural identity of the pragma.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the full pragma declaration.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Pragma name.
    #[tracked]
    pub name: SpannedElem<'db, Ident<'db>>,

    /// Pragma arguments/items in source order.
    #[tracked]
    #[returns(ref)]
    pub items: Vec<SpannedElem<'db, Ident<'db>>>,
}

impl<'db> Spanned<'db> for Pragma<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Pragma::span(*self, db)
    }
}

/// Top-level module item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Item<'db> {
    /// Function declaration or definition.
    FunctionDef(FunctionDef<'db>),
    /// Type alias declaration.
    TypeAlias(TypeAlias<'db>),
    /// Algebraic data type declaration.
    AdtDef(AdtDef<'db>),
    /// Type class declaration.
    ClassDef(ClassDef<'db>),
    /// Type class instance declaration.
    InstanceDef(InstanceDef<'db>),
    /// Contract declaration.
    ContractDef(ContractDef<'db>),
    /// Import declaration.
    Import(Import<'db>),
    /// Export declaration.
    Export(Export<'db>),
    /// Pragma declaration.
    Pragma(Pragma<'db>),
    /// Parser recovery placeholder.
    Error {
        /// Span covering the recovered invalid top-level item.
        span: Span<'db>,
    },
}

impl<'db> Spanned<'db> for Item<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::FunctionDef(def) => def.span(db),
            Self::TypeAlias(def) => def.span(db),
            Self::AdtDef(def) => def.span(db),
            Self::ClassDef(def) => def.span(db),
            Self::InstanceDef(def) => def.span(db),
            Self::ContractDef(def) => def.span(db),
            Self::Import(def) => def.span(db),
            Self::Export(def) => def.span(db),
            Self::Pragma(def) => def.span(db),
            Self::Error { span } => *span,
        }
    }
}

/// A module/source file after lowering into HIR.
///
/// A module is itself a definition so item identity can be rooted in an owner
/// chain. The module span is rooted at the source file, while child definitions
/// usually use def anchors.
#[salsa::tracked(debug)]
pub struct Module<'db> {
    /// Stable structural identity of the module.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the source file contents.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Top-level items in source order.
    #[tracked]
    #[returns(ref)]
    pub items: Vec<Item<'db>>,
}

impl<'db> Spanned<'db> for Module<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Module::span(*self, db)
    }
}

impl<'db> Module<'db> {
    /// Returns the stable definition identity for this module.
    pub fn def_id_value(&self, db: &'db dyn Db) -> DefId<'db> {
        Module::def_id(*self, db)
    }
}
