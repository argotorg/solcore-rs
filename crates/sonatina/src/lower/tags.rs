//! The visible source bindings around each instruction that lowering stamped.

use std::collections::HashMap;

use sonatina_ir::{InstId, module::FuncRef};

use crate::locals::{Frame, LocalsTag};

/// A set of visible bindings, as the id [`DebugTags::intern`] gave it.
pub(super) type SetId = u32;

/// What lowering recorded about one instruction.
#[derive(Debug, Clone)]
pub(super) struct InstTag {
    pub(super) before: SetId,
    pub(super) after: SetId,
    pub(super) frame: Frame,
    /// The value label of each binding of `before`, in the order of the set, or
    /// empty before lowering of the function has finished. See `Labeller`.
    pub(super) before_labels: Vec<Option<u32>>,
    /// Likewise for `after`.
    pub(super) after_labels: Vec<Option<u32>>,
}

/// The visible binding sets of a module and the tag of every instruction that
/// has an origin. Binding sets are interned: lowering compares and copies ids.
#[derive(Debug, Default)]
pub(crate) struct DebugTags {
    sets: Vec<Vec<u32>>,
    ids: HashMap<Vec<u32>, SetId>,
    pub(super) insts: HashMap<(FuncRef, InstId), InstTag>,
}

impl DebugTags {
    /// The id of the set of bindings `set`, which lists binder ids in the order
    /// they were bound.
    pub(super) fn intern(&mut self, set: &[u32]) -> SetId {
        if let Some(id) = self.ids.get(set) {
            return *id;
        }
        let id = self.sets.len() as SetId;
        self.sets.push(set.to_vec());
        self.ids.insert(set.to_vec(), id);
        id
    }

    /// The binder ids of an interned set.
    pub(super) fn set(&self, id: SetId) -> &[u32] {
        &self.sets[id as usize]
    }

    /// The tag of `inst` of `func`, when lowering gave the instruction one.
    pub(crate) fn tag(&self, func: FuncRef, inst: InstId) -> Option<LocalsTag> {
        let tag = self.insts.get(&(func, inst))?;
        let before = self.sets[tag.before as usize].clone();
        let after = self.sets[tag.after as usize].clone();
        let labels = |labels: &[Option<u32>], ids: &[u32]| {
            if labels.len() == ids.len() {
                labels.to_vec()
            } else {
                vec![None; ids.len()]
            }
        };
        Some(LocalsTag {
            inst: inst.0,
            before_labels: labels(&tag.before_labels, &before),
            after_labels: labels(&tag.after_labels, &after),
            before,
            after,
            frame: tag.frame,
        })
    }
}
