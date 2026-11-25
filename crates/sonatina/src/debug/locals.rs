//! The parameters and local variables that are in scope after each opcode.
//!
//! Lowering tags every IR instruction that has an origin with the source
//! bindings visible before it and after it (`crate::locals`). The ethdebug
//! format gives an instruction's context as it holds after the instruction
//! executes, so each opcode of the runtime code gets the bindings that hold
//! once it has run, and a debugger that stops before an opcode reads the list
//! of the opcode before it.
//!
//! An IR instruction becomes several opcodes, a run of consecutive opcodes with
//! the same tag. Every opcode of a run but the last gets the bindings visible
//! before the instruction, the last gets those visible after it. The
//! difference is the instruction that completes a `let`.
//!
//! A list is left out rather than guessed: an opcode whose bindings are not
//! known gets the fields of the contract and no locals.
//!
//! A parameter or local of one word also gets a stack pointer where `stack`
//! can say which word of the stack holds it after the opcode. Lowering names, in
//! the tag of each instruction, the label of the value that holds each binding
//! before the instruction and after it, and the opcode uses the one that goes
//! with the list it takes. A binding without a label, or whose labelled value is
//! not found on the stack exactly once and alive, is listed with its type and no
//! pointer, as before.

use std::collections::{HashMap, HashSet};

use ethdebug::{Pointer, Variable};
use hull::{SourceBinder, origin::ResolvedOrigin};

use super::{
    Resources, stack,
    types::{Env, holds_one_word, locate},
};
use crate::{CompiledSection, LocalsTag, PcRange};

const STOP: u8 = 0x00;
const JUMP: u8 = 0x56;
const JUMPI: u8 = 0x57;
const RETURN: u8 = 0xf3;
const REVERT: u8 = 0xfd;
const INVALID: u8 = 0xfe;
const SELFDESTRUCT: u8 = 0xff;

/// Where a function is written: the url of its file and its byte range.
struct FunctionText {
    url: String,
    start: u32,
    end: u32,
}

impl FunctionText {
    fn of<'db>(env: &Env<'_, 'db>, span: hir::span::Span<'db>) -> Self {
        let absolute = span.resolve_to_absolute(env.db);
        Self {
            url: absolute.file.url(env.db).as_str().to_owned(),
            start: absolute.start.as_u32(),
            end: absolute.end.as_u32(),
        }
    }

    fn contains(&self, origin: &ResolvedOrigin) -> bool {
        self.url == origin.url && self.start <= origin.start && origin.end <= self.end
    }
}

/// An opcode of the code region and the range of the byte map that holds it.
struct Op<'a> {
    pc: u32,
    byte: u8,
    range: &'a PcRange,
}

impl Op<'_> {
    fn tag(&self) -> Option<&LocalsTag> {
        self.range.debug.tag.as_ref()
    }

    /// The IR function and block that the opcode belongs to.
    fn place(&self) -> (&str, u32) {
        (&self.range.debug.func, self.range.debug.block)
    }

    /// An opcode that ends straight-line execution: the next opcode to run need
    /// not be the next in the code, so what holds after it is not what holds
    /// after the opcode before it.
    fn transfers_control(&self) -> bool {
        matches!(
            self.byte,
            STOP | JUMP | JUMPI | RETURN | REVERT | INVALID | SELFDESTRUCT
        )
    }
}

/// The opcodes of the code region, each with the range of the byte map that
/// holds it.
fn opcodes<'a>(code: &[u8], section: &'a CompiledSection) -> Vec<Op<'a>> {
    let mut ops = Vec::new();
    let mut range = 0;
    for pc in ethdebug::instruction_offsets(code) {
        while section.map[range].end <= pc {
            range += 1;
        }
        ops.push(Op {
            pc,
            byte: code[pc as usize],
            range: &section.map[range],
        });
    }
    ops
}

/// A binding of a list, as its binder id and the label of the value that holds
/// it, when lowering knows one.
type Bound = (u32, Option<u32>);

/// The bindings, as binder ids in the order they were bound, that hold after
/// each opcode of `code`, or `None` where they are not known. `keep` says whether
/// a binding may be listed at the code of a range: the tags of lowering are
/// about what is visible in the machine's frame, and `keep` narrows them to what
/// is visible in the source at that range.
fn bindings_after(
    ops: &[Op<'_>],
    keep: &dyn Fn(&PcRange, u32) -> bool,
) -> Vec<(u32, Option<Vec<Bound>>)> {
    // The bindings of `set` that `keep` allows, each with its label.
    let kept = |range: &PcRange, set: &[u32], labels: &[Option<u32>]| -> Vec<Bound> {
        set.iter()
            .enumerate()
            .filter(|&(_, &id)| keep(range, id))
            .map(|(at, &id)| (id, labels.get(at).copied().flatten()))
            .collect()
    };

    // Opcodes of one IR instruction are consecutive and share its tag.
    let same_instruction = |a: &Op<'_>, b: &Op<'_>| match (a.tag(), b.tag()) {
        (Some(x), Some(y)) => x.inst == y.inst && a.range.debug.func == b.range.debug.func,
        _ => false,
    };

    let mut after: Vec<Option<Vec<Bound>>> = vec![None; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        let Some(tag) = op.tag() else { continue };
        // After a call or a return the next opcode to run is in another
        // frame, and a branch, a `break` or a `continue` may leave scopes (the
        // end of a loop or of a block): a jump says nothing about the bindings
        // at its target, wherever it goes. Its list would be the statement's,
        // which holds the bindings of the scope that the jump leaves.
        if matches!(op.byte, JUMP | JUMPI) {
            continue;
        }
        let last = ops
            .get(i + 1)
            .is_none_or(|next| !same_instruction(op, next));
        after[i] = Some(if last {
            kept(op.range, &tag.after, &tag.after_labels)
        } else {
            kept(op.range, &tag.before, &tag.before_labels)
        });
    }

    // The next tagged opcode of the same function and block, for glue code that
    // comes first in its block (a label, say). Glue is not an instruction that
    // lowering labelled, so what it lists has no labels.
    let unlabelled =
        |bound: Vec<Bound>| -> Vec<Bound> { bound.into_iter().map(|(id, _)| (id, None)).collect() };
    let mut next_before: Vec<Option<Vec<Bound>>> = vec![None; ops.len()];
    let mut upcoming: Option<(&Op<'_>, &LocalsTag)> = None;
    for (i, op) in ops.iter().enumerate().rev() {
        next_before[i] = upcoming
            .filter(|(next, _)| next.place() == op.place())
            .map(|(next, tag)| unlabelled(kept(next.range, &tag.before, &[])));
        if let Some(tag) = op.tag() {
            upcoming = Some((op, tag));
        }
    }

    for i in 0..ops.len() {
        if ops[i].tag().is_some() {
            continue;
        }
        // Code the compiler placed between instructions: it runs in the state
        // of the opcode before it, unless that one decided where control goes.
        after[i] = if ops[i].transfers_control() {
            None
        } else {
            i.checked_sub(1)
                .filter(|&prev| {
                    ops[prev].place() == ops[i].place() && !ops[prev].transfers_control()
                })
                .and_then(|prev| after[prev].clone().map(unlabelled))
                .or_else(|| next_before[i].clone())
        };
    }
    ops.iter()
        .zip(after)
        .map(|(op, set)| (op.pc, set))
        .collect()
}

/// The bindings of `set` that are in scope, in the order they were bound. Of
/// several bindings of one name only the one bound last is: the innermost wins.
fn visible<'a>(set: &[u32], name: impl Fn(u32) -> &'a str) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut out: Vec<u32> = set
        .iter()
        .rev()
        .copied()
        .filter(|&id| seen.insert(name(id)))
        .collect();
    out.reverse();
    out
}

/// Adds to `lists` the list of every distinct set of bindings, and returns the
/// list that holds after each opcode: the fields, without those that a visible
/// local hides, then the parameters and locals.
///
/// `fields` is the list of every instruction without locals, which is the list
/// at index `fields_list` of `lists`.
pub(super) fn per_opcode<'db>(
    env: &Env<'_, 'db>,
    resources: &mut Resources<'db>,
    binders: &[SourceBinder<'db>],
    code: &[u8],
    section: &CompiledSection,
    lists: &mut Vec<Vec<Variable>>,
    fields_list: u32,
) -> Vec<(u32, u32)> {
    let fields = lists[fields_list as usize].clone();
    let functions: Vec<FunctionText> = binders
        .iter()
        .map(|binder| FunctionText::of(env, binder.function))
        .collect();
    // Whether a binding is of a type that one word of the stack holds.
    let one_word: Vec<bool> = binders
        .iter()
        .map(|binder| holds_one_word(env, binder.ty))
        .collect();
    let mut variables: HashMap<(u32, Option<u64>), Variable> = HashMap::new();
    let mut by_set: HashMap<Vec<(u32, Option<u64>)>, u32> = HashMap::new();
    // A binding is listed only where the statement was written inside the
    // function that declares it: the code of a function that specialization
    // inlined into another has the other function's bindings in the machine and
    // not in its source.
    let contained = |range: &PcRange, id: u32| -> bool {
        let Ok(stamp) = &range.origin else {
            return false;
        };
        let Ok(origin) = ResolvedOrigin::parse(stamp) else {
            return false;
        };
        origin.generated.is_none() && functions[id as usize].contains(&origin)
    };
    let ops = opcodes(code, section);
    let stacks = stack::stacks_after(
        &ops.iter()
            .map(|op| stack::Opcode {
                byte: op.byte,
                range: op.range,
            })
            .collect::<Vec<_>>(),
        section,
    );
    bindings_after(&ops, &contained)
        .into_iter()
        .zip(&stacks.after)
        .map(|((pc, set), cells)| {
            let Some(set) = set.filter(|set| !set.is_empty()) else {
                return (pc, fields_list);
            };
            // The slot of each binding of one word whose labelled value is on
            // the stack after the opcode exactly once and alive.
            let slots: Vec<(u32, Option<u64>)> = set
                .iter()
                .map(|&(id, label)| {
                    let slot = label
                        .zip(cells.as_deref())
                        .filter(|_| one_word[id as usize])
                        .and_then(|(label, cells)| stack::slot_of(cells, label));
                    (id, slot)
                })
                .collect();
            let index = *by_set.entry(slots.clone()).or_insert_with(|| {
                let ids: Vec<u32> = slots.iter().map(|&(id, _)| id).collect();
                let in_scope = visible(&ids, |id| binders[id as usize].name.as_str());
                let mut list: Vec<Variable> = fields
                    .iter()
                    .filter(|field| {
                        !in_scope
                            .iter()
                            .any(|&id| binders[id as usize].name == field.identifier)
                    })
                    .cloned()
                    .collect();
                for &id in &in_scope {
                    let slot = slots
                        .iter()
                        .find(|&&(candidate, _)| candidate == id)
                        .and_then(|&(_, slot)| slot);
                    let variable = variables.entry((id, slot)).or_insert_with(|| {
                        let binder = &binders[id as usize];
                        Variable {
                            identifier: binder.name.clone(),
                            declaration: locate(env, binder.declaration),
                            ty: resources.types.describe(env, binder.ty),
                            pointer: slot.map(|slot| Pointer::Stack { slot }),
                        }
                    });
                    list.push(variable.clone());
                }
                lists.push(list);
                lists.len() as u32 - 1
            });
            (pc, index)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use solcore_test_utils::run_in_large_stack;

    use super::*;
    use crate::{Frame, PcDebug};

    /// An IR instruction `inst` of block `block`, with the bindings before and
    /// after it, over the code bytes `start..end`.
    fn tagged(
        (start, end): (u32, u32),
        block: u32,
        (inst, before, after, frame): (u32, &[u32], &[u32], Frame),
    ) -> PcRange {
        PcRange {
            start,
            end,
            origin: Ok("s 0 1 memory:///x.sol".to_owned()),
            debug: PcDebug {
                func: "f".to_owned(),
                block,
                unit: None,
                machine_inst: None,
                tag: Some(LocalsTag {
                    inst,
                    before: before.to_vec(),
                    after: after.to_vec(),
                    before_labels: vec![None; before.len()],
                    after_labels: vec![None; after.len()],
                    frame,
                }),
            },
        }
    }

    /// Code that Sonatina placed without an instruction.
    fn glue((start, end): (u32, u32), block: u32) -> PcRange {
        PcRange {
            start,
            end,
            origin: Err("label_or_fixup_only"),
            debug: PcDebug {
                func: "f".to_owned(),
                block,
                unit: None,
                machine_inst: None,
                tag: None,
            },
        }
    }

    fn section(code: &[u8], map: Vec<PcRange>) -> CompiledSection {
        CompiledSection {
            bytes: code.to_vec(),
            code_len: Some(code.len() as u32),
            map,
            stack_plans: Vec::new(),
        }
    }

    /// The bindings after each opcode, by the opcode's offset.
    fn after(code: &[u8], map: Vec<PcRange>) -> Vec<(u32, Option<Vec<u32>>)> {
        let section = section(code, map);
        let ops = opcodes(code, &section);
        bindings_after(&ops, &|_, _| true)
            .into_iter()
            .map(|(pc, set)| {
                (
                    pc,
                    set.map(|set| set.into_iter().map(|(id, _)| id).collect()),
                )
            })
            .collect()
    }

    #[test]
    fn a_tag_survives_encoding() {
        for tag in [
            LocalsTag {
                inst: 12,
                before: vec![],
                after: vec![3],
                before_labels: vec![],
                after_labels: vec![Some(41)],
                frame: Frame::Plain,
            },
            LocalsTag {
                inst: 0,
                before: vec![0, 4, 9],
                after: vec![0, 4, 9, 10],
                before_labels: vec![None, Some(7), None],
                after_labels: vec![None, Some(7), None, Some(8)],
                frame: Frame::Call,
            },
            LocalsTag {
                inst: 7,
                before: vec![1],
                after: vec![],
                before_labels: vec![None],
                after_labels: vec![],
                frame: Frame::Jump,
            },
        ] {
            assert_eq!(LocalsTag::parse(&tag.encode()), Ok(tag));
        }
        assert!(LocalsTag::parse("1:2:3").is_err());
        assert!(LocalsTag::parse("1:2:3:x").is_err());
        assert!(LocalsTag::parse("1:a:3:p").is_err());
        assert!(LocalsTag::parse("1:2:3:p:4").is_err());
    }

    #[test]
    fn the_innermost_binding_of_a_name_is_in_scope() {
        let names = ["total", "bonus", "total", "x"];
        let name = |id: u32| names[id as usize];
        assert_eq!(visible(&[0, 1], name), [0, 1]);
        // The second `total` shadows the first, which no longer counts, and the
        // others keep the order in which they were bound.
        assert_eq!(visible(&[0, 1, 2, 3], name), [1, 2, 3]);
        assert_eq!(visible(&[], name), Vec::<u32>::new());
    }

    #[test]
    fn the_opcode_that_completes_an_instruction_lists_what_holds_after_it() {
        // PUSH1 1, DUP1, ADD (one IR instruction that completes a `let`), then
        // the next instruction's PUSH0.
        let code = [0x60, 0x01, 0x80, 0x01, 0x5f];
        let plain = (7, &[1][..], &[1, 2][..], Frame::Plain);
        let map = vec![
            tagged((0, 2), 0, plain),
            tagged((2, 3), 0, plain),
            tagged((3, 4), 0, plain),
            tagged((4, 5), 0, (8, &[1, 2], &[1, 2], Frame::Plain)),
        ];
        assert_eq!(
            after(&code, map),
            [
                (0, Some(vec![1])),
                (2, Some(vec![1])),
                (3, Some(vec![1, 2])),
                (4, Some(vec![1, 2])),
            ]
        );
    }

    #[test]
    fn a_jump_out_of_the_frame_says_nothing_about_the_bindings() {
        // PUSH2 callee, JUMP, JUMPDEST: a call. Then a return's SWAP1, JUMP.
        let code = [0x61, 0x00, 0x09, 0x56, 0x5b, 0x90, 0x56];
        let call = (3, &[1][..], &[1, 2][..], Frame::Call);
        let ret = (4, &[1, 2][..], &[1, 2][..], Frame::Return);
        let map = vec![
            tagged((0, 3), 0, call),
            tagged((3, 4), 0, call),
            tagged((4, 5), 0, call),
            tagged((5, 6), 0, ret),
            tagged((6, 7), 0, ret),
        ];
        assert_eq!(
            after(&code, map),
            [
                (0, Some(vec![1])),
                (3, None),
                (4, Some(vec![1, 2])),
                (5, Some(vec![1, 2])),
                (6, None),
            ]
        );
        // A `break` or `continue` leaves scopes: the same for its jump.
        let leave = (5, &[1, 2][..], &[1, 2][..], Frame::Jump);
        let map = vec![tagged((0, 3), 0, leave), tagged((3, 4), 0, leave)];
        assert_eq!(after(&code[..4], map), [(0, Some(vec![1, 2])), (3, None)]);
        // So does a plain jump, the end of a branch: its target may be past the
        // end of the scope that the statement's list holds. A conditional jump
        // (`PUSH2 target, JUMPI`) is the same.
        let join = (6, &[1][..], &[1][..], Frame::Plain);
        let map = vec![tagged((0, 3), 0, join), tagged((3, 4), 0, join)];
        assert_eq!(after(&code[..4], map), [(0, Some(vec![1])), (3, None)]);
        let branch = [0x61, 0x00, 0x09, 0x57];
        let map = vec![tagged((0, 3), 0, join), tagged((3, 4), 0, join)];
        assert_eq!(after(&branch, map), [(0, Some(vec![1])), (3, None)]);
    }

    #[test]
    fn code_without_an_instruction_takes_the_state_around_it() {
        // PUSH1 1 (block 0), then glue in block 0, then the glue that opens
        // block 1 (a label), the first instruction of block 1, and a jump that
        // no instruction made.
        let code = [0x60, 0x01, 0x5f, 0x5b, 0x5f, 0x56];
        let map = vec![
            tagged((0, 2), 0, (1, &[1], &[1, 2], Frame::Plain)),
            glue((2, 3), 0),
            glue((3, 4), 1),
            tagged((4, 5), 1, (2, &[1, 2], &[1, 2], Frame::Plain)),
            glue((5, 6), 1),
        ];
        assert_eq!(
            after(&code, map),
            [
                (0, Some(vec![1, 2])),
                // In the state the instruction before it left.
                (2, Some(vec![1, 2])),
                // First in its block: the state the block's first instruction
                // starts in.
                (3, Some(vec![1, 2])),
                (4, Some(vec![1, 2])),
                // Control goes elsewhere: not known.
                (5, None),
            ]
        );
        // Glue with no tagged opcode in its block has nothing to take.
        let map = vec![glue((0, 2), 0), glue((2, 3), 1)];
        assert_eq!(after(&code[..3], map), [(0, None), (2, None)]);
    }

    /// One opcode of the runtime of a compiled fixture: what it is, the range of
    /// the fixture's text it was made from, and what it lists.
    struct Listed {
        byte: u8,
        origin: Option<(usize, usize)>,
        /// The parameters and locals, which have a declaration.
        locals: Vec<String>,
        /// Every listed identifier: the contract's fields, then the locals.
        variables: Vec<String>,
    }

    /// The fixture `scopes` compiled at O0 with debug information: its text,
    /// the identifiers of the contract's fields, and what each opcode of the
    /// runtime lists.
    fn scopes() -> (String, Vec<String>, Vec<Listed>) {
        use hull::origin::SourceTable;

        use super::super::{storage, test_support::lower_case};
        use crate::{CompileOptions, OptLevel, compile_hull_program};

        let (db, _, program) = lower_case("scopes");
        let table = SourceTable::from_program(db, &program);
        let env = Env {
            db,
            sources: &table,
        };
        let options = CompileOptions {
            opt_level: OptLevel::O0,
            debug_info: true,
        };
        let objects = compile_hull_program(db, &program, options).expect("compilation succeeds");
        let object = objects
            .iter()
            .find(|object| object.contract == "Scopes")
            .expect("the contract");
        let section = object.runtime.as_ref().expect("a runtime");
        let code = &section.bytes[..section.code_len.expect("a code length") as usize];

        let mut resources = Resources::new();
        let facts = &program.debug.contracts["Scopes"];
        let fields = storage::fields(&env, &mut resources, "Scopes", facts);
        let mut lists = vec![fields.per_instruction.clone()];
        let at = per_opcode(
            &env,
            &mut resources,
            &program.debug.binders,
            code,
            section,
            &mut lists,
            0,
        );

        let repo = solcore_test_utils::repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(repo.join("tests/ethdebug/scopes/main.sol"))
            .expect("the fixture");
        let ops = at
            .into_iter()
            .map(|(pc, list)| {
                let range = section
                    .map
                    .iter()
                    .find(|range| range.start <= pc && pc < range.end)
                    .expect("a range");
                let origin = range
                    .origin
                    .as_ref()
                    .ok()
                    .and_then(|stamp| ResolvedOrigin::parse(stamp).ok())
                    .filter(|origin| {
                        origin.generated.is_none() && origin.url.ends_with("/scopes/main.sol")
                    })
                    .map(|origin| (origin.start as usize, origin.end as usize));
                let list = &lists[list as usize];
                Listed {
                    byte: code[pc as usize],
                    origin,
                    locals: list
                        .iter()
                        .filter(|variable| variable.declaration.is_some())
                        .map(|variable| variable.identifier.clone())
                        .collect(),
                    variables: list
                        .iter()
                        .map(|variable| variable.identifier.clone())
                        .collect(),
                }
            })
            .collect();
        let fields = fields
            .per_instruction
            .iter()
            .map(|field| field.identifier.clone())
            .collect();
        (text, fields, ops)
    }

    #[test]
    fn the_stack_plan_is_usable_for_instructions_of_a_compiled_fixture_and_not_for_all() {
        use super::super::test_support::lower_case;
        use crate::{CompileOptions, OptLevel, compile_hull_program};

        run_in_large_stack(|| {
            let (db, _, program) = lower_case("scopes");
            let options = CompileOptions {
                opt_level: OptLevel::O0,
                debug_info: true,
            };
            let objects =
                compile_hull_program(db, &program, options).expect("compilation succeeds");
            let object = objects
                .iter()
                .find(|object| object.contract == "Scopes")
                .expect("the contract");
            let section = object.runtime.as_ref().expect("a runtime");
            assert!(
                !section.stack_plans.is_empty(),
                "O0 with debug info exports plans"
            );
            let code = &section.bytes[..section.code_len.expect("a code length") as usize];
            let ops = opcodes(code, section);
            let stacks = stack::stacks_after(
                &ops.iter()
                    .map(|op| stack::Opcode {
                        byte: op.byte,
                        range: op.range,
                    })
                    .collect::<Vec<_>>(),
                section,
            );
            println!(
                "scopes: {} of {} instructions have a usable stack plan",
                stacks.usable, stacks.instructions
            );
            for (reason, count) in &stacks.refused {
                println!("scopes: {count} instructions have none: {reason}");
            }
            assert_eq!(
                stacks.usable + stacks.refused.values().sum::<usize>(),
                stacks.instructions
            );
            // Branches and returns end a block and have none, so not all do.
            assert!(0 < stacks.usable && stacks.usable < stacks.instructions);
        });
    }

    /// The byte range of `text` from the start of `from` to the start of `to`.
    fn between(text: &str, from: &str, to: &str) -> (usize, usize) {
        let start = text.find(from).unwrap_or_else(|| panic!("`{from}`"));
        let end = start + text[start..].find(to).unwrap_or_else(|| panic!("`{to}`"));
        (start, end)
    }

    /// The opcodes made from statements inside `range`.
    fn within(ops: &[Listed], (start, end): (usize, usize)) -> Vec<&Listed> {
        ops.iter()
            .filter(|op| op.origin.is_some_and(|(a, b)| start <= a && b <= end))
            .collect()
    }

    #[test]
    fn no_jump_lists_a_local_and_every_jump_lists_the_fields() {
        run_in_large_stack(|| {
            let (text, fields, ops) = scopes();
            // The jumps that leave a loop or a block are made by statements of
            // the fixture; the dispatcher and the helpers have jumps of their own.
            let own = within(&ops, (0, text.len()));
            assert!(own.iter().any(|op| op.byte == JUMP));
            assert!(own.iter().any(|op| op.byte == JUMPI));
            for op in ops.iter().filter(|op| matches!(op.byte, JUMP | JUMPI)) {
                // A jump lists the fields of the contract and no local, wherever
                // it leaves to.
                assert_eq!(op.locals, Vec::<String>::new());
                assert_eq!(op.variables, fields);
            }
        });
    }

    #[test]
    fn a_loop_variable_is_not_listed_after_its_loop() {
        run_in_large_stack(|| {
            let (text, _, ops) = scopes();
            let function = between(&text, "function loop_sum", "function block_sum");
            let in_loop = between(&text, "for (let i", "let after_loop");
            let inside = within(&ops, in_loop);
            assert!(
                inside
                    .iter()
                    .any(|op| op.locals.iter().any(|name| name == "i")),
                "the loop's own code lists `i`"
            );
            // The branches that leave the loop (its test, the `break`) are made by
            // statements inside it and list no local.
            let leaving: Vec<_> = inside
                .iter()
                .filter(|op| matches!(op.byte, JUMP | JUMPI))
                .collect();
            assert!(leaving.len() >= 2, "{} jumps", leaving.len());
            for op in leaving {
                assert_eq!(op.locals, Vec::<String>::new());
            }
            let after = within(&ops, (in_loop.1, function.1));
            assert!(!after.is_empty());
            for op in after {
                assert!(!op.locals.iter().any(|name| name == "i"), "{:?}", op.locals);
            }
        });
    }

    #[test]
    fn a_block_variable_is_not_listed_after_its_block() {
        run_in_large_stack(|| {
            let (text, _, ops) = scopes();
            let function = between(&text, "function block_sum", "function captured");
            let block = between(&text, "if (n == uint256(1))", "let after_block");
            let inside = within(&ops, block);
            assert!(
                inside
                    .iter()
                    .any(|op| op.locals.iter().any(|name| name == "inner")),
                "the block's own code lists `inner`"
            );
            // The branch that leaves the block is made by the `if` around it.
            let leaving: Vec<_> = inside
                .iter()
                .filter(|op| matches!(op.byte, JUMP | JUMPI))
                .collect();
            assert!(!leaving.is_empty());
            for op in leaving {
                assert_eq!(op.locals, Vec::<String>::new());
            }
            let after = within(&ops, (block.1, function.1));
            assert!(!after.is_empty());
            for op in after {
                assert!(
                    !op.locals.iter().any(|name| name == "inner"),
                    "{:?}",
                    op.locals
                );
            }
        });
    }

    #[test]
    fn a_lambda_lists_its_parameter_and_not_a_variable_it_captures() {
        run_in_large_stack(|| {
            let (text, _, ops) = scopes();
            // The first use of `base` in the lambda is in code that compile-time
            // evaluation removes, so the captured parameter that lifting made
            // must be recognized by what it is and not by where its first use is.
            let lambda = between(&text, "lam (v", "return f(");
            let body = within(&ops, lambda);
            assert!(!body.is_empty());
            let listed: BTreeSet<&str> = body
                .iter()
                .flat_map(|op| op.locals.iter().map(String::as_str))
                .collect();
            assert_eq!(listed, BTreeSet::from(["v"]));
        });
    }
}
