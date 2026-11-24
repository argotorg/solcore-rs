//! Constructors of imported data types that an import does not include.

use super::*;
use item_refs::find_origin_adt;

/// Returns one fact per constructor of the data type behind `item_ref` that
/// `visible` does not include, where `provider` is the module the type was
/// imported from.
pub(super) fn hidden_constructor_facts<'db>(
    db: &'db dyn Db,
    item_ref: &ItemRef<'db>,
    visible: Option<&VisibleConstructors>,
    provider: ModuleId<'db>,
) -> Vec<HiddenConstructorFact<'db>> {
    let module = item_ref.origin.module;
    if module_has_parse_errors(db, module) {
        return Vec::new();
    }
    let Some(adt) = find_origin_adt(db, module, item_ref.origin.def_id) else {
        return Vec::new();
    };
    let type_name = spanned_name_text(db, &adt.name(db));
    let module_name = module_id_display(db, provider);
    adt.ctors(db)
        .iter()
        .enumerate()
        .filter_map(|(index, ctor)| {
            let name = spanned_name_text(db, &ctor.name);
            if visible.is_some_and(|visible| visible.contains(&name)) {
                return None;
            }
            Some(HiddenConstructorFact {
                ty: item_ref.origin.def_id,
                index: hir_nameres::CtorIndex::from_usize(index),
                fields: ctor
                    .field_names
                    .iter()
                    .flatten()
                    .map(|field| {
                        (
                            spanned_name_text(db, field),
                            LabelSpan::from_span(db, field.span(db)),
                        )
                    })
                    .collect(),
                constructor: hir_nameres::HiddenConstructor {
                    type_name: type_name.clone(),
                    constructor: name,
                    is_struct: ctor.field_names.is_some(),
                    module: module_name.clone(),
                    span: LabelSpan::from_span(db, ctor.name.span(db)),
                },
            })
        })
        .collect()
}

impl<'db> ModuleEnv<'db> {
    /// Returns the constructor `name` of the imported data type `ty` when the
    /// module it was imported from does not export it and no visible term
    /// resolves to it.
    pub fn find_hidden_constructor(
        &self,
        ty: DefId<'db>,
        name: &str,
    ) -> Option<hir_nameres::HiddenConstructor> {
        find_hidden_constructor(&self.surface, &self.hidden_constructors, ty, name)
    }

    /// Returns the constructor of the imported data type `ty` that has the
    /// type's own name, which a call such as `Box(1)` refers to, when it is
    /// hidden in the sense of [`Self::find_hidden_constructor`].
    pub fn find_hidden_same_name_constructor(
        &self,
        ty: DefId<'db>,
    ) -> Option<hir_nameres::HiddenConstructor> {
        hidden_fact(&self.surface, &self.hidden_constructors, ty, |fact| {
            fact.constructor.constructor == fact.constructor.type_name
        })
        .map(|fact| fact.constructor.clone())
    }

    /// Returns the constructor of the imported data type `ty` that declares
    /// the field `field`, with the field's declaration span, when that
    /// constructor is hidden in the sense of [`Self::find_hidden_constructor`].
    pub fn find_hidden_field_constructor(
        &self,
        ty: DefId<'db>,
        field: &str,
    ) -> Option<(hir_nameres::HiddenConstructor, LabelSpan)> {
        let fact = hidden_fact(&self.surface, &self.hidden_constructors, ty, |fact| {
            fact.fields.iter().any(|(name, _)| name == field)
        })?;
        let (_, span) = fact.fields.iter().find(|(name, _)| name == field)?;
        Some((fact.constructor.clone(), span.clone()))
    }
}

/// Import names for body name resolution: the import surface plus the hidden
/// constructor facts that only diagnostics read.
pub(super) struct SurfaceWithHiddenConstructors<'a, 'db> {
    pub(super) surface: &'a ModuleImportSurface<'db>,
    pub(super) hidden_constructors: &'a [HiddenConstructorFact<'db>],
}

impl<'db> hir_nameres::ImportedNames<'db> for SurfaceWithHiddenConstructors<'_, 'db> {
    fn imported(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        name: &str,
    ) -> Option<hir_nameres::Resolution<'db>> {
        self.surface.imported(db, namespace, name)
    }

    fn has_constructor_leaf(&self, db: &'db dyn hir::Db, leaf: &str) -> bool {
        self.surface.has_constructor_leaf(db, leaf)
    }

    fn may_contain_unknown_unqualified(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        name: &str,
    ) -> bool {
        self.surface
            .may_contain_unknown_unqualified(db, namespace, name)
    }

    fn has_incomplete_module_qualifier(&self, db: &'db dyn hir::Db, qualifier: &str) -> bool {
        self.surface.has_incomplete_module_qualifier(db, qualifier)
    }

    fn candidate_names(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
    ) -> Vec<String> {
        self.surface.candidate_names(db, namespace)
    }

    fn constructor_type_candidates(
        &self,
        db: &'db dyn hir::Db,
        leaf: &str,
    ) -> Vec<hir_nameres::ConstructorTypeCandidate> {
        self.surface.constructor_type_candidates(db, leaf)
    }

    fn private_candidate(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        qualifier: &str,
        name: &str,
    ) -> Option<hir_nameres::PrivateCandidate> {
        self.surface
            .private_candidate(db, namespace, qualifier, name)
    }

    fn hidden_constructor(
        &self,
        _db: &'db dyn hir::Db,
        ty: DefId<'db>,
        name: &str,
    ) -> Option<hir_nameres::HiddenConstructor> {
        find_hidden_constructor(self.surface, self.hidden_constructors, ty, name)
    }
}

fn find_hidden_constructor<'db>(
    surface: &ModuleImportSurface<'db>,
    hidden_constructors: &[HiddenConstructorFact<'db>],
    ty: DefId<'db>,
    name: &str,
) -> Option<hir_nameres::HiddenConstructor> {
    hidden_fact(surface, hidden_constructors, ty, |fact| {
        fact.constructor.constructor == name
    })
    .map(|fact| fact.constructor.clone())
}

/// Returns the first fact for `ty` that `matches` accepts, unless a term in
/// `surface` makes its constructor visible.
fn hidden_fact<'a, 'db>(
    surface: &ModuleImportSurface<'db>,
    hidden_constructors: &'a [HiddenConstructorFact<'db>],
    ty: DefId<'db>,
    matches: impl Fn(&HiddenConstructorFact<'db>) -> bool,
) -> Option<&'a HiddenConstructorFact<'db>> {
    let fact = hidden_constructors
        .iter()
        .find(|fact| fact.ty == ty && matches(fact))?;
    let visible = surface.terms.values().any(|resolution| {
        matches!(
            resolution,
            hir_nameres::Resolution::Ctor { ty: visible_ty, index }
                if *visible_ty == ty && *index == fact.index
        )
    });
    (!visible).then_some(fact)
}
