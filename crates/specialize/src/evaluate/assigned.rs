use rustc_hash::FxHashSet;

use super::{CEnv, VEnv, known::collect_pat_binders};
use crate::ir::MonoPat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AssignedNames {
    Names(FxHashSet<String>),
    All,
}

impl AssignedNames {
    pub(super) fn empty() -> Self {
        AssignedNames::Names(FxHashSet::default())
    }

    pub(super) fn is_empty(&self) -> bool {
        matches!(self, AssignedNames::Names(names) if names.is_empty())
    }

    pub(super) fn insert(&mut self, name: String) {
        if let AssignedNames::Names(names) = self {
            names.insert(name);
        }
    }

    pub(super) fn merge(&mut self, other: AssignedNames) {
        match (self, other) {
            (this @ AssignedNames::Names(_), AssignedNames::All) => *this = AssignedNames::All,
            (AssignedNames::All, _) => {}
            (AssignedNames::Names(lhs), AssignedNames::Names(rhs)) => lhs.extend(rhs),
        }
    }

    pub(super) fn insert_pat_binders(&mut self, pats: &[MonoPat<'_>]) {
        if let AssignedNames::Names(names) = self {
            for pat in pats {
                collect_pat_binders(pat, names);
            }
        }
    }
}

pub(super) fn invalidate_assigned<'db>(
    names: &AssignedNames,
    env: &mut VEnv<'db>,
    comptime_env: &mut CEnv,
) {
    match names {
        AssignedNames::All => {
            env.clear();
            comptime_env.clear();
        }
        AssignedNames::Names(names) => {
            for name in names {
                env.remove(name);
                comptime_env.remove(name);
            }
        }
    }
}
