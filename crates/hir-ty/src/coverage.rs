//! Pattern-match coverage analysis.
//!
//! This module implements Maranget's usefulness test over pattern matrices. The
//! surrounding inference code is responsible for translating HIR patterns into
//! this small pattern language and for supplying type-specific constructor data.

use hir::anchor::DefId;

/// Constructor head used by coverage analysis.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum CoverageCtor<'db> {
    /// User ADT constructor.
    User {
        /// Type definition that owns this constructor.
        ty: DefId<'db>,
        /// Constructor index inside the ADT definition.
        index: u32,
        /// Display name of the owning type.
        ty_name: String,
        /// Display name of the constructor.
        name: String,
    },
    /// Builtin constructor.
    Builtin(BuiltinCoverageCtor),
}

/// Builtin constructor heads known to the coverage checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BuiltinCoverageCtor {
    /// Boolean `true`.
    True,
    /// Boolean `false`.
    False,
    /// Unit constructor.
    Unit,
    /// Tuple constructor of the given arity.
    Tuple(usize),
    /// Builtin pair constructor.
    Pair,
    /// Builtin sum left injection.
    Inl,
    /// Builtin sum right injection.
    Inr,
}

/// Pattern representation consumed by the coverage algorithm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoveragePat<'db> {
    /// Wildcard or variable pattern.
    Wild,
    /// Constructor pattern.
    Ctor(CoverageCtor<'db>, Vec<CoveragePat<'db>>),
    /// Literal-like constant with an open-ended constructor signature.
    Literal(String),
    /// A pattern whose exact matching set is intentionally opaque.
    Opaque,
}

/// Witness pattern for a value not covered by a pattern matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WitnessPat<'db> {
    /// Any inhabitant.
    Wild,
    /// Constructor witness with field witnesses.
    Ctor(CoverageCtor<'db>, Vec<WitnessPat<'db>>),
}

/// Result of checking one pattern matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoverageAnalysis<'db> {
    /// One missing value row when the matrix is non-exhaustive.
    pub(crate) missing: Option<Vec<WitnessPat<'db>>>,
    /// Indices of arms that are covered by previous arms.
    pub(crate) unreachable: Vec<usize>,
}

/// Type-dependent constructor information needed by the matrix algorithm.
pub(crate) trait ConstructorOracle<'db, Ty> {
    /// Returns the complete finite constructor signature for `ty`, when known.
    fn constructors(&mut self, ty: Ty) -> Option<Vec<CoverageCtor<'db>>>;

    /// Returns the field types for `ctor` at scrutinee type `ty`.
    fn fields(&mut self, ctor: &CoverageCtor<'db>, ty: Ty) -> Option<Vec<Ty>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Usefulness<'db> {
    Useful(Vec<WitnessPat<'db>>),
    Useless,
    Unknown,
}

/// Computes exhaustiveness and unreachable-arm information for a pattern matrix.
pub(crate) fn analyze<'db, Ty, O>(
    oracle: &mut O,
    tys: &[Ty],
    rows: &[Vec<CoveragePat<'db>>],
) -> CoverageAnalysis<'db>
where
    Ty: Clone,
    O: ConstructorOracle<'db, Ty>,
{
    let mut previous = Vec::with_capacity(rows.len());
    let mut unreachable = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        if matches!(
            usefulness_witness(oracle, tys, &previous, row),
            Usefulness::Useless
        ) {
            unreachable.push(index);
        }
        previous.push(row.clone());
    }

    let wildcard_row = vec![CoveragePat::Wild; tys.len()];
    let missing = match usefulness_witness(oracle, tys, rows, &wildcard_row) {
        Usefulness::Useful(witness) => Some(witness),
        Usefulness::Useless | Usefulness::Unknown => None,
    };

    CoverageAnalysis {
        missing,
        unreachable,
    }
}

fn usefulness_witness<'db, Ty, O>(
    oracle: &mut O,
    tys: &[Ty],
    matrix: &[Vec<CoveragePat<'db>>],
    query: &[CoveragePat<'db>],
) -> Usefulness<'db>
where
    Ty: Clone,
    O: ConstructorOracle<'db, Ty>,
{
    if query.len() != tys.len() || matrix.iter().any(|row| row.len() != tys.len()) {
        return Usefulness::Unknown;
    }
    usefulness_rec(oracle, tys, matrix, query)
}

fn usefulness_rec<'db, Ty, O>(
    oracle: &mut O,
    tys: &[Ty],
    matrix: &[Vec<CoveragePat<'db>>],
    query: &[CoveragePat<'db>],
) -> Usefulness<'db>
where
    Ty: Clone,
    O: ConstructorOracle<'db, Ty>,
{
    if matrix.is_empty() {
        return Usefulness::Useful(witness_from_query(query));
    }
    if tys.is_empty() {
        return Usefulness::Useless;
    }

    let Some((head, rest_query)) = query.split_first() else {
        return Usefulness::Unknown;
    };
    let Some((head_ty, rest_tys)) = tys.split_first() else {
        return Usefulness::Unknown;
    };

    match head {
        CoveragePat::Ctor(ctor, fields) => {
            let Some(field_tys) = oracle.fields(ctor, head_ty.clone()) else {
                return Usefulness::Unknown;
            };
            if field_tys.len() != fields.len() {
                return Usefulness::Unknown;
            }
            let specialized = specialize_ctor_matrix(ctor, fields.len(), matrix);
            let mut next_tys = field_tys;
            next_tys.extend_from_slice(rest_tys);
            let mut next_query = fields.clone();
            next_query.extend_from_slice(rest_query);
            recompose_ctor(
                ctor.clone(),
                fields.len(),
                usefulness_rec(oracle, &next_tys, &specialized, &next_query),
            )
        }
        CoveragePat::Literal(value) => {
            let specialized = specialize_literal_matrix(value, matrix);
            prepend_wild(usefulness_rec(oracle, rest_tys, &specialized, rest_query))
        }
        CoveragePat::Opaque => {
            let default = default_matrix(matrix);
            prepend_wild(usefulness_rec(oracle, rest_tys, &default, rest_query))
        }
        CoveragePat::Wild => {
            let seen = root_ctors(matrix);
            if seen.is_empty() {
                let default = default_matrix(matrix);
                return prepend_wild(usefulness_rec(oracle, rest_tys, &default, rest_query));
            }

            let Some(ctors) = oracle.constructors(head_ty.clone()) else {
                return Usefulness::Unknown;
            };
            if ctors.is_empty() {
                return Usefulness::Unknown;
            }

            let mut saw_unknown = false;
            for ctor in ctors {
                let Some(field_tys) = oracle.fields(&ctor, head_ty.clone()) else {
                    saw_unknown = true;
                    continue;
                };
                let field_count = field_tys.len();
                let specialized = specialize_ctor_matrix(&ctor, field_count, matrix);
                let mut next_tys = field_tys;
                next_tys.extend_from_slice(rest_tys);
                let mut next_query = vec![CoveragePat::Wild; field_count];
                next_query.extend_from_slice(rest_query);

                match recompose_ctor(
                    ctor,
                    field_count,
                    usefulness_rec(oracle, &next_tys, &specialized, &next_query),
                ) {
                    Usefulness::Useful(witness) => return Usefulness::Useful(witness),
                    Usefulness::Unknown => saw_unknown = true,
                    Usefulness::Useless => {}
                }
            }

            if saw_unknown {
                Usefulness::Unknown
            } else {
                Usefulness::Useless
            }
        }
    }
}

fn specialize_ctor_matrix<'db>(
    ctor: &CoverageCtor<'db>,
    field_count: usize,
    matrix: &[Vec<CoveragePat<'db>>],
) -> Vec<Vec<CoveragePat<'db>>> {
    let mut specialized = Vec::new();
    for row in matrix {
        let Some((head, rest)) = row.split_first() else {
            continue;
        };
        match head {
            CoveragePat::Ctor(head_ctor, fields) if head_ctor == ctor => {
                let mut next = fields.clone();
                next.extend(rest.iter().cloned());
                specialized.push(next);
            }
            CoveragePat::Wild => {
                let mut next = vec![CoveragePat::Wild; field_count];
                next.extend(rest.iter().cloned());
                specialized.push(next);
            }
            CoveragePat::Ctor(_, _) | CoveragePat::Literal(_) | CoveragePat::Opaque => {}
        }
    }
    specialized
}

fn specialize_literal_matrix<'db>(
    value: &str,
    matrix: &[Vec<CoveragePat<'db>>],
) -> Vec<Vec<CoveragePat<'db>>> {
    matrix
        .iter()
        .filter_map(|row| {
            let (head, rest) = row.split_first()?;
            match head {
                CoveragePat::Literal(head_value) if head_value == value => Some(rest.to_vec()),
                CoveragePat::Wild => Some(rest.to_vec()),
                CoveragePat::Ctor(_, _) | CoveragePat::Literal(_) | CoveragePat::Opaque => None,
            }
        })
        .collect()
}

fn default_matrix<'db>(matrix: &[Vec<CoveragePat<'db>>]) -> Vec<Vec<CoveragePat<'db>>> {
    matrix
        .iter()
        .filter_map(|row| {
            let (head, rest) = row.split_first()?;
            matches!(head, CoveragePat::Wild).then(|| rest.to_vec())
        })
        .collect()
}

fn root_ctors<'db>(matrix: &[Vec<CoveragePat<'db>>]) -> Vec<CoverageCtor<'db>> {
    let mut seen = Vec::new();
    for row in matrix {
        if let Some(CoveragePat::Ctor(ctor, _)) = row.first()
            && !seen.contains(ctor)
        {
            seen.push(ctor.clone());
        }
    }
    seen
}

fn recompose_ctor<'db>(
    ctor: CoverageCtor<'db>,
    field_count: usize,
    usefulness: Usefulness<'db>,
) -> Usefulness<'db> {
    match usefulness {
        Usefulness::Useful(mut witness) => {
            if witness.len() < field_count {
                return Usefulness::Unknown;
            }
            let rest = witness.split_off(field_count);
            let fields = witness;
            let mut row = Vec::with_capacity(rest.len() + 1);
            row.push(WitnessPat::Ctor(ctor, fields));
            row.extend(rest);
            Usefulness::Useful(row)
        }
        Usefulness::Useless => Usefulness::Useless,
        Usefulness::Unknown => Usefulness::Unknown,
    }
}

fn prepend_wild<'db>(usefulness: Usefulness<'db>) -> Usefulness<'db> {
    match usefulness {
        Usefulness::Useful(rest) => {
            let mut row = Vec::with_capacity(rest.len() + 1);
            row.push(WitnessPat::Wild);
            row.extend(rest);
            Usefulness::Useful(row)
        }
        Usefulness::Useless => Usefulness::Useless,
        Usefulness::Unknown => Usefulness::Unknown,
    }
}

fn witness_from_query<'db>(query: &[CoveragePat<'db>]) -> Vec<WitnessPat<'db>> {
    query
        .iter()
        .map(|pat| match pat {
            CoveragePat::Ctor(ctor, fields) => {
                WitnessPat::Ctor(ctor.clone(), witness_from_query(fields))
            }
            CoveragePat::Wild | CoveragePat::Literal(_) | CoveragePat::Opaque => WitnessPat::Wild,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TestTy {
        Bool,
        Pair(Box<TestTy>, Box<TestTy>),
        Word,
    }

    struct TestOracle;

    impl<'db> ConstructorOracle<'db, TestTy> for TestOracle {
        fn constructors(&mut self, ty: TestTy) -> Option<Vec<CoverageCtor<'db>>> {
            match ty {
                TestTy::Bool => Some(vec![builtin(BuiltinCoverageCtor::False), true_ctor()]),
                TestTy::Pair(_, _) => Some(vec![builtin(BuiltinCoverageCtor::Pair)]),
                TestTy::Word => None,
            }
        }

        fn fields(&mut self, ctor: &CoverageCtor<'db>, ty: TestTy) -> Option<Vec<TestTy>> {
            match (ctor, ty) {
                (
                    CoverageCtor::Builtin(BuiltinCoverageCtor::True)
                    | CoverageCtor::Builtin(BuiltinCoverageCtor::False),
                    TestTy::Bool,
                ) => Some(Vec::new()),
                (CoverageCtor::Builtin(BuiltinCoverageCtor::Pair), TestTy::Pair(lhs, rhs)) => {
                    Some(vec![*lhs, *rhs])
                }
                _ => None,
            }
        }
    }

    fn builtin<'db>(ctor: BuiltinCoverageCtor) -> CoverageCtor<'db> {
        CoverageCtor::Builtin(ctor)
    }

    fn true_ctor<'db>() -> CoverageCtor<'db> {
        builtin(BuiltinCoverageCtor::True)
    }

    fn false_ctor<'db>() -> CoverageCtor<'db> {
        builtin(BuiltinCoverageCtor::False)
    }

    fn true_pat<'db>() -> CoveragePat<'db> {
        CoveragePat::Ctor(true_ctor(), Vec::new())
    }

    fn false_pat<'db>() -> CoveragePat<'db> {
        CoveragePat::Ctor(false_ctor(), Vec::new())
    }

    fn pair_pat<'db>(lhs: CoveragePat<'db>, rhs: CoveragePat<'db>) -> CoveragePat<'db> {
        CoveragePat::Ctor(builtin(BuiltinCoverageCtor::Pair), vec![lhs, rhs])
    }

    #[test]
    fn exhaustive_bool_has_no_missing_witness() {
        let mut oracle = TestOracle;
        let analysis = analyze(
            &mut oracle,
            &[TestTy::Bool],
            &[vec![false_pat()], vec![true_pat()]],
        );
        assert_eq!(analysis.missing, None);
        assert!(analysis.unreachable.is_empty());
    }

    #[test]
    fn non_exhaustive_bool_reports_constructor_witness() {
        let mut oracle = TestOracle;
        let analysis = analyze(&mut oracle, &[TestTy::Bool], &[vec![true_pat()]]);
        assert_eq!(
            analysis.missing,
            Some(vec![WitnessPat::Ctor(false_ctor(), Vec::new())])
        );
        assert!(analysis.unreachable.is_empty());
    }

    #[test]
    fn wildcard_after_complete_bool_is_unreachable() {
        let mut oracle = TestOracle;
        let analysis = analyze(
            &mut oracle,
            &[TestTy::Bool],
            &[vec![false_pat()], vec![true_pat()], vec![CoveragePat::Wild]],
        );
        assert_eq!(analysis.missing, None);
        assert_eq!(analysis.unreachable, vec![2]);
    }

    #[test]
    fn duplicate_literal_is_unreachable_but_literals_do_not_exhaust_open_types() {
        let mut oracle = TestOracle;
        let analysis = analyze(
            &mut oracle,
            &[TestTy::Word],
            &[
                vec![CoveragePat::Literal("number:1".to_owned())],
                vec![CoveragePat::Literal("number:1".to_owned())],
            ],
        );
        assert_eq!(analysis.missing, Some(vec![WitnessPat::Wild]));
        assert_eq!(analysis.unreachable, vec![1]);
    }

    #[test]
    fn nested_constructor_witness_is_preserved() {
        let mut oracle = TestOracle;
        let ty = TestTy::Pair(Box::new(TestTy::Bool), Box::new(TestTy::Word));
        let analysis = analyze(
            &mut oracle,
            &[ty],
            &[vec![pair_pat(true_pat(), CoveragePat::Wild)]],
        );
        assert_eq!(
            analysis.missing,
            Some(vec![WitnessPat::Ctor(
                builtin(BuiltinCoverageCtor::Pair),
                vec![WitnessPat::Ctor(false_ctor(), Vec::new()), WitnessPat::Wild],
            )])
        );
        assert!(analysis.unreachable.is_empty());
    }
}
