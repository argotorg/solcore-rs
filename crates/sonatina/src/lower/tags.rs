//! The visible source bindings around each instruction that lowering stamped.

use std::collections::HashMap;

use sonatina_ir::{InstId, module::FuncRef};

use crate::locals::{Frame, LocalsTag};

/// A set of visible bindings, as the id [`DebugTags::intern`] gave it.
pub(super) type SetId = u32;

/// What lowering recorded about one instruction.
#[derive(Debug, Clone, Copy)]
pub(super) struct InstTag {
    pub(super) before: SetId,
    pub(super) after: SetId,
    pub(super) frame: Frame,
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

    /// The tag of `inst` of `func`, when lowering gave the instruction one.
    pub(crate) fn tag(&self, func: FuncRef, inst: InstId) -> Option<LocalsTag> {
        let tag = self.insts.get(&(func, inst))?;
        Some(LocalsTag {
            inst: inst.0,
            before: self.sets[tag.before as usize].clone(),
            after: self.sets[tag.after as usize].clone(),
            frame: tag.frame,
        })
    }
}
