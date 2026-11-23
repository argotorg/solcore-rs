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
    pub bindings: Vec<(Pred<'db>, Evidence<'db>)>,
}

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
        let local_bindings = prerequisites
            .iter()
            .filter_map(|evidence| {
                self.dictionary_predicate(evidence)
                    .map(|pred| (pred, evidence.clone()))
            })
            .collect::<Vec<_>>();
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
            let env = trait_env_with_givens(
                self.db,
                base,
                local_bindings.iter().map(|(pred, _)| *pred).collect(),
            );
            let report = hir_ty::solve_report(self.db, env, canonical_goal(self.db, pred));
            let selection = if report.exhausted {
                Err(SuperclassFailure::Exhausted)
            } else {
                match report.solution {
                    Solution::Unique {
                        evidence: proof,
                        subst,
                    } => {
                        // Specialization has closed inputs: fresh answer
                        // variables cannot identify an executable dictionary.
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
            };
            let mut referenced = FxHashSet::default();
            if let Ok(proof) = &selection {
                collect_given_predicates(proof, &mut referenced);
            }
            let bindings = local_bindings
                .iter()
                .filter(|(pred, _)| referenced.contains(pred))
                .map(|(pred, evidence)| {
                    (
                        *pred,
                        self.normalize_dictionary(evidence.clone(), normalize_projections),
                    )
                })
                .collect();
            superclasses.push((
                pred,
                SuperclassRecipe {
                    module,
                    selection,
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
