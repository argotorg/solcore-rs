use super::*;

/// A method instantiation needs both its receiver dictionary and the
/// independently selected dictionaries for the method's own constraints.
#[derive(Debug, Clone)]
pub(super) struct MethodEvidence<'db> {
    pub dictionary: Evidence<'db>,
    pub bindings: Vec<(Pred<'db>, Evidence<'db>)>,
}

impl<'db> From<Evidence<'db>> for MethodEvidence<'db> {
    fn from(dictionary: Evidence<'db>) -> Self {
        Self {
            dictionary,
            bindings: Vec::new(),
        }
    }
}

/// An executable dictionary retains the environment in which each implicit
/// superclass was selected. Solver proofs only describe declared prerequisites.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Evidence<'db> {
    Instance {
        instance: DefId<'db>,
        args: Vec<Ty<'db>>,
        sub_evidence: Vec<Self>,
        superclasses: Vec<(Pred<'db>, SuperclassRecipe<'db>)>,
    },
    Builtin {
        pred: Pred<'db>,
    },
    /// A failed selection is still a stable dictionary value. Keeping it
    /// deferred lets methods that do not use the superclass remain executable.
    Unavailable {
        pred: Pred<'db>,
        failure: SuperclassFailure,
        // A cyclic hierarchy may still project back to this executable child.
        source: Box<Self>,
    },
    Superclass {
        class: DefId<'db>,
        pred: Pred<'db>,
        child: Box<Self>,
    },
    Derived {
        kind: DerivedClauseKind<'db>,
        pred: Pred<'db>,
        sub_evidence: Vec<Self>,
        superclasses: Vec<(Pred<'db>, SuperclassRecipe<'db>)>,
    },
}

/// A selected proof is elaborated only when its superclass is projected. This
/// keeps cyclic class hierarchies finite without discarding dictionary identity.
/// Only givens actually referenced by the proof are captured.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct SuperclassRecipe<'db> {
    pub module: Module<'db>,
    pub selection: Result<LogicalEvidence<'db>, SuperclassFailure>,
    /// Givens of the selection that are prerequisites of the dictionary that
    /// holds this recipe, with their position among them. A copy here would
    /// double the dictionary with every level of a conditional impl.
    pub prerequisites: Vec<(Pred<'db>, usize)>,
    /// Givens of the selection that come from the function the dictionary
    /// was built in.
    pub bindings: Vec<(Pred<'db>, Evidence<'db>)>,
}

impl<'db> SuperclassRecipe<'db> {
    /// The givens of the selection, with the dictionaries that
    /// `prerequisites`, those of the dictionary that holds this recipe,
    /// provide.
    pub(super) fn givens(
        &self,
        prerequisites: &[Evidence<'db>],
    ) -> Vec<(Pred<'db>, Evidence<'db>)> {
        self.prerequisites
            .iter()
            .map(|(pred, position)| (*pred, prerequisites[*position].clone()))
            .chain(self.bindings.iter().cloned())
            .collect()
    }
}

/// Predicates followed from one given through the superclasses their classes
/// declare. A superclass that grows its type argument has no finite closure;
/// class hierarchies in practice stay far below the bound.
const MAX_SUPERCLASS_CLOSURE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum SuperclassFailure {
    Exhausted,
    NoUniqueDictionary,
    OpenDictionary,
    CyclicDictionary,
    ProjectionDepthExceeded,
}

impl SuperclassFailure {
    pub fn description(self) -> &'static str {
        match self {
            Self::Exhausted => "trait solver exhausted",
            Self::NoUniqueDictionary => "no unique dictionary",
            Self::OpenDictionary => "trait solver returned an open dictionary",
            Self::CyclicDictionary => "cyclic superclass has no executable dictionary",
            Self::ProjectionDepthExceeded => "superclass projection depth exceeded",
        }
    }
}

impl<'db> Driver<'db> {
    pub(super) fn elaborate_evidence(
        &mut self,
        module: Module<'db>,
        proof: LogicalEvidence<'db>,
        bindings: &[(Pred<'db>, Evidence<'db>)],
    ) -> Evidence<'db> {
        self.elaborate_dictionary(module, proof, bindings, true)
    }

    /// Recipe expansion shares the projection caller's cycle guard, so it must
    /// not start another normalization traversal while building its proof.
    pub(super) fn elaborate_dictionary(
        &mut self,
        module: Module<'db>,
        proof: LogicalEvidence<'db>,
        bindings: &[(Pred<'db>, Evidence<'db>)],
        normalize_projections: bool,
    ) -> Evidence<'db> {
        let mut dictionary = match proof {
            LogicalEvidence::Builtin { pred } => {
                let dictionary = bindings
                    .iter()
                    .find_map(|(given, evidence)| (*given == pred).then(|| evidence.clone()))
                    .unwrap_or(Evidence::Builtin { pred });
                return self.normalize_dictionary(dictionary, normalize_projections);
            }
            LogicalEvidence::Superclass { class, pred, child } => {
                // No given is the dictionary of a goal that matched it only
                // up to `comptime`. The impl is selected where the goal is
                // used, as it is for such an assumption itself.
                if normalize_projections && self.assumes_requalified_given(&child, bindings) {
                    return Evidence::Builtin { pred };
                }
                let child =
                    self.elaborate_dictionary(module, *child, bindings, normalize_projections);
                return self.normalize_dictionary(
                    Evidence::Superclass {
                        class,
                        pred,
                        child: Box::new(child),
                    },
                    normalize_projections,
                );
            }
            LogicalEvidence::Instance {
                instance,
                args,
                sub_evidence,
            } => Evidence::Instance {
                instance,
                args,
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|proof| {
                        self.elaborate_dictionary(module, proof, bindings, normalize_projections)
                    })
                    .collect(),
                superclasses: Vec::new(),
            },
            LogicalEvidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => Evidence::Derived {
                kind,
                pred,
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|proof| {
                        self.elaborate_dictionary(module, proof, bindings, normalize_projections)
                    })
                    .collect(),
                superclasses: Vec::new(),
            },
        };
        let Some(head) = self.dictionary_predicate(&dictionary) else {
            return dictionary;
        };
        let prerequisites = match &dictionary {
            Evidence::Instance { sub_evidence, .. } | Evidence::Derived { sub_evidence, .. } => {
                sub_evidence
            }
            _ => unreachable!(),
        };
        // The position of each prerequisite binding among the prerequisites.
        let (prerequisite_positions, mut local_bindings): (Vec<_>, Vec<_>) = prerequisites
            .iter()
            .enumerate()
            .filter_map(|(position, evidence)| {
                self.dictionary_predicate(evidence)
                    .map(|pred| (position, (pred, evidence.clone())))
            })
            .unzip();
        let prerequisites_len = local_bindings.len();
        for (pred, evidence) in bindings {
            if !local_bindings.iter().any(|(given, _)| given == pred) {
                local_bindings.push((*pred, evidence.clone()));
            }
        }
        // The predicates that two of the givens reach, found when a proof
        // first starts at a given.
        let mut shared = None;
        let mut superclasses = Vec::new();
        let Some(base) = self.try_module_trait_env(module) else {
            self.push_missing_module_trait_env(None);
            return dictionary;
        };
        for pred in hir_ty::direct_superclass_predicates(self.db, base.base(self.db), head) {
            // Explicit prerequisites already carry their selected dictionaries.
            if prerequisites
                .iter()
                .any(|evidence| self.dictionary_predicate(evidence) == Some(pred))
            {
                continue;
            }
            let selection = self.select_superclass(
                module,
                base,
                pred,
                &local_bindings,
                prerequisites_len,
                &mut shared,
                normalize_projections,
            );
            let mut referenced = FxHashSet::default();
            if let Ok(proof) = &selection {
                collect_given_predicates(proof, &mut referenced);
            }
            let mut recipe_prerequisites = Vec::new();
            let mut bindings = Vec::new();
            for (index, (pred, evidence)) in local_bindings.iter().enumerate() {
                if !referenced.contains(pred) {
                    continue;
                }
                match prerequisite_positions.get(index) {
                    Some(position) => recipe_prerequisites.push((*pred, *position)),
                    None => bindings.push((
                        *pred,
                        self.normalize_dictionary(evidence.clone(), normalize_projections),
                    )),
                }
            }
            superclasses.push((
                pred,
                SuperclassRecipe {
                    module,
                    selection,
                    prerequisites: recipe_prerequisites,
                    bindings,
                },
            ));
        }
        match &mut dictionary {
            Evidence::Instance {
                superclasses: captured,
                ..
            }
            | Evidence::Derived {
                superclasses: captured,
                ..
            } => *captured = superclasses,
            _ => unreachable!(),
        }
        dictionary
    }

    /// Whether `proof` starts at an assumption that no binding provides as
    /// it is spelled, while one does up to `comptime`.
    fn assumes_requalified_given(
        &self,
        proof: &LogicalEvidence<'db>,
        bindings: &[(Pred<'db>, Evidence<'db>)],
    ) -> bool {
        let assumed = match proof {
            LogicalEvidence::Builtin { pred } => *pred,
            LogicalEvidence::Superclass { child, .. } => {
                return self.assumes_requalified_given(child, bindings);
            }
            LogicalEvidence::Instance { .. } | LogicalEvidence::Derived { .. } => return false,
        };
        !bindings.iter().any(|(given, _)| *given == assumed)
            && bindings
                .iter()
                .any(|(given, _)| preds_equal_up_to_comptime(self.db, *given, assumed))
    }

    /// Selects one superclass of a new dictionary under `givens`: its
    /// prerequisites, which are the first `prerequisites_len` of them, and
    /// the dictionaries of the enclosing function.
    ///
    /// A dictionary that holds one of the enclosing function differs with
    /// the function it is built in, and the specializations of its methods
    /// multiply with it. So the proof from the prerequisites alone is kept
    /// where it builds the same dictionary, or where the enclosing function's
    /// dictionaries turn out not to hold the superclass.
    ///
    /// Two givens that reach one predicate used to leave a superclass that
    /// needs it without a unique dictionary. Now the proofs that start at
    /// different givens are one dictionary, and the proof that is found
    /// starts at whichever given inference met first. The prerequisites
    /// alone decide such a superclass.
    #[allow(clippy::too_many_arguments)]
    fn select_superclass(
        &mut self,
        module: Module<'db>,
        base: hir_ty::TraitEnvId<'db>,
        pred: Pred<'db>,
        givens: &[(Pred<'db>, Evidence<'db>)],
        prerequisites_len: usize,
        shared: &mut Option<FxHashSet<Pred<'db>>>,
        normalize_projections: bool,
    ) -> Result<LogicalEvidence<'db>, SuperclassFailure> {
        let proof = self.solve_superclass(base, pred, givens)?;
        // A recipe is expanded with the givens it captured, as it was
        // selected, and prerequisites are part of the dictionary anyway.
        if !normalize_projections || givens.len() == prerequisites_len {
            return Ok(proof);
        }
        let mut rooted = Vec::new();
        collect_given_rooted_predicates(&proof, &mut rooted);
        if rooted.is_empty() {
            return Ok(proof);
        }
        let shared = shared.get_or_insert_with(|| self.shared_superclasses(module, givens));
        if rooted.iter().any(|pred| shared.contains(pred)) {
            return self
                .solve_superclass(base, pred, &givens[..prerequisites_len])
                .map_err(|_| SuperclassFailure::NoUniqueDictionary);
        }
        let mut named = FxHashSet::default();
        collect_given_predicates(&proof, &mut named);
        if !givens[prerequisites_len..]
            .iter()
            .any(|(given, _)| named.contains(given))
        {
            return Ok(proof);
        }
        let Ok(own) = self.solve_superclass(base, pred, &givens[..prerequisites_len]) else {
            return Ok(proof);
        };
        let canonical = self.recipe_dictionary(module, &own, &givens[..prerequisites_len]);
        let enclosing = self.recipe_dictionary(module, &proof, givens);
        if self.has_no_dictionary(&enclosing) || self.same_dictionary(&canonical, &enclosing) {
            return Ok(own);
        }
        Ok(proof)
    }

    /// The dictionary that a recipe with `proof`, selected under `givens`,
    /// expands to.
    fn recipe_dictionary(
        &mut self,
        module: Module<'db>,
        proof: &LogicalEvidence<'db>,
        givens: &[(Pred<'db>, Evidence<'db>)],
    ) -> Evidence<'db> {
        let mut referenced = FxHashSet::default();
        collect_given_predicates(proof, &mut referenced);
        let bindings = givens
            .iter()
            .filter(|(pred, _)| referenced.contains(pred))
            .map(|(pred, evidence)| (*pred, self.normalize_dictionary(evidence.clone(), true)))
            .collect::<Vec<_>>();
        self.elaborate_dictionary(module, proof.clone(), &bindings, false)
    }

    /// Solves one superclass of a dictionary under `givens`.
    fn solve_superclass(
        &mut self,
        base: hir_ty::TraitEnvId<'db>,
        pred: Pred<'db>,
        givens: &[(Pred<'db>, Evidence<'db>)],
    ) -> Result<LogicalEvidence<'db>, SuperclassFailure> {
        let env = trait_env_with_givens(
            self.db,
            base,
            givens.iter().map(|(pred, _)| *pred).collect(),
        );
        let report = hir_ty::solve_report(self.db, env, canonical_goal(self.db, pred));
        if report.exhausted {
            return Err(SuperclassFailure::Exhausted);
        }
        match report.solution {
            Solution::Unique {
                evidence: proof,
                subst,
            } => {
                // Specialization has closed inputs: fresh answer variables
                // cannot identify an executable dictionary.
                if logical_evidence_is_closed(self.db, &proof)
                    && subst
                        .values
                        .iter()
                        .all(|(_, ty)| ty_is_closed(self.db, *ty))
                {
                    Ok(proof)
                } else {
                    Err(SuperclassFailure::OpenDictionary)
                }
            }
            _ => Err(SuperclassFailure::NoUniqueDictionary),
        }
    }

    /// The predicates that two of `givens` reach through the superclasses
    /// that `module` sees: the ones for which inference there found proofs
    /// that start at different givens.
    fn shared_superclasses(
        &self,
        module: Module<'db>,
        givens: &[(Pred<'db>, Evidence<'db>)],
    ) -> FxHashSet<Pred<'db>> {
        let mut shared = FxHashSet::default();
        let Some(env) = self.try_module_trait_env(module) else {
            return shared;
        };
        // The given that reached each predicate first.
        let mut reached_from = FxHashMap::<Pred<'db>, usize>::default();
        for (index, (given, _)) in givens.iter().enumerate() {
            let mut reached = FxHashSet::default();
            let mut pending = VecDeque::from([*given]);
            // The walk is breadth first, so a superclass that grows its type
            // argument does not use up the bound.
            while let Some(pred) = pending.pop_front() {
                if reached.len() >= MAX_SUPERCLASS_CLOSURE || !reached.insert(pred) {
                    continue;
                }
                if *reached_from.entry(pred).or_insert(index) != index {
                    shared.insert(pred);
                }
                pending.extend(hir_ty::direct_superclass_predicates(
                    self.db,
                    env.base(self.db),
                    pred,
                ));
            }
        }
        shared
    }

    fn normalize_dictionary(
        &mut self,
        dictionary: Evidence<'db>,
        normalize: bool,
    ) -> Evidence<'db> {
        if normalize && let Evidence::Superclass { pred, child, .. } = &dictionary {
            // Equivalent projections must have the same specialization key,
            // including when recursive generic calls traverse a class cycle.
            return self.superclass_instance_evidence(*pred, child).unwrap_or(
                Evidence::Unavailable {
                    pred: *pred,
                    failure: SuperclassFailure::NoUniqueDictionary,
                    source: child.clone(),
                },
            );
        }
        dictionary
    }

    pub(super) fn dictionary_predicate(&self, evidence: &Evidence<'db>) -> Option<Pred<'db>> {
        Some(match evidence {
            Evidence::Instance { instance, args, .. } => {
                let info = self.instances.get(instance)?;
                TySubst::from_args(args.clone()).apply_pred(self.db, info.head)
            }
            Evidence::Builtin { pred }
            | Evidence::Unavailable { pred, .. }
            | Evidence::Superclass { pred, .. }
            | Evidence::Derived { pred, .. } => *pred,
        })
    }
}

fn logical_evidence_is_closed<'db>(db: &'db dyn Db, evidence: &LogicalEvidence<'db>) -> bool {
    match evidence {
        LogicalEvidence::Instance {
            args, sub_evidence, ..
        } => {
            args.iter().all(|ty| ty_is_closed(db, *ty))
                && sub_evidence
                    .iter()
                    .all(|child| logical_evidence_is_closed(db, child))
        }
        LogicalEvidence::Derived {
            pred, sub_evidence, ..
        } => {
            pred_is_closed(db, *pred)
                && sub_evidence
                    .iter()
                    .all(|child| logical_evidence_is_closed(db, child))
        }
        LogicalEvidence::Builtin { pred } => pred_is_closed(db, *pred),
        LogicalEvidence::Superclass { pred, child, .. } => {
            pred_is_closed(db, *pred) && logical_evidence_is_closed(db, child)
        }
    }
}

/// The predicates of the dictionaries that `proof` takes from assumptions:
/// each assumption it uses, or the superclass it projects from one.
fn collect_given_rooted_predicates<'db>(proof: &LogicalEvidence<'db>, rooted: &mut Vec<Pred<'db>>) {
    match proof {
        LogicalEvidence::Builtin { pred } | LogicalEvidence::Superclass { pred, .. } => {
            rooted.push(*pred);
        }
        LogicalEvidence::Instance { sub_evidence, .. }
        | LogicalEvidence::Derived { sub_evidence, .. } => {
            for proof in sub_evidence {
                collect_given_rooted_predicates(proof, rooted);
            }
        }
    }
}

fn collect_given_predicates<'db>(
    proof: &LogicalEvidence<'db>,
    referenced: &mut FxHashSet<Pred<'db>>,
) {
    match proof {
        LogicalEvidence::Builtin { pred } => {
            referenced.insert(*pred);
        }
        LogicalEvidence::Superclass { child, .. } => collect_given_predicates(child, referenced),
        LogicalEvidence::Instance { sub_evidence, .. }
        | LogicalEvidence::Derived { sub_evidence, .. } => {
            for proof in sub_evidence {
                collect_given_predicates(proof, referenced);
            }
        }
    }
}
