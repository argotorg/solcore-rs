//! Cache-stable frontend desugar planning.
//!
//! This module records desugar facts without mutating parsed HIR or reparsing
//! generated source. That keeps the parser and LSP-facing spans tied to the
//! user's file while later phases can opt into a normalized view.

use hir::{
    anchor::DefId,
    arena::{Arena, Id},
    ast::{
        function::{Expr, ExprKind, FuncBody, FuncParam, FuncSig, Pat, PatKind, Stmt, StmtKind},
        item::{
            AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FieldInit, FunctionDef,
            InstanceDef, Item, Module, TypeAlias,
        },
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    span::{Span, Spanned},
};

use crate::Db;

/// Source span that should receive diagnostics for a generated/desugared node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceOrigin<'db> {
    /// User-written syntax that caused the generated node to exist.
    pub span: Span<'db>,
    /// Desugar category for diagnostics and debugging.
    pub kind: SourceOriginKind,
}

impl<'db> SourceOrigin<'db> {
    /// Creates a source-origin record for generated/desugared syntax.
    pub const fn new(span: Span<'db>, kind: SourceOriginKind) -> Self {
        Self { span, kind }
    }
}

/// Categories of user syntax that can produce generated/desugared nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceOriginKind {
    /// Tuple expression syntax normalized to unit/single/pair product form.
    TupleExpr,
    /// Tuple pattern syntax normalized to unit/single/pair product form.
    TuplePat,
    /// Tuple type syntax normalized to unit/single/pair product form.
    TupleType,
    /// If statement lowered through match-on-bool.
    IfStatement,
    /// If expression lowered through match-on-bool.
    IfExpression,
    /// Bool constructor/pattern rewritten to the unit-sum encoding.
    BoolConstructor,
    /// Contract field read rewritten to a storage hook.
    FieldRead,
    /// Contract field write rewritten to a storage hook.
    FieldWrite,
    /// Indirect call rewritten through the invokable dictionary.
    IndirectCall,
    /// Generated glue with no tighter user token than the enclosing construct.
    GeneratedGlue,
}

/// Product payload shape used by frontend desugaring.
///
/// Tuple-like syntax is normalized with this language-level product convention:
///
/// - `()` becomes unit.
/// - `(a)` becomes `a`.
/// - `(a, b)` becomes `pair(a, b)`.
/// - `(a, b, c)` becomes `pair(a, pair(b, c))`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ProductShape<T> {
    /// Empty product/unit.
    Unit,
    /// Singleton product, represented without a pair wrapper.
    Single(T),
    /// Right-nested pair product.
    Pair {
        /// First element at this level.
        head: T,
        /// Remaining product payload.
        tail: Box<ProductShape<T>>,
    },
}

impl<T: Clone> ProductShape<T> {
    /// Builds a right-nested product shape from source-order elements.
    pub fn from_slice(elems: &[T]) -> Self {
        let Some((head, tail)) = elems.split_first() else {
            return Self::Unit;
        };
        if tail.is_empty() {
            Self::Single(head.clone())
        } else {
            Self::Pair {
                head: head.clone(),
                tail: Box::new(Self::from_slice(tail)),
            }
        }
    }
}

/// Planned pre-typecheck desugars for one module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PreTypeckDesugarPlan<'db> {
    /// Tuple type references from item signatures, aliases, and fields.
    ///
    /// Body-local type annotations live in [`BodyPreTypeckDesugarPlan::types`]
    /// so type checking can depend on one body at a time.
    pub types: Vec<TypeProductDesugar<'db>>,
    /// Tuple expression/pattern rewrites inside function and lambda bodies.
    pub bodies: Vec<BodyPreTypeckDesugarPlan<'db>>,
    /// Tuple expression rewrites inside contract field initializers.
    pub field_inits: Vec<FieldInitPreTypeckDesugarPlan<'db>>,
}

/// Planned tuple-type desugar for one source type occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeProductDesugar<'db> {
    /// Source type reference being normalized.
    pub ty: TypeRef<'db>,
    /// Diagnostic origin for the normalized type.
    pub origin: SourceOrigin<'db>,
    /// Unit/single/right-nested-pair product shape.
    pub product: ProductShape<TypeRef<'db>>,
}

/// Planned body-local pre-typecheck desugars.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyPreTypeckDesugarPlan<'db> {
    /// Function or lambda body containing the source nodes.
    pub body: FuncBody<'db>,
    /// Tuple type references inside local annotations and lambda signatures.
    pub types: Vec<TypeProductDesugar<'db>>,
    /// Tuple expression/pattern transforms in traversal order.
    pub transforms: Vec<PreTypeckTransform<'db>>,
}

/// One body-local pre-typecheck transform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PreTypeckTransform<'db> {
    /// Tuple expression normalized to unit/single/pair product form.
    TupleExprToProduct {
        /// Source expression.
        expr: Id<Expr<'db>>,
        /// Diagnostic origin for generated product nodes.
        origin: SourceOrigin<'db>,
        /// Unit/single/right-nested-pair payload shape.
        product: ProductShape<Id<Expr<'db>>>,
    },
    /// Tuple pattern normalized to unit/single/pair product form.
    TuplePatToProduct {
        /// Source pattern.
        pat: Id<Pat<'db>>,
        /// Diagnostic origin for generated product nodes.
        origin: SourceOrigin<'db>,
        /// Unit/single/right-nested-pair payload shape.
        product: ProductShape<Id<Pat<'db>>>,
    },
}

/// Planned pre-typecheck desugars for a contract field initializer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldInitPreTypeckDesugarPlan<'db> {
    /// Contract owning the field.
    pub contract: DefId<'db>,
    /// Field name for debugging and snapshot-friendly assertions.
    pub field_name: String,
    /// Tuple expression transforms in traversal order.
    pub transforms: Vec<FieldInitPreTypeckTransform<'db>>,
}

/// One field-initializer pre-typecheck transform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FieldInitPreTypeckTransform<'db> {
    /// Tuple expression normalized to unit/single/pair product form.
    TupleExprToProduct {
        /// Source expression in the field initializer arena.
        expr: Id<Expr<'db>>,
        /// Diagnostic origin for generated product nodes.
        origin: SourceOrigin<'db>,
        /// Unit/single/right-nested-pair payload shape.
        product: ProductShape<Id<Expr<'db>>>,
    },
}

/// Computes pre-typecheck desugar facts for the parsed module without changing
/// the module itself.
#[salsa::tracked]
pub fn pre_typeck_desugar_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> PreTypeckDesugarPlan<'db> {
    let mut collector = ModuleCollector {
        db,
        types: Vec::new(),
        bodies: Vec::new(),
        field_inits: Vec::new(),
    };
    for item in module.items(db) {
        collector.item(*item);
    }
    PreTypeckDesugarPlan {
        types: collector.types,
        bodies: collector.bodies,
        field_inits: collector.field_inits,
    }
}

/// Computes pre-typecheck desugar facts for one body tree.
///
/// The returned list contains `body` and any nested lambda bodies that contain
/// tuple/product desugar facts. Keeping this as a separate tracked query gives
/// type checking a narrow cache boundary to call before inference.
#[salsa::tracked]
pub fn pre_typeck_desugar_body_tree<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
) -> Vec<BodyPreTypeckDesugarPlan<'db>> {
    let mut collector = BodyCollector {
        db,
        body,
        types: Vec::new(),
        nested_bodies: Vec::new(),
        transforms: Vec::new(),
    };
    for stmt in body.top_level_stmts(db) {
        collector.stmt(*stmt);
    }

    let mut bodies = Vec::new();
    if !collector.types.is_empty() || !collector.transforms.is_empty() {
        bodies.push(BodyPreTypeckDesugarPlan {
            body,
            types: collector.types,
            transforms: collector.transforms,
        });
    }
    bodies.extend(collector.nested_bodies);
    bodies
}

struct ModuleCollector<'db> {
    db: &'db dyn Db,
    types: Vec<TypeProductDesugar<'db>>,
    bodies: Vec<BodyPreTypeckDesugarPlan<'db>>,
    field_inits: Vec<FieldInitPreTypeckDesugarPlan<'db>>,
}

impl<'db> ModuleCollector<'db> {
    fn item(&mut self, item: Item<'db>) {
        match item {
            Item::FunctionDef(function) => self.function(function),
            Item::TypeAlias(alias) => self.type_alias(alias),
            Item::AdtDef(adt) => self.adt(adt),
            Item::ClassDef(class) => self.class(class),
            Item::InstanceDef(instance) => self.instance(instance),
            Item::ContractDef(contract) => self.contract(contract),
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
    }

    fn contract_item(&mut self, item: ContractItem<'db>) {
        match item {
            ContractItem::FunctionDef(function) => self.function(function),
            ContractItem::TypeAlias(alias) => self.type_alias(alias),
            ContractItem::AdtDef(adt) => self.adt(adt),
            ContractItem::Error { .. } => {}
        }
    }

    fn function(&mut self, function: FunctionDef<'db>) {
        self.func_sig(function.sig(self.db));
        if let Some(body) = function.body(self.db) {
            self.bodies
                .extend(pre_typeck_desugar_body_tree(self.db, body));
        }
    }

    fn type_alias(&mut self, alias: TypeAlias<'db>) {
        self.type_ref(alias.ty(self.db));
    }

    fn adt(&mut self, adt: AdtDef<'db>) {
        for ctor in adt.ctors(self.db) {
            self.type_ref(*ctor.fields.atom());
        }
    }

    fn class(&mut self, class: ClassDef<'db>) {
        for pred in class.super_preds(self.db) {
            self.pred_ref(*pred);
        }
        self.pred_ref(class.head(self.db));
        for method in class.methods(self.db) {
            self.func_sig(method);
        }
    }

    fn instance(&mut self, instance: InstanceDef<'db>) {
        for pred in instance.preds(self.db) {
            self.pred_ref(*pred);
        }
        self.pred_ref(instance.head(self.db));
        for method in instance.methods(self.db) {
            self.function(*method);
        }
    }

    fn contract(&mut self, contract: ContractDef<'db>) {
        for field in contract.fields(self.db) {
            self.field(contract.def_id_value(self.db), field);
        }
        for item in contract.items(self.db) {
            self.contract_item(*item);
        }
    }

    fn field(&mut self, contract: DefId<'db>, field: &FieldDef<'db>) {
        self.type_ref(field.ty());
        if let Some(init) = field.init() {
            self.field_init(contract, field.name().atom().text(self.db).to_owned(), init);
        }
    }

    fn field_init(&mut self, contract: DefId<'db>, field_name: String, init: &FieldInit<'db>) {
        let mut collector = FieldInitCollector {
            exprs: &init.exprs,
            transforms: Vec::new(),
        };
        collector.expr(init.root);
        if !collector.transforms.is_empty() {
            self.field_inits.push(FieldInitPreTypeckDesugarPlan {
                contract,
                field_name,
                transforms: collector.transforms,
            });
        }
    }

    fn func_sig(&mut self, sig: &FuncSig<'db>) {
        for pred in &sig.preds {
            self.pred_ref(*pred);
        }
        for param in sig.params.atom() {
            self.func_param(param);
        }
        if let Some(ret) = sig.ret {
            self.type_ref(ret);
        }
    }

    fn func_param(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.type_ref(*ty);
        }
    }

    fn pred_ref(&mut self, pred: PredRef<'db>) {
        let kind = pred.kind(self.db);
        self.type_ref(kind.ty);
        for arg in kind.args.atom() {
            self.type_ref(*arg);
        }
    }

    fn type_ref(&mut self, ty: TypeRef<'db>) {
        match ty.kind(self.db) {
            TypeRefKind::Named { args, .. } => {
                for arg in args.atom() {
                    self.type_ref(*arg);
                }
            }
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.type_ref(*param);
                }
                self.type_ref(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.type_ref(*inner),
            TypeRefKind::Tuple { elems } => {
                self.types.push(TypeProductDesugar {
                    ty,
                    origin: SourceOrigin::new(ty.span(self.db), SourceOriginKind::TupleType),
                    product: ProductShape::from_slice(elems.atom()),
                });
                for elem in elems.atom() {
                    self.type_ref(*elem);
                }
            }
            TypeRefKind::Error { .. } => {}
        }
    }
}

struct BodyCollector<'db> {
    db: &'db dyn Db,
    body: FuncBody<'db>,
    types: Vec<TypeProductDesugar<'db>>,
    nested_bodies: Vec<BodyPreTypeckDesugarPlan<'db>>,
    transforms: Vec<PreTypeckTransform<'db>>,
}

impl<'db> BodyCollector<'db> {
    fn stmt(&mut self, stmt_id: Id<Stmt<'db>>) {
        match &self.body.stmts(self.db).get(stmt_id).kind {
            StmtKind::Let { ty, init, .. } => {
                if let Some(ty) = ty {
                    self.type_ref(*ty);
                }
                if let Some(init) = init {
                    self.expr(*init);
                }
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(*expr);
                }
            }
            StmtKind::Expr(expr) => self.expr(*expr),
            StmtKind::Assign { lhs, rhs, .. } => {
                self.expr(*lhs);
                self.expr(*rhs);
            }
            StmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.expr(*scrutinee);
                }
                for arm in arms {
                    for pat in &arm.pats {
                        self.pat(*pat);
                    }
                    for stmt in &arm.body {
                        self.stmt(*stmt);
                    }
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                for stmt in init {
                    self.stmt(*stmt);
                }
                self.expr(*cond);
                for stmt in post {
                    self.stmt(*stmt);
                }
                for stmt in body {
                    self.stmt(*stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.expr(*cond);
                for stmt in then_body {
                    self.stmt(*stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.stmt(*stmt);
                    }
                }
            }
            StmtKind::Block { body } => {
                for stmt in body {
                    self.stmt(*stmt);
                }
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn expr(&mut self, expr_id: Id<Expr<'db>>) {
        let expr = self.body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::DotCtor { args, .. } => {
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Lambda { params, ret, body } => {
                for param in params.atom() {
                    self.func_param(param);
                }
                if let Some(ret) = ret {
                    self.type_ref(*ret);
                }
                self.nested_bodies
                    .extend(pre_typeck_desugar_body_tree(self.db, *body));
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(*lhs);
                self.expr(*rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(*base);
                self.expr(*index);
            }
            ExprKind::Call { callee, args } => {
                self.expr(*callee);
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Field { base, .. } => self.expr(*base),
            ExprKind::TypeAnnot { expr, ty } => {
                self.expr(*expr);
                self.type_ref(*ty);
            }
            ExprKind::UnaryOp { expr, .. } => self.expr(*expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(*cond);
                self.expr(*then_expr);
                self.expr(*else_expr);
            }
            ExprKind::Tuple(elems) => {
                self.transforms
                    .push(PreTypeckTransform::TupleExprToProduct {
                        expr: expr_id,
                        origin: SourceOrigin::new(expr.span, SourceOriginKind::TupleExpr),
                        product: ProductShape::from_slice(elems),
                    });
                for elem in elems {
                    self.expr(*elem);
                }
            }
            ExprKind::Proxy { ty, .. } => self.type_ref(*ty),
            ExprKind::Lit(_) | ExprKind::Ident(_) | ExprKind::Error => {}
        }
    }

    fn pat(&mut self, pat_id: Id<Pat<'db>>) {
        let pat = self.body.pats(self.db).get(pat_id);
        match &pat.kind {
            PatKind::Ctor { args, .. } => {
                for arg in args {
                    self.pat(*arg);
                }
            }
            PatKind::ComptimeLabel { expr, .. } => self.expr(*expr),
            PatKind::Tuple { elems } => {
                self.transforms.push(PreTypeckTransform::TuplePatToProduct {
                    pat: pat_id,
                    origin: SourceOrigin::new(pat.span, SourceOriginKind::TuplePat),
                    product: ProductShape::from_slice(elems),
                });
                for elem in elems {
                    self.pat(*elem);
                }
            }
            PatKind::Wildcard | PatKind::Var(_) | PatKind::Lit(_) | PatKind::Error => {}
        }
    }

    fn func_param(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.type_ref(*ty);
        }
    }

    fn type_ref(&mut self, ty: TypeRef<'db>) {
        match ty.kind(self.db) {
            TypeRefKind::Named { args, .. } => {
                for arg in args.atom() {
                    self.type_ref(*arg);
                }
            }
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.type_ref(*param);
                }
                self.type_ref(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.type_ref(*inner),
            TypeRefKind::Tuple { elems } => {
                self.types.push(TypeProductDesugar {
                    ty,
                    origin: SourceOrigin::new(ty.span(self.db), SourceOriginKind::TupleType),
                    product: ProductShape::from_slice(elems.atom()),
                });
                for elem in elems.atom() {
                    self.type_ref(*elem);
                }
            }
            TypeRefKind::Error { .. } => {}
        }
    }
}

struct FieldInitCollector<'a, 'db> {
    exprs: &'a Arena<Expr<'db>>,
    transforms: Vec<FieldInitPreTypeckTransform<'db>>,
}

impl<'a, 'db> FieldInitCollector<'a, 'db> {
    fn expr(&mut self, expr_id: Id<Expr<'db>>) {
        let expr = self.exprs.get(expr_id);
        match &expr.kind {
            ExprKind::DotCtor { args, .. } => {
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(*lhs);
                self.expr(*rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(*base);
                self.expr(*index);
            }
            ExprKind::Call { callee, args } => {
                self.expr(*callee);
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Field { base, .. } => self.expr(*base),
            ExprKind::TypeAnnot { expr, .. } | ExprKind::UnaryOp { expr, .. } => self.expr(*expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(*cond);
                self.expr(*then_expr);
                self.expr(*else_expr);
            }
            ExprKind::Tuple(elems) => {
                self.transforms
                    .push(FieldInitPreTypeckTransform::TupleExprToProduct {
                        expr: expr_id,
                        origin: SourceOrigin::new(expr.span, SourceOriginKind::TupleExpr),
                        product: ProductShape::from_slice(elems),
                    });
                for elem in elems {
                    self.expr(*elem);
                }
            }
            ExprKind::Lambda { .. }
            | ExprKind::Lit(_)
            | ExprKind::Ident(_)
            | ExprKind::Proxy { .. }
            | ExprKind::Error => {}
        }
    }
}
