//! Contract storage fields as the variables of a runtime program.
//!
//! A field is listed with its type whenever the type has a description. It gets
//! a pointer only when its place is known for certain: every field before it is
//! exactly one slot, so its slot is its declaration index. Hull assigns a slot
//! by declaration index, and the specializer (a contract whose module sees
//! std's field-access classes) sums std's `StorageSize` over the earlier fields;
//! the two agree only while every size is 1, which is why `()` (size 0 in std,
//! one slot in Hull) is treated as of unknown size.
//!
//! The size table is that of std's explicit `StorageSize` instances that return
//! 1 (`std/std.sol`: `word`, `uint256`, `bytes32`, `address`, `string`,
//! `memory<string>`, `bytes`, `memory<bytes>`, `mapping` and `array`). std also
//! has explicit instances for `uint8` to `uint128` and `int8` to `int256`, each
//! returning 1, but they are left out of the table to keep it short, so a field
//! after one of them has no pointer. A `bool` and a user data type have no
//! explicit instance; their size comes from the type's representation, which is
//! not known here, so no later field gets a pointer.
//!
//! Limitation: a pointer says where the whole word is when the field is a
//! `word`, `uint256`, `bytes32` or `address`. A `bool`, a `string` or a user
//! type has a type and no pointer; a mapping has no bytes of its own, and its
//! entries are reached through a pointer template.

use ethdebug::{Pointer, Template, Variable, is_pointer_identifier};
use hir_ty::{BuiltinTyCtor, Ty, TyCtor, TyKind, UserTyCtorKind, is_canonical_std_def_named};
use hull::FieldFacts;

use super::{
    Resources,
    types::{Env, locate},
};

/// What the variables of contract `contract` are: the list for every
/// instruction, which leaves out each declaration to keep the file small, and the
/// list of the program-level context, which has them.
pub(super) struct Fields {
    pub(super) per_instruction: Vec<Variable>,
    pub(super) program: Vec<Variable>,
}

pub(super) fn fields<'db>(
    env: &Env<'_, 'db>,
    resources: &mut Resources<'db>,
    contract: &str,
    facts: &[FieldFacts<'db>],
) -> Fields {
    let mut out = Fields {
        per_instruction: Vec::new(),
        program: Vec::new(),
    };
    // The slot of the next field, while every field so far has a size std states.
    let mut slot = Some(0u64);
    for field in facts {
        let ty = field.ty;
        let own_slot = slot.filter(|_| one_word(env, ty) || mapping_parts(env, ty).is_some());
        if let (Some(slot), Some((key, value))) = (own_slot, mapping_parts(env, ty))
            && one_word(env, key)
            && one_word(env, value)
        {
            // The key is the word that std's `Typedef.rep` gives; it is the
            // value itself for the one-word types above, and not for others.
            let name = format!("solcore${contract}${}$entry", field.name);
            if is_pointer_identifier(&name) {
                resources.add_template(name, Template::MappingEntry { slot });
            }
        }
        let pointer = own_slot
            .filter(|_| one_word(env, ty))
            .map(|slot| Pointer::Storage {
                name: field.name.clone(),
                slot,
            });
        let variable = Variable {
            identifier: field.name.clone(),
            declaration: None,
            ty: resources.types.describe(env, ty),
            pointer,
        };
        out.program.push(Variable {
            declaration: locate(env, field.declaration),
            ..variable.clone()
        });
        out.per_instruction.push(variable);
        slot = slot
            .zip(storage_size(env, ty))
            .map(|(slot, size)| slot + size);
    }
    out
}

fn is_std<'db>(env: &Env<'_, 'db>, ty: Ty<'db>, name: &str, arity: usize) -> bool {
    match ty.kind(env.db) {
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } => {
            user.kind == UserTyCtorKind::Adt
                && args.len() == arity
                && is_canonical_std_def_named(env.db, user.def, name)
        }
        _ => false,
    }
}

/// `word`, `uint256`, `bytes32` and `address`, whose load and store move the
/// whole word of the slot unchanged.
fn one_word<'db>(env: &Env<'_, 'db>, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(env.db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty()
    ) || ["uint256", "bytes32", "address"]
        .iter()
        .any(|name| is_std(env, ty, name, 0))
}

fn mapping_parts<'db>(env: &Env<'_, 'db>, ty: Ty<'db>) -> Option<(Ty<'db>, Ty<'db>)> {
    if !is_std(env, ty, "mapping", 2) {
        return None;
    }
    match ty.kind(env.db) {
        TyKind::Named { args, .. } => Some((args[0], args[1])),
        _ => None,
    }
}

/// The slots of `ty`, when std's explicit `StorageSize` instance gives exactly 1.
///
/// Every size here is 1, so the slot of a field is its declaration index. That
/// is the slot Hull assigns and the sum the specializer folds, so the pointer
/// does not depend on which of the two lowers the field access. `()` is left
/// out: std sizes it 0 and Hull gives it a slot.
fn storage_size<'db>(env: &Env<'_, 'db>, ty: Ty<'db>) -> Option<u64> {
    let db = env.db;
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty() => return Some(1),
        _ => {}
    }
    let std_one = ["uint256", "bytes32", "address", "string", "bytes"]
        .iter()
        .any(|name| is_std(env, ty, name, 0));
    let std_array = is_std(env, ty, "array", 1) || is_std(env, ty, "mapping", 2);
    let reference = is_std(env, ty, "memory", 1)
        && matches!(
            ty.kind(db),
            TyKind::Named { args, .. }
                if is_std(env, args[0], "string", 0) || is_std(env, args[0], "bytes", 0)
        );
    (std_one || std_array || reference).then_some(1)
}

#[cfg(test)]
mod tests {
    use hull::origin::SourceTable;
    use solcore_test_utils::run_in_large_stack;

    use super::{super::test_support::lower_case, *};

    /// The fields of `contract` in the fixture `types`: each one's identifier
    /// and pointer slot, with the resources they filled.
    fn listed(contract: &str) -> (Vec<(String, Option<u64>)>, Resources<'static>, Fields) {
        listed_in("types", contract)
    }

    /// The same for the contract of the fixture directory `case`.
    fn listed_in(
        case: &str,
        contract: &str,
    ) -> (Vec<(String, Option<u64>)>, Resources<'static>, Fields) {
        let (db, _, program) = lower_case(case);
        let sources = SourceTable::from_program(db, &program);
        let env = Env {
            db,
            sources: &sources,
        };
        let mut resources = Resources::new();
        let facts = &program.debug.contracts[contract];
        let fields = fields(&env, &mut resources, contract, facts);
        let slots = fields
            .per_instruction
            .iter()
            .map(|field| {
                let slot = field.pointer.as_ref().map(|pointer| match pointer {
                    Pointer::Storage { slot, .. } => *slot,
                    Pointer::Stack { .. } => unreachable!("fields are in storage"),
                });
                (field.identifier.clone(), slot)
            })
            .collect();
        (slots, resources, fields)
    }

    fn named(slots: &[(&str, Option<u64>)]) -> Vec<(String, Option<u64>)> {
        slots
            .iter()
            .map(|(name, slot)| ((*name).to_owned(), *slot))
            .collect()
    }

    #[test]
    fn a_field_has_a_pointer_when_its_slot_is_the_sum_of_sizes_std_states() {
        run_in_large_stack(|| {
            let (slots, resources, fields) = listed("Early");
            // `b` and `d` are one slot each in std, but are no whole word of
            // their own; `e` to `g` are mappings, and `h` follows them.
            assert_eq!(
                slots,
                named(&[
                    ("a", Some(0)),
                    ("b", None),
                    ("c", Some(2)),
                    ("d", None),
                    ("e", None),
                    ("f", None),
                    ("g", None),
                    ("h", Some(7)),
                ])
            );
            // Every field has a type, and only the program-level list has
            // declarations.
            assert!(
                fields
                    .per_instruction
                    .iter()
                    .all(|field| field.ty.is_some())
            );
            assert!(
                fields
                    .per_instruction
                    .iter()
                    .all(|f| f.declaration.is_none())
            );
            assert!(
                fields
                    .program
                    .iter()
                    .all(|field| field.declaration.is_some())
            );
            assert_eq!(fields.program.len(), fields.per_instruction.len());
            // Only a mapping from one-word keys to one-word values has a template,
            // named for the contract and the field, at the field's own slot.
            assert_eq!(
                resources.templates(),
                [(
                    "solcore$Early$e$entry".to_owned(),
                    Template::MappingEntry { slot: 4 }
                )]
            );
        });
    }

    #[test]
    fn no_field_after_one_of_unstated_size_has_a_pointer() {
        run_in_large_stack(|| {
            // A `bool` has no explicit `StorageSize` instance, and the table leaves
            // out std's instances for `uint8` and the other integer widths, so
            // no field after either has a pointer.
            let (slots, resources, _) = listed("AfterBool");
            assert_eq!(
                slots,
                named(&[("a", Some(0)), ("b", None), ("c", None), ("d", None)])
            );
            assert!(resources.templates().is_empty());
            let (slots, resources, fields) = listed("Types");
            assert!(slots.iter().all(|(_, slot)| slot.is_none()));
            assert!(resources.templates().is_empty());
            assert_eq!(fields.per_instruction.len(), 7);
        });
    }

    #[test]
    fn no_field_after_a_unit_field_has_a_pointer() {
        run_in_large_stack(|| {
            // Hull gives `u` a slot and std sizes it 0, so `w` is at slot 2 where
            // Hull lowers the access and at slot 1 where the specializer does.
            // `w` has no pointer; `v`, before the unit field, keeps its own.
            for case in ["no-std", "types"] {
                let (slots, _, fields) = listed_in(case, "AfterUnit");
                assert_eq!(
                    slots,
                    named(&[("v", Some(0)), ("u", None), ("w", None)]),
                    "{case}"
                );
                // `w` is still listed, with its type.
                assert!(fields.per_instruction[2].ty.is_some(), "{case}");
            }
        });
    }

    #[test]
    fn a_field_of_a_type_with_three_constructors_is_listed_without_a_type() {
        run_in_large_stack(|| {
            let (slots, _, fields) = listed_in("no-std", "Tagged");
            // `Shape` (three constructors) and `Holder` (a struct that holds it)
            // have no type; `Flip` (two constructors) has one. A field with no
            // type is still listed, and fields around it keep their pointers
            // where every earlier field is one word.
            assert_eq!(
                slots,
                named(&[
                    ("a", Some(0)),
                    ("s", None),
                    ("h", None),
                    ("f", None),
                    ("w", None)
                ])
            );
            let has_type: Vec<bool> = fields
                .per_instruction
                .iter()
                .map(|field| field.ty.is_some())
                .collect();
            assert_eq!(has_type, [true, false, false, true, true]);
        });
    }
}
