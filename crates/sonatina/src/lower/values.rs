//! Which IR value holds a source binding at each stamped instruction.
//!
//! Debug information at optimization level 0 can give a parameter or a local a
//! stack location only if the backend knows which machine value it is. Lowering
//! tells it by putting a value label (`sonatina_ir::ValueLabel`) on the IR
//! value that holds the binding, and by recording in each instruction's locals
//! tag which label belongs to which binding while the instruction runs and once
//! it has completed. A label is a plain number, and the pair (binding, value)
//! has one number for the whole module, so a binding that was reassigned has a
//! different label for each value it held and the tag of an instruction names
//! only the label of the value that is current at that instruction.
//!
//! Finding the current value must not change the IR. The SSA builder's
//! `use_var` can insert a phi, so it is never asked for this purpose.
//! Lowering already calls it and `def_var` for the program's own reads and
//! assignments, and this module only records what they returned: for each
//! (block, variable) the values in order, with the number of instructions that
//! existed at that time. The value of a binding at an instruction is then the
//! last one recorded in the instruction's block before the instruction was
//! made. A block that has not defined or read the variable has no record and
//! takes the end state of its predecessor when it has exactly one, which is
//! what the SSA builder itself does for such a block. Anything else (a join,
//! or a loop head that nothing in the block has read yet) has no known value,
//! and the binding gets no label there. Labels are added to values just before
//! the builder seals its blocks, so a phi that the builder removes as trivial
//! passes its labels to the value that replaces it.

use std::collections::{HashMap, HashSet};

use sonatina_ir::{BlockId, InstId, Value, ValueId, ValueLabel, builder::Variable};

use super::FunctionBuilderOf;
use super::tags::DebugTags;

/// A record of one `def_var` or `use_var`.
#[derive(Debug, Clone, Copy)]
struct Seen {
    /// The number of instructions that existed when it happened. An instruction
    /// with an index below this was made before it.
    at: u32,
    value: ValueId,
}

#[derive(Default)]
pub(super) struct ValueTrace {
    seen: HashMap<(BlockId, Variable), Vec<Seen>>,
    /// The variable of each source binding that holds one word.
    variables: HashMap<u32, Variable>,
    /// The label of each (binding, value) already given one.
    labels: HashMap<(u32, ValueId), u32>,
}

impl ValueTrace {
    pub(super) fn note(&mut self, block: BlockId, var: Variable, at: usize, value: ValueId) {
        self.seen.entry((block, var)).or_default().push(Seen {
            at: at as u32,
            value,
        });
    }

    /// Declares that the source binding `binder` lives in `var`.
    pub(super) fn bind(&mut self, binder: u32, var: Variable) {
        self.variables.insert(binder, var);
    }

    /// Puts labels on the values that hold bindings and fills the label lists of
    /// every tag of `func`. `next_label` is the module's counter.
    pub(super) fn label(
        &mut self,
        fb: &mut FunctionBuilderOf,
        tags: &mut DebugTags,
        func: sonatina_ir::module::FuncRef,
        next_label: &mut u32,
    ) {
        if self.variables.is_empty() {
            return;
        }
        let predecessors = predecessors(fb);
        let mut insts: Vec<InstId> = tags
            .insts
            .keys()
            .filter(|(owner, _)| *owner == func)
            .map(|(_, inst)| *inst)
            .collect();
        insts.sort_by_key(|inst| inst.0);
        for inst in insts {
            let block = fb.func.layout.inst_block(inst);
            let (before_set, after_set) = {
                let tag = &tags.insts[&(func, inst)];
                (tags.set(tag.before).to_vec(), tags.set(tag.after).to_vec())
            };
            let mut side = |binders: &[u32], at: u32| -> Vec<Option<u32>> {
                binders
                    .iter()
                    .map(|&binder| {
                        let var = *self.variables.get(&binder)?;
                        let value = self.value_at(&predecessors, block, var, at)?;
                        if !matches!(
                            fb.func.dfg.value(value),
                            Value::Inst { .. } | Value::Arg { .. }
                        ) {
                            return None;
                        }
                        let label = *self.labels.entry((binder, value)).or_insert_with(|| {
                            let label = *next_label;
                            *next_label += 1;
                            fb.func.dfg.add_value_label(value, ValueLabel(label));
                            label
                        });
                        Some(label)
                    })
                    .collect()
            };
            // Before the instruction runs: what was recorded before it was made.
            // Once it has completed: also what was recorded right after it,
            // before the next instruction, which is where an assignment that
            // uses its result defines the variable.
            let before = side(&before_set, inst.0);
            let after = side(&after_set, inst.0 + 1);
            let tag = tags.insts.get_mut(&(func, inst)).expect("tag exists");
            tag.before_labels = before;
            tag.after_labels = after;
        }
    }

    /// The value of `var` in `block` among the records made before instruction
    /// index `at`, or, when the block has none, at the end of its only
    /// predecessor.
    fn value_at(
        &self,
        predecessors: &HashMap<BlockId, Vec<BlockId>>,
        block: BlockId,
        var: Variable,
        at: u32,
    ) -> Option<ValueId> {
        let mut block = block;
        let mut at = at;
        let mut visited = HashSet::new();
        loop {
            if let Some(seen) = self
                .seen
                .get(&(block, var))
                .and_then(|seen| seen.iter().rev().find(|seen| seen.at <= at))
            {
                return Some(seen.value);
            }
            if !visited.insert(block) {
                return None;
            }
            match predecessors.get(&block).map(Vec::as_slice) {
                Some(&[only]) if only != block => {
                    block = only;
                    at = u32::MAX;
                }
                _ => return None,
            }
        }
    }
}

/// The distinct predecessors of every block, from the terminators made so far.
fn predecessors(fb: &FunctionBuilderOf) -> HashMap<BlockId, Vec<BlockId>> {
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    // The entry block is also entered from outside, so it never has just one
    // predecessor.
    if let Some(entry) = fb.func.layout.entry_block() {
        preds.entry(entry).or_default().push(entry);
    }
    for block in fb.func.layout.iter_block() {
        let Some(last) = fb.func.layout.last_inst_of(block) else {
            continue;
        };
        let Some(info) = fb.func.dfg.branch_info(last) else {
            continue;
        };
        for dest in info.dests() {
            let list = preds.entry(dest).or_default();
            if !list.contains(&block) {
                list.push(block);
            }
        }
    }
    preds
}
