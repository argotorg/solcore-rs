//! Where a labelled value sits on the EVM stack after each opcode, from the
//! operand stack plan that Sonatina exports.
//!
//! This is the one place that reads the plan. A plan record is the stack before
//! the first opcode of a machine instruction, top first, with the labels of the
//! values on it. For the opcodes of the instruction this module needs the stack
//! after each of them, which is what an ethdebug context describes: the context
//! of an opcode holds once the opcode has run.
//!
//! The stack after an opcode is found by replaying the opcodes the instruction
//! actually emitted, starting from its record. `PUSH`, `DUP`, `SWAP` and `POP`
//! act as the EVM defines, and any other opcode takes and gives the words its
//! stack effect says; the words it gives are results, and the record of the
//! next instruction of the block says which values they are. The stack after
//! the last opcode of an instruction is that next record.
//!
//! Nothing is guessed. An instruction gets no stack at all unless
//!
//! - it has a plan record, and its opcodes form one run in the code (the
//!   emitter's peephole pass can move an opcode of one instruction in front of
//!   the opcodes of the one before it, which makes the records of both
//!   unusable);
//! - an instruction of the same block follows directly and has a usable record
//!   as well;
//! - the replay never needs a word below the planned stack and meets only
//!   opcodes with a known stack effect, and what it ends with is the next
//!   record: the same number of words, each the same value (a constant matches a
//!   constant, a result matches a value that is not a constant). If the replay
//!   disagrees with the next record, the record was shifted and nothing the
//!   instruction says is used;
//! - an instruction that calls a function, which is a run that ends in `JUMP`
//!   and the `JUMPDEST` the callee returns to, lists the stack after the call as
//!   the next record. The export does not say how many arguments the callee takes
//!   or how many results it gives, so the check compares what the call cannot
//!   have changed: every word below the continuation (the address the run
//!   pushes for the callee to return to) must be the same value at the bottom of
//!   the next record. The depth of the continuation is read from the opcodes: it
//!   is the depth of the swap that puts it below the arguments, or 0 when the
//!   call has no argument and the continuation is pushed right before the
//!   callee's address. A call whose continuation cannot be found that way, which
//!   has no word below the continuation, or whose next record is shorter than
//!   the words below it or differs from them, gets no stack at all, for any of
//!   its opcodes;
//!
//! The opcodes that transfer control (`JUMP` and `JUMPI`, `RETURN` and the
//! rest) have no stack here, and so has any instruction that ends a block.

use std::collections::{BTreeMap, HashMap};

use sonatina_codegen::{
    isa::evm::stack_plan::{InstStack, StackEntry, StackPlan},
    object::MachineInstId,
};
use sonatina_ir::{InstId, ValueLabel};

use crate::{CompiledSection, PcRange};

const STOP: u8 = 0x00;
const POP: u8 = 0x50;
const JUMP: u8 = 0x56;
const JUMPI: u8 = 0x57;
const JUMPDEST: u8 = 0x5b;
const PUSH0: u8 = 0x5f;
const PUSH32: u8 = 0x7f;
const DUP1: u8 = 0x80;
const DUP16: u8 = 0x8f;
const SWAP1: u8 = 0x90;
const SWAP16: u8 = 0x9f;
const RETURN: u8 = 0xf3;
const REVERT: u8 = 0xfd;
const INVALID: u8 = 0xfe;
const SELFDESTRUCT: u8 = 0xff;

/// An opcode of the code region and the range of the byte map that holds it.
pub(super) struct Opcode<'a> {
    pub byte: u8,
    pub range: &'a PcRange,
}

/// One word of the stack after an opcode.
#[derive(Clone, Copy, Debug)]
pub(super) struct Cell<'a> {
    /// The labels of the value that the word holds. Empty for a word that is
    /// not a planned value.
    labels: &'a [ValueLabel],
    /// Nothing from the instruction on reads the value, though the word is still
    /// on the stack.
    dead: bool,
}

impl<'a> Cell<'a> {
    const OTHER: Cell<'static> = Cell {
        labels: &[],
        dead: false,
    };

    fn of(entry: &'a StackEntry) -> Self {
        match entry {
            StackEntry::Value(value) => Cell {
                labels: &value.labels,
                dead: value.dead,
            },
            StackEntry::ReturnAddress | StackEntry::CallContinuation => Cell::OTHER,
        }
    }
}

/// The stack after each opcode, by the index of the opcode, and what was found.
pub(super) struct Stacks<'a> {
    /// `None` where no stack is known, which is also where it is refused.
    pub after: Vec<Option<Vec<Cell<'a>>>>,
    /// Runs of opcodes of one machine instruction in a function.
    pub instructions: usize,
    /// The ones whose opcodes have a stack.
    pub usable: usize,
    /// How many of the others were refused, by the reason.
    pub refused: BTreeMap<&'static str, usize>,
}

/// A word of a replayed stack.
#[derive(Clone, Copy)]
enum Word<'a> {
    /// An entry of the record the replay started from.
    Planned(&'a StackEntry),
    /// A constant that an emitted `PUSH` made.
    Pushed,
    /// What an opcode produced.
    Result,
}

impl<'a> Word<'a> {
    fn cell(self) -> Cell<'a> {
        match self {
            Word::Planned(entry) => Cell::of(entry),
            Word::Pushed | Word::Result => Cell::OTHER,
        }
    }

    /// Whether the word is the word that `entry` of the next record lists.
    fn agrees_with(self, entry: &StackEntry) -> bool {
        match (self, entry) {
            (Word::Planned(StackEntry::Value(have)), StackEntry::Value(want)) => {
                have.value == want.value || (have.constant && want.constant)
            }
            (Word::Planned(StackEntry::ReturnAddress), StackEntry::ReturnAddress) => true,
            (Word::Pushed, StackEntry::Value(want)) => want.constant,
            (Word::Result, StackEntry::Value(want)) => !want.constant,
            _ => false,
        }
    }
}

/// The words an opcode takes from the stack and the words it puts on it, for
/// the opcodes that are not handled apart. `None` for an opcode whose effect
/// is not known here.
fn effect(byte: u8) -> Option<(usize, usize)> {
    Some(match byte {
        // Arithmetic, comparison and bit operations.
        0x01..=0x07 | 0x0a | 0x0b | 0x10..=0x12 | 0x13..=0x14 | 0x16..=0x18 => (2, 1),
        0x08 | 0x09 => (3, 1),
        0x15 | 0x19 | 0x1e => (1, 1),
        0x1a..=0x1d => (2, 1),
        0x20 => (2, 1),
        // The environment.
        0x30 | 0x32..=0x34 | 0x36 | 0x38 | 0x3a | 0x3d | 0x41..=0x48 | 0x4a => (0, 1),
        0x31 | 0x35 | 0x3b | 0x3f | 0x40 | 0x49 => (1, 1),
        0x37 | 0x39 | 0x3e => (3, 0),
        0x3c => (4, 0),
        // Memory, storage and transient storage.
        0x51 | 0x54 | 0x5c => (1, 1),
        0x52 | 0x53 | 0x55 | 0x5d => (2, 0),
        0x5e => (3, 0),
        0x58..=0x5a => (0, 1),
        // Logs.
        0xa0..=0xa4 => (2 + usize::from(byte - 0xa0), 0),
        // Creating and calling.
        0xf0 => (3, 1),
        0xf1 | 0xf2 => (7, 1),
        0xf4 | 0xfa => (6, 1),
        0xf5 => (4, 1),
        _ => return None,
    })
}

/// The stack after each of `ops`, which are the opcodes of the code region in
/// order.
pub(super) fn stacks_after<'a>(ops: &[Opcode<'_>], section: &'a CompiledSection) -> Stacks<'a> {
    let plans: HashMap<u32, &StackPlan> = section
        .stack_plans
        .iter()
        .map(|function| (function.func.as_u32(), &function.plan))
        .collect();

    // Maximal runs of consecutive opcodes of one instruction of one block.
    struct Run {
        start: usize,
        end: usize,
        key: Option<(u32, u32)>,
        block: u32,
    }
    let key_of = |op: &Opcode<'_>| {
        let debug = &op.range.debug;
        debug.unit.zip(debug.machine_inst)
    };
    let mut runs: Vec<Run> = Vec::new();
    for (at, op) in ops.iter().enumerate() {
        let (key, block) = (key_of(op), op.range.debug.block);
        match runs.last_mut() {
            Some(run) if run.key == key && run.block == block && key.is_some() => run.end = at + 1,
            _ => runs.push(Run {
                start: at,
                end: at + 1,
                key,
                block,
            }),
        }
    }
    // An instruction whose opcodes are in two runs was cut by another
    // instruction's opcodes.
    let mut count: HashMap<(u32, u32), usize> = HashMap::new();
    for run in &runs {
        if let Some(key) = run.key {
            *count.entry(key).or_default() += 1;
        }
    }
    let record = |key: (u32, u32)| -> Option<&'a InstStack> {
        plans
            .get(&key.0)?
            .stack_before(MachineInstId(InstId(key.1)))
    };

    let mut stacks = Stacks {
        after: vec![None; ops.len()],
        instructions: 0,
        usable: 0,
        refused: BTreeMap::new(),
    };
    for (at, run) in runs.iter().enumerate() {
        let Some(key) = run.key else { continue };
        stacks.instructions += 1;
        let refused = &mut stacks.refused;
        let mut refuse = |reason: &'static str| *refused.entry(reason).or_default() += 1;
        if count[&key] != 1 {
            refuse("its opcodes are in several runs");
            continue;
        }
        let next = runs.get(at + 1).and_then(|next| {
            let next_key = next.key?;
            (next.block == run.block && next_key.0 == key.0).then_some(next_key)
        });
        let Some(next_key) = next else {
            refuse("no instruction follows in the block");
            continue;
        };
        if count[&next_key] != 1 {
            refuse("the next instruction has its opcodes in several runs");
            continue;
        }
        let (Some(first), Some(second)) = (record(key), record(next_key)) else {
            refuse("no plan record");
            continue;
        };
        let bytes: Vec<u8> = ops[run.start..run.end].iter().map(|op| op.byte).collect();
        if let Some(after) = replay(&bytes, first, second) {
            stacks.usable += 1;
            for (slot, cells) in stacks.after[run.start..run.end].iter_mut().zip(after) {
                *slot = cells;
            }
        } else {
            refuse("the replay does not agree with the next record");
        }
    }
    stacks
}

/// The stack after each opcode of one instruction, `bytes`, which starts from
/// `first` and is followed by the instruction of `second`. `None` when the
/// replay cannot be trusted.
fn replay<'a>(
    bytes: &[u8],
    first: &'a InstStack,
    second: &'a InstStack,
) -> Option<Vec<Option<Vec<Cell<'a>>>>> {
    let mut stack: Vec<Word<'a>> = first.stack.iter().map(Word::Planned).collect();
    let mut after: Vec<Option<Vec<Cell<'a>>>> = vec![None; bytes.len()];
    let cells = |stack: &[Word<'a>]| stack.iter().map(|word| word.cell()).collect::<Vec<_>>();
    // The stack at the `JUMP` of a call, below the address it jumps to.
    let mut at_call = None;
    for (at, &byte) in bytes.iter().enumerate() {
        match byte {
            PUSH0..=PUSH32 => stack.insert(0, Word::Pushed),
            DUP1..=DUP16 => {
                let word = *stack.get(usize::from(byte - DUP1))?;
                stack.insert(0, word);
            }
            SWAP1..=SWAP16 => {
                let depth = usize::from(byte - SWAP1) + 1;
                if stack.len() <= depth {
                    return None;
                }
                stack.swap(0, depth);
            }
            POP => {
                if stack.is_empty() {
                    return None;
                }
                stack.remove(0);
            }
            // A call: the jump, then the opcode the callee returns to.
            JUMP if at + 2 == bytes.len() && bytes[at + 1] == JUMPDEST => {
                if stack.is_empty() {
                    return None;
                }
                stack.remove(0);
                // The words under the continuation. They belong to the caller
                // and the callee does not touch them.
                let depth = continuation_depth(&bytes[..at])?;
                if !matches!(stack.get(depth), Some(Word::Pushed)) {
                    return None;
                }
                at_call = Some(stack[depth + 1..].to_vec());
                continue;
            }
            JUMPDEST if at + 1 == bytes.len() && at > 0 && bytes[at - 1] == JUMP => continue,
            STOP | JUMP | JUMPI | JUMPDEST | RETURN | REVERT | INVALID | SELFDESTRUCT => {
                return None;
            }
            _ => {
                let (takes, gives) = effect(byte)?;
                if stack.len() < takes {
                    return None;
                }
                stack.drain(..takes);
                for _ in 0..gives {
                    stack.insert(0, Word::Result);
                }
            }
        }
        after[at] = Some(cells(&stack));
    }

    let want = &second.stack;
    match at_call {
        None => {
            if stack.len() != want.len()
                || !stack
                    .iter()
                    .zip(want)
                    .all(|(word, entry)| word.agrees_with(entry))
            {
                return None;
            }
        }
        Some(below) => {
            // What the callee takes and gives is not known here, so the call
            // can only be checked from below: all the words under the
            // continuation are the words at the bottom of the next record. At
            // least one must exist, or nothing ties the record to the call.
            if below.is_empty()
                || want.len() < below.len()
                || !below
                    .iter()
                    .rev()
                    .zip(want.iter().rev())
                    .all(|(word, entry)| word.agrees_with(entry))
            {
                return None;
            }
        }
    }
    // After the last opcode the stack is the next record.
    *after.last_mut()? = Some(want.iter().map(Cell::of).collect());
    Some(after)
}

/// How far below the top the continuation of a call is when the run has pushed
/// the callee's address and is about to `JUMP`, and the address is removed:
/// `before_jump` are the opcodes of the run up to the `JUMP`.
///
/// The emitter pushes the continuation, moves it below the arguments with the
/// swap of the argument count, and then pushes the callee's address. With no
/// argument there is no swap, and the continuation is pushed right before the
/// address. Anything else between them, such as the set-up of the frame pointer,
/// makes the depth unknown here.
fn continuation_depth(before_jump: &[u8]) -> Option<usize> {
    let (&callee, rest) = before_jump.split_last()?;
    if !(PUSH0..=PUSH32).contains(&callee) {
        return None;
    }
    match *rest.last()? {
        byte @ SWAP1..=SWAP16 => Some(usize::from(byte - SWAP1) + 1),
        PUSH0..=PUSH32 => Some(0),
        _ => None,
    }
}

/// The slot, counted from the top, of the one word that holds the value labelled
/// `label`; `None` when no word does, or when more than one does, or when the
/// value is dead.
pub(super) fn slot_of(stack: &[Cell<'_>], label: u32) -> Option<u64> {
    let mut found = stack
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.labels.iter().any(|candidate| candidate.0 == label));
    let (slot, cell) = found.next()?;
    if found.next().is_some() || cell.dead {
        return None;
    }
    Some(slot as u64)
}

#[cfg(test)]
mod tests {
    use sonatina_codegen::isa::evm::stack_plan::StackValue;
    use sonatina_ir::ValueId;

    use super::*;

    fn value(id: u32, labels: &[u32], constant: bool, dead: bool) -> StackEntry {
        StackEntry::Value(StackValue {
            value: ValueId::from_u32(id),
            dead,
            constant,
            labels: labels.iter().map(|&label| ValueLabel(label)).collect(),
        })
    }

    fn record(entries: Vec<StackEntry>) -> InstStack {
        InstStack {
            inst: MachineInstId(InstId(0)),
            stack: entries,
        }
    }

    const ADD: u8 = 0x01;
    const PUSH1: u8 = 0x60;
    const SWAP_1: u8 = 0x90;
    const DUP_1: u8 = 0x80;

    fn slots(after: &[Option<Vec<Cell<'_>>>], at: usize, label: u32) -> Option<u64> {
        slot_of(after[at].as_ref()?, label)
    }

    #[test]
    fn a_stack_opcode_moves_a_labelled_word() {
        // [x@1, y@2, RET] then PUSH1, ADD: the sum takes the place of the words
        // the push and x gave. The next record lists it above y.
        let first = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let second = record(vec![
            value(3, &[3], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let after = replay(&[PUSH1, ADD], &first, &second).expect("agrees");
        assert_eq!(slots(&after, 0, 1), Some(1));
        assert_eq!(slots(&after, 0, 2), Some(2));
        // After the add the next record is the stack.
        assert_eq!(slots(&after, 1, 3), Some(0));
        assert_eq!(slots(&after, 1, 2), Some(1));
        assert_eq!(slots(&after, 1, 1), None);
    }

    #[test]
    fn a_swap_changes_the_slots() {
        // x is swapped under y, then POP removes y.
        let first = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let second = record(vec![
            value(1, &[1], false, false),
            StackEntry::ReturnAddress,
        ]);
        let after = replay(&[SWAP_1, POP], &first, &second).expect("agrees");
        assert_eq!(slots(&after, 0, 1), Some(1));
        assert_eq!(slots(&after, 0, 2), Some(0));
        assert_eq!(slots(&after, 1, 1), Some(0));
        assert_eq!(slots(&after, 1, 2), None);
    }

    #[test]
    fn a_word_with_two_copies_or_a_dead_value_has_no_slot() {
        let first = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, true),
            StackEntry::ReturnAddress,
        ]);
        let second = record(vec![
            value(1, &[1], false, false),
            value(1, &[1], false, false),
            value(2, &[2], false, true),
            StackEntry::ReturnAddress,
        ]);
        let after = replay(&[DUP_1], &first, &second).expect("agrees");
        // Two copies of the value labelled 1, and a dead value 2.
        assert_eq!(slots(&after, 0, 1), None);
        assert_eq!(slots(&after, 0, 2), None);
    }

    #[test]
    fn a_replay_that_disagrees_with_the_next_record_gives_nothing() {
        let first = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        // The record after SWAP1 should list y first.
        let shifted = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        assert!(replay(&[SWAP_1], &first, &shifted).is_none());
        // A different length.
        let short = record(vec![value(2, &[2], false, false)]);
        assert!(replay(&[SWAP_1], &first, &short).is_none());
        // An opcode that needs more words than the plan lists.
        let one = record(vec![value(1, &[1], false, false)]);
        assert!(replay(&[SWAP_1], &one, &one).is_none());
        // An opcode that is not known.
        assert!(replay(&[0x0c], &first, &first).is_none());
    }

    #[test]
    fn a_call_is_checked_from_below_and_lists_the_next_record_after_it() {
        // [x@1, RET] then PUSH1 (continuation), SWAP1, PUSH1 (callee), JUMP,
        // JUMPDEST. The callee takes the continuation and x and returns r.
        let first = record(vec![
            value(1, &[1], false, false),
            StackEntry::ReturnAddress,
        ]);
        let second = record(vec![
            value(9, &[9], false, false),
            StackEntry::ReturnAddress,
        ]);
        let bytes = [PUSH1, SWAP_1, PUSH1, JUMP, JUMPDEST];
        let after = replay(&bytes, &first, &second).expect("agrees");
        assert_eq!(slots(&after, 1, 1), Some(0));
        // The jump says nothing; the landing is the next record.
        assert!(after[3].is_none());
        assert_eq!(slots(&after, 4, 9), Some(0));
        // A next record that does not end in the same word is refused.
        let wrong = record(vec![
            value(9, &[9], false, false),
            value(5, &[], false, false),
        ]);
        assert!(replay(&bytes, &first, &wrong).is_none());
    }

    #[test]
    fn a_call_needs_every_word_below_its_continuation_in_the_next_record() {
        // [x@1, w@2, RET]: the call takes x and the continuation and leaves w
        // and the return address alone. The callee returns r.
        let first = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let bytes = [PUSH1, SWAP_1, PUSH1, JUMP, JUMPDEST];
        let agrees = |entries: Vec<StackEntry>| replay(&bytes, &first, &record(entries)).is_some();

        // The same words below, with one or two results on top.
        let one = record(vec![
            value(9, &[9], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let after = replay(&bytes, &first, &one).expect("agrees");
        assert_eq!(slots(&after, 1, 1), Some(0));
        assert_eq!(slots(&after, 1, 2), Some(2));
        assert_eq!(slots(&after, 4, 9), Some(0));
        assert_eq!(slots(&after, 4, 2), Some(1));
        assert!(agrees(vec![
            value(8, &[8], false, false),
            value(9, &[9], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]));

        // The return address agrees and the word above it does not: refused.
        // The bottom word alone used to be enough.
        assert!(!agrees(vec![
            value(9, &[9], false, false),
            value(3, &[3], false, false),
            StackEntry::ReturnAddress,
        ]));
        // The word below is missing, or in the wrong place.
        assert!(!agrees(vec![
            value(9, &[9], false, false),
            StackEntry::ReturnAddress,
        ]));
        assert!(!agrees(vec![
            value(2, &[2], false, false),
            value(9, &[9], false, false),
            StackEntry::ReturnAddress,
        ]));
    }

    #[test]
    fn a_call_without_arguments_has_its_continuation_on_top() {
        // [w@2, RET] then PUSH1 (continuation), PUSH1 (callee), JUMP, JUMPDEST.
        let first = record(vec![
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let bytes = [PUSH1, PUSH1, JUMP, JUMPDEST];
        let agreeing = record(vec![
            value(9, &[9], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let after = replay(&bytes, &first, &agreeing).expect("agrees");
        assert_eq!(slots(&after, 3, 9), Some(0));
        assert_eq!(slots(&after, 3, 2), Some(1));
        let disagreeing = record(vec![
            value(9, &[9], false, false),
            value(3, &[3], false, false),
            StackEntry::ReturnAddress,
        ]);
        assert!(replay(&bytes, &first, &disagreeing).is_none());
    }

    #[test]
    fn a_call_whose_continuation_cannot_be_found_or_has_nothing_below_gets_no_stack() {
        let first = record(vec![
            value(1, &[1], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        let second = record(vec![
            value(9, &[9], false, false),
            value(2, &[2], false, false),
            StackEntry::ReturnAddress,
        ]);
        // The same call as above, with an opcode between the swap and the
        // callee's address: the depth of the continuation is unknown.
        let bytes = [PUSH1, SWAP_1, PUSH1, POP, PUSH1, JUMP, JUMPDEST];
        assert!(replay(&bytes, &first, &second).is_none());
        // The opcode before the callee's address is not a push at all.
        let bytes = [PUSH1, SWAP_1, ADD, JUMP, JUMPDEST];
        assert!(replay(&bytes, &first, &second).is_none());
        // A swap that is not the continuation's: the word it reaches is not the
        // one that the run pushed.
        let bytes = [PUSH1, SWAP_1, SWAP_1, PUSH1, JUMP, JUMPDEST];
        assert!(replay(&bytes, &first, &second).is_none());
        // Nothing below the continuation: the record cannot be tied to the call.
        let alone = record(vec![value(1, &[1], false, false)]);
        let after = record(vec![value(9, &[9], false, false)]);
        let bytes = [PUSH1, SWAP_1, PUSH1, JUMP, JUMPDEST];
        assert!(replay(&bytes, &alone, &after).is_none());
    }
}
