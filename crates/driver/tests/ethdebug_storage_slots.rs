//! The storage locations that `--ethdebug` advertises, checked against the
//! storage of the running code.
//!
//! Each case compiles a fixture, deploys one of its contracts in revm and calls
//! it. The call assigns known values to storage fields. The checks:
//!
//! 1. exactly the expected fields have a storage pointer, and a pointer names
//!    the slot at the field's declaration index (the slot that holds the field
//!    when every earlier field takes one slot);
//! 2. the word in the slot that a pointer names is the value the program
//!    assigned to that field, or 0 for a field it never assigned;
//! 3. for a mapping with a pointer template, the word in the slot that the
//!    template computes for a key is the value assigned for that key, and a
//!    mapping that has no template has none in the resources either;
//! 4. fields that have no pointer are not claimed to be anywhere: where a case
//!    states where the program really put one, the word is there, which shows
//!    what the missing pointer would have had to say.

use std::collections::BTreeSet;

use revm::primitives::{U256, keccak256};
use serde_json::Value;

#[path = "support/ethdebug_output.rs"]
mod ethdebug_output;

use ethdebug_output::{CALLER, call_data, compile, execute, repo_root};

/// A contract of a fixture and what its `main()` does to its storage.
struct Case {
    fixture: &'static str,
    contract: &'static str,
    /// Whether the contract dispatches on a selector (it imports `std.dispatch`).
    dispatches: bool,
    /// The fields that must have a storage pointer, in declaration order.
    pointers: &'static [&'static str],
    /// A field with a pointer and the word its slot holds after the call.
    held: &'static [(&'static str, u64)],
    /// A mapping field with a template, a key and the word the entry holds.
    entries: &'static [(&'static str, Key, u64)],
    /// Mapping fields that have a template; every other has none.
    templates: &'static [&'static str],
    /// Slots that the program wrote for fields without a pointer, and the words.
    unlocated: &'static [(u64, u64)],
}

#[derive(Clone, Copy)]
enum Key {
    Word(u64),
    Caller,
}

const CASES: &[Case] = &[
    Case {
        fixture: "storage-slots",
        contract: "Slots",
        dispatches: true,
        pointers: &["first", "owner", "second", "digest", "last"],
        held: &[
            ("first", 11),
            ("owner", 0x22),
            ("second", 33),
            ("digest", 44),
            ("last", 55),
        ],
        entries: &[
            ("balances", Key::Word(0x66), 66),
            ("marks", Key::Word(7), 77),
        ],
        templates: &["balances", "marks"],
        unlocated: &[],
    },
    // A bool has no explicit size in std, so nothing after it is known.
    Case {
        fixture: "storage-slots",
        contract: "AfterBool",
        dispatches: true,
        pointers: &["head"],
        held: &[("head", 1)],
        entries: &[],
        templates: &[],
        // `flag` at slot 1, `tail` at slot 2.
        unlocated: &[(1, 1), (2, 3)],
    },
    Case {
        fixture: "storage-fields",
        contract: "StorageFields",
        dispatches: true,
        pointers: &["count", "untouched"],
        held: &[("count", 17), ("untouched", 0)],
        entries: &[("balances", Key::Caller, 17)],
        templates: &["balances"],
        unlocated: &[],
    },
    // A module that sees no std class: slots follow the declaration order, and
    // a `()` field takes one.
    Case {
        fixture: "no-std",
        contract: "AfterUnit",
        dispatches: false,
        pointers: &["v"],
        held: &[("v", 3)],
        entries: &[],
        templates: &[],
        unlocated: &[(2, 7)],
    },
    Case {
        fixture: "no-std",
        contract: "Tagged",
        dispatches: false,
        pointers: &["a"],
        held: &[("a", 5)],
        entries: &[],
        templates: &[],
        unlocated: &[(4, 9)],
    },
    // The size table of std: strings, memory strings and mappings take one slot.
    Case {
        fixture: "types",
        contract: "Early",
        dispatches: true,
        pointers: &["a", "c", "h"],
        held: &[("a", 1), ("c", 3), ("h", 8)],
        entries: &[],
        templates: &["e"],
        unlocated: &[],
    },
    Case {
        fixture: "types",
        contract: "AfterBool",
        dispatches: true,
        pointers: &["a"],
        held: &[("a", 1)],
        entries: &[],
        templates: &[],
        unlocated: &[(2, 3)],
    },
    // A `()` field is one slot in Hull and none in std: a module that sees std
    // gives the field after it no pointer.
    Case {
        fixture: "types",
        contract: "AfterUnit",
        dispatches: true,
        pointers: &["v"],
        held: &[("v", 3)],
        entries: &[],
        templates: &[],
        unlocated: &[(1, 7)],
    },
    Case {
        fixture: "types",
        contract: "Types",
        dispatches: true,
        pointers: &[],
        held: &[],
        entries: &[],
        templates: &[],
        unlocated: &[],
    },
];

/// The slot a pointer template computes for `key`. A template is a tree of
/// `{"~keccak256": [...]}` and `{"~wordsized": n | "key"}`; the hash is of the
/// concatenated words.
fn evaluate(expression: &Value, key: U256) -> Vec<u8> {
    let object = expression.as_object().expect("an object");
    let (operator, operand) = object.iter().next().expect("one operator");
    assert_eq!(object.len(), 1, "{expression}");
    match operator.as_str() {
        "~wordsized" => {
            let word = match operand {
                Value::String(name) => {
                    assert_eq!(name, "key");
                    key
                }
                Value::Number(number) => U256::from(number.as_u64().unwrap()),
                other => panic!("{other}"),
            };
            word.to_be_bytes::<32>().to_vec()
        }
        "~keccak256" => {
            let parts: Vec<u8> = operand
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|part| evaluate(part, key))
                .collect();
            keccak256(&parts).to_vec()
        }
        other => panic!("unknown operator {other}"),
    }
}

fn run(case: &Case) {
    let name = format!("{} {}", case.fixture, case.contract);
    let dir = repo_root().join("tests/ethdebug").join(case.fixture);
    let output = compile(&dir, &["--ethdebug"]);
    let [_, (runtime_program, runtime)] = output.programs(case.contract);
    let creation = output.bytes(&format!("{}.bin", case.contract));
    let resources = output.resources();

    // The fields of the program-level context, in declaration order.
    let fields = runtime_program["context"]["variables"].as_array().unwrap();
    let slot_of = |field: &str| -> Option<U256> {
        let variable = fields
            .iter()
            .find(|variable| variable["identifier"] == field)
            .unwrap_or_else(|| panic!("{name}: no field `{field}`"));
        let pointer = variable.get("pointer")?;
        assert_eq!(pointer["location"], "storage", "{name}: {variable}");
        assert_eq!(pointer["name"], field, "{name}: {variable}");
        Some(U256::from(pointer["slot"].as_u64().unwrap()))
    };

    // 1. Which fields have a pointer, and where it points.
    let with_pointer: Vec<&str> = fields
        .iter()
        .filter(|variable| variable.get("pointer").is_some())
        .map(|variable| variable["identifier"].as_str().unwrap())
        .collect();
    assert_eq!(with_pointer, case.pointers, "{name}: fields with a pointer");

    // Run `main()` once.
    let data = if case.dispatches {
        call_data("main()", &[])
    } else {
        Vec::new()
    };
    let execution = execute(&creation, &runtime, &[data]);
    let stored = |slot: U256| execution.storage.get(&slot).copied().unwrap_or_default();

    // 2. What the advertised slots hold.
    for (field, value) in case.held {
        let slot = slot_of(field).unwrap_or_else(|| panic!("{name}: `{field}` has no pointer"));
        assert_eq!(
            stored(slot),
            U256::from(*value),
            "{name}: the slot {slot} that `{field}` advertises"
        );
    }
    // The slot of a pointer is the declaration index of its field.
    for (index, field) in fields.iter().enumerate() {
        if let Some(slot) = slot_of(field["identifier"].as_str().unwrap()) {
            assert_eq!(slot, U256::from(index), "{name}: slot of `{field}`");
        }
    }
    // 3. The entries of mappings.
    let templates = resources["pointers"].as_object().unwrap();
    let prefix = format!("solcore${}$", case.contract);
    let found: BTreeSet<&str> = templates
        .keys()
        .filter_map(|key| key.strip_prefix(&prefix)?.strip_suffix("$entry"))
        .collect();
    assert_eq!(
        found,
        case.templates.iter().copied().collect(),
        "{name}: mappings with a template"
    );
    for (field, key, value) in case.entries {
        let template = &templates[&format!("{prefix}{field}$entry")];
        assert_eq!(template["expect"], serde_json::json!(["key"]));
        assert_eq!(template["for"]["location"], "storage");
        let key = match key {
            Key::Word(word) => U256::from(*word),
            Key::Caller => U256::from_be_slice(CALLER.as_slice()),
        };
        let slot = U256::from_be_slice(&evaluate(&template["for"]["slot"], key));
        assert_eq!(
            stored(slot),
            U256::from(*value),
            "{name}: the entry of `{field}` that its template locates"
        );
    }
    // 4. Fields without a pointer.
    for (slot, value) in case.unlocated {
        assert_eq!(
            stored(U256::from(*slot)),
            U256::from(*value),
            "{name}: slot {slot}"
        );
    }
    println!(
        "{name}: {} pointers and {} entries checked against {} stored words",
        case.held.len(),
        case.entries.len(),
        execution.storage.len()
    );
}

#[test]
fn storage_pointers_name_the_slots_the_running_code_uses() {
    for case in CASES {
        run(case);
    }
}
