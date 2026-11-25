//! Writer for the stock ethdebug format: programs, resources and the compilation.
//!
//! Plain data in, JSON out. The crate knows nothing about the compiler that
//! produced the data. Field names and required fields follow the schemas of
//! ethdebug/format at commit 952f1bd, specification version 0.1.0-draft.3:
//! `program`, `program/instruction`, `program/context`,
//! `program/context/variables`, `pointer`, `materials/compilation`,
//! `materials/source`, `materials/source-range` and `info/resources`. The schemas
//! reject unknown fields, so nothing else is written.
//!
//! The one exception is the type objects, which follow the draft schema of
//! ethdebug/format pull request #424; see [`draft_types`].

pub mod draft_types;

use std::{error, fmt};

use serde_json::{Map, Value, json};

pub use draft_types::{TYPES_DRAFT, TypeDesc, TypeRef};

const VERSION: &str = "0.1.0-draft.3";

/// One source file of a compilation.
pub struct Source<'a> {
    pub id: u32,
    pub path: &'a str,
    pub contents: &'a str,
    pub language: &'a str,
}

/// One compiler invocation and the sources it read.
pub struct Compilation<'a> {
    pub id: &'a str,
    pub compiler_name: &'a str,
    pub compiler_version: &'a str,
    pub sources: &'a [Source<'a>],
}

/// Whether a program describes creation code or runtime code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Create,
    Call,
}

/// What is known about a range of code bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// The bytes carry out `length` bytes of source `source`, from `offset`.
    Code {
        source: u32,
        offset: u32,
        length: u32,
    },
    /// A human-readable note and no source range.
    Remark(String),
    /// Nothing is known: the instruction gets no `context`.
    None,
}

/// The context of the code bytes `start..end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub start: u32,
    pub end: u32,
    pub context: Context,
}

/// Where a variable's value is, as far as the compiler can state it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pointer {
    /// The whole word at a constant storage slot. `name` is written only when
    /// it is a valid pointer identifier.
    Storage { name: String, slot: u64 },
    /// The whole word at a stack slot, counted from the top of the stack at the
    /// time the context holds: slot 0 is the top. The schema says "Describes
    /// stack slots as number of positions from the top (at time of
    /// observation)", and without a `length` the region is the whole slot.
    Stack { slot: u64 },
}

/// A pointer defined in terms of variables that the reader supplies, stored in
/// the resources document under a name chosen by the compiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Template {
    /// The entry of a mapping whose own slot is `slot`: the word at
    /// `keccak256(slot ++ key)`, both operands one word wide. The reader gives
    /// the variable `key`.
    MappingEntry { slot: u64 },
}

/// One variable of a context. Nothing is known about a value without `pointer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub identifier: String,
    /// Source id, offset and length of the declaring identifier.
    pub declaration: Option<(u32, u32, u32)>,
    pub ty: Option<TypeRef>,
    pub pointer: Option<Pointer>,
}

/// One bytecode and what is known about it.
pub struct Program<'a> {
    pub contract: &'a str,
    /// Source id, offset and length of the contract's definition.
    pub definition: (u32, u32, u32),
    pub environment: Environment,
    pub compilation_id: &'a str,
    /// The code region only, without any data that follows it.
    pub code: &'a [u8],
    /// Must tile `code` exactly: in order, without gaps or overlaps.
    pub ranges: &'a [Range],
    /// The index in `variable_lists` of the variables that hold before the first
    /// instruction. A list without variables writes no context.
    pub program_variables: Option<u32>,
    /// The index in `variable_lists` of the variables that hold after the
    /// instruction at each offset. Sorted by offset, strictly ascending; every
    /// offset must be the start of an instruction.
    pub variables_at: &'a [(u32, u32)],
    pub variable_lists: &'a [Vec<Variable>],
}

/// A program that cannot be written as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    fn new(message: String) -> Self {
        Self { message }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl error::Error for Error {}

fn stamp(schema: &str) -> Value {
    json!({ "schema": schema, "version": VERSION })
}

/// The compilation alone (`materials/compilation`).
pub fn compilation_json(compilation: &Compilation<'_>) -> Value {
    let sources: Vec<Value> = compilation
        .sources
        .iter()
        .map(|source| {
            json!({
                "id": source.id,
                "path": source.path,
                "contents": source.contents,
                "language": source.language,
            })
        })
        .collect();
    json!({
        "id": compilation.id,
        "compiler": {
            "name": compilation.compiler_name,
            "version": compilation.compiler_version,
        },
        "settings": draft_types::compilation_settings(),
        "sources": sources,
    })
}

/// The resources object (`info/resources`).
///
/// `types[n]` is written under the key `t<n>` (see [`draft_types::type_key`]),
/// which is what a [`TypeRef::Id`] names. `templates` are pointer templates by
/// name; a name must be a valid pointer identifier ([`is_pointer_identifier`]).
pub fn resources_json(
    compilation: &Compilation<'_>,
    types: &[TypeDesc],
    templates: &[(String, Template)],
) -> Value {
    let types: Map<String, Value> = types
        .iter()
        .enumerate()
        .map(|(n, desc)| (draft_types::type_key(n as u32), desc.to_json()))
        .collect();
    let pointers: Map<String, Value> = templates
        .iter()
        .map(|(name, template)| (name.clone(), template_json(template)))
        .collect();
    json!({
        "ethdebug": stamp("ethdebug/format/info/resources"),
        "types": types,
        "pointers": pointers,
        "compilation": compilation_json(compilation),
    })
}

/// Whether `name` matches `pointer/identifier`: letters, `$`, `_` and `-`, then
/// those and digits.
pub fn is_pointer_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let allowed = |c: char| c.is_ascii_alphabetic() || matches!(c, '$' | '_' | '-');
    chars.next().is_some_and(allowed) && chars.all(|c| allowed(c) || c.is_ascii_digit())
}

fn template_json(template: &Template) -> Value {
    match template {
        Template::MappingEntry { slot } => json!({
            "expect": ["key"],
            "for": {
                "name": "entry",
                "location": "storage",
                "slot": { "~keccak256": [{ "~wordsized": slot }, { "~wordsized": "key" }] },
            },
        }),
    }
}

fn pointer_json(pointer: &Pointer) -> Value {
    match pointer {
        Pointer::Storage { name, slot } => {
            let mut out = Map::new();
            if is_pointer_identifier(name) {
                out.insert("name".to_owned(), json!(name));
            }
            out.insert("location".to_owned(), json!("storage"));
            out.insert("slot".to_owned(), json!(slot));
            Value::Object(out)
        }
        Pointer::Stack { slot } => json!({"location": "stack", "slot": slot}),
    }
}

fn variables_json(variables: &[Variable]) -> Value {
    let variables: Vec<Value> = variables
        .iter()
        .map(|variable| {
            let mut out = Map::new();
            out.insert("identifier".to_owned(), json!(variable.identifier));
            if let Some((source, offset, length)) = variable.declaration {
                out.insert(
                    "declaration".to_owned(),
                    source_range(source, offset, length),
                );
            }
            if let Some(ty) = &variable.ty {
                out.insert("type".to_owned(), ty.to_json());
            }
            if let Some(pointer) = &variable.pointer {
                out.insert("pointer".to_owned(), pointer_json(pointer));
            }
            Value::Object(out)
        })
        .collect();
    Value::Array(variables)
}

/// The offsets of the instructions of `code`, in order. A PUSH whose immediate
/// is cut off by the end of the code still counts as one instruction.
pub fn instruction_offsets(code: &[u8]) -> Vec<u32> {
    let mut offsets = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        offsets.push(pc as u32);
        pc += 1 + push_data_len(code[pc]);
    }
    offsets
}

/// A program object with one instruction per opcode of `program.code`.
///
/// Each instruction gets the context of the range that contains its offset, and
/// the variables of the list that `variables_at` names for its offset.
/// Refused: ranges that are empty, unsorted, overlapping, leave a gap, pass the
/// end of the code or start inside an instruction; a code context, a variable
/// declaration or the definition naming a source that is not in `sources` or a
/// range that cannot be sliced as UTF-8 text; a variable list that does not
/// exist; `variables_at` entries that are unsorted or not at an instruction.
pub fn program_json(program: &Program<'_>, sources: &[Source<'_>]) -> Result<Value, Error> {
    let (def_source, def_offset, def_length) = program.definition;
    check_source(sources, def_source, def_offset, def_length).map_err(|why| {
        Error::new(format!(
            "definition of `{}` is invalid: {why}",
            program.contract
        ))
    })?;
    check_ranges(program.ranges, program.code.len(), sources)?;
    check_variables(program, sources)?;

    let mut instructions = Vec::new();
    let (mut pc, mut range) = (0, 0);
    let mut starts = Vec::new();
    while pc < program.code.len() {
        starts.push(pc);
        while program.ranges[range].end as usize <= pc {
            range += 1;
        }
        let opcode = program.code[pc];
        let mut instruction = json!({
            "offset": pc,
            "operation": { "mnemonic": mnemonic(opcode) },
        });
        let mut context = Map::new();
        match &program.ranges[range].context {
            Context::Code {
                source,
                offset,
                length,
            } => {
                context.insert("code".to_owned(), source_range(*source, *offset, *length));
            }
            Context::Remark(text) => {
                context.insert("remark".to_owned(), json!(text));
            }
            Context::None => {}
        }
        let at = program
            .variables_at
            .binary_search_by_key(&(pc as u32), |(offset, _)| *offset);
        if let Ok(at) = at {
            let list = &program.variable_lists[program.variables_at[at].1 as usize];
            if !list.is_empty() {
                context.insert("variables".to_owned(), variables_json(list));
            }
        }
        if !context.is_empty() {
            instruction["context"] = Value::Object(context);
        }
        instructions.push(instruction);
        pc += 1 + push_data_len(opcode);
    }
    // A range that starts inside a PUSH immediate would be skipped silently.
    for Range { start, end, .. } in program.ranges {
        if starts.binary_search(&(*start as usize)).is_err() {
            let why = "does not start at an instruction";
            return Err(Error::new(format!("range {start}..{end} {why}")));
        }
    }

    for (offset, _) in program.variables_at {
        if starts.binary_search(&(*offset as usize)).is_err() {
            return Err(Error::new(format!(
                "variables at {offset} are not at an instruction"
            )));
        }
    }

    let mut out = json!({
        "ethdebug": stamp("ethdebug/format/program"),
        "compilation": { "id": program.compilation_id },
        "contract": {
            "name": program.contract,
            "definition": source_range(def_source, def_offset, def_length),
        },
        "environment": match program.environment {
            Environment::Create => "create",
            Environment::Call => "call",
        },
        "instructions": instructions,
    });
    if let Some(list) = program.program_variables {
        let list = &program.variable_lists[list as usize];
        if !list.is_empty() {
            out["context"] = json!({ "variables": variables_json(list) });
        }
    }
    Ok(out)
}

fn check_variables(program: &Program<'_>, sources: &[Source<'_>]) -> Result<(), Error> {
    let lists = program.variable_lists.len();
    let missing = |list: u32| Error::new(format!("variable list {list} does not exist ({lists})"));
    if let Some(list) = program.program_variables
        && list as usize >= lists
    {
        return Err(missing(list));
    }
    for window in program.variables_at.windows(2) {
        if window[0].0 >= window[1].0 {
            return Err(Error::new(format!(
                "variables at {} follow variables at {}",
                window[1].0, window[0].0
            )));
        }
    }
    for (_, list) in program.variables_at {
        if *list as usize >= lists {
            return Err(missing(*list));
        }
    }
    for variable in program.variable_lists.iter().flatten() {
        if variable.identifier.is_empty() {
            return Err(Error::new("a variable has no identifier".to_owned()));
        }
        if let Some((source, offset, length)) = variable.declaration {
            check_source(sources, source, offset, length).map_err(|why| {
                Error::new(format!(
                    "declaration of `{}` is invalid: {why}",
                    variable.identifier
                ))
            })?;
        }
    }
    Ok(())
}

fn source_range(source: u32, offset: u32, length: u32) -> Value {
    json!({ "source": { "id": source }, "range": { "offset": offset, "length": length } })
}

fn check_ranges(ranges: &[Range], code_len: usize, sources: &[Source<'_>]) -> Result<(), Error> {
    let mut cursor = 0;
    for Range {
        start,
        end,
        context,
    } in ranges
    {
        if end <= start {
            return Err(Error::new(format!("range {start}..{end} is empty")));
        }
        if *start < cursor {
            return Err(Error::new(format!(
                "range {start}..{end} starts before {cursor}, where the previous range ended"
            )));
        }
        if *start > cursor {
            return Err(Error::new(format!(
                "bytes {cursor}..{start} are in no range"
            )));
        }
        if let Context::Code {
            source,
            offset,
            length,
        } = context
        {
            check_source(sources, *source, *offset, *length)
                .map_err(|why| Error::new(format!("range {start}..{end}: {why}")))?;
        }
        cursor = *end;
    }
    if cursor as usize != code_len {
        return Err(Error::new(format!(
            "ranges end at {cursor}, but the code has {code_len} bytes"
        )));
    }
    Ok(())
}

/// The source `id` must be listed and hold the text `offset..offset + length`.
fn check_source(sources: &[Source<'_>], id: u32, offset: u32, length: u32) -> Result<(), String> {
    let source = sources
        .iter()
        .find(|source| source.id == id)
        .ok_or_else(|| format!("source {id} is not in the source list"))?;
    let end = u64::from(offset) + u64::from(length);
    if end > source.contents.len() as u64 {
        return Err(format!(
            "source {id} has {} bytes, but the range is {offset}..{end}",
            source.contents.len()
        ));
    }
    if !source.contents.is_char_boundary(offset as usize)
        || !source.contents.is_char_boundary(end as usize)
    {
        return Err(format!(
            "range {offset}..{end} of source {id} splits a UTF-8 character"
        ));
    }
    Ok(())
}

/// Number of immediate bytes that follow `opcode` (PUSH1 to PUSH32).
fn push_data_len(opcode: u8) -> usize {
    match opcode {
        0x60..=0x7f => usize::from(opcode - 0x5f),
        _ => 0,
    }
}

/// Opcodes with a name of their own, as of the Osaka fork. The others are
/// `PUSH0` to `PUSH32`, `DUP1` to `DUP16`, `SWAP1` to `SWAP16`, `LOG0` to
/// `LOG4`, and `INVALID` for every byte that is not an opcode.
#[rustfmt::skip]
const NAMED: &[(u8, &str)] = &[
    (0x00, "STOP"), (0x01, "ADD"), (0x02, "MUL"), (0x03, "SUB"), (0x04, "DIV"), (0x05, "SDIV"),
    (0x06, "MOD"), (0x07, "SMOD"), (0x08, "ADDMOD"), (0x09, "MULMOD"), (0x0a, "EXP"),
    (0x0b, "SIGNEXTEND"), (0x10, "LT"), (0x11, "GT"), (0x12, "SLT"), (0x13, "SGT"), (0x14, "EQ"),
    (0x15, "ISZERO"), (0x16, "AND"), (0x17, "OR"), (0x18, "XOR"), (0x19, "NOT"), (0x1a, "BYTE"),
    (0x1b, "SHL"), (0x1c, "SHR"), (0x1d, "SAR"), (0x1e, "CLZ"), (0x20, "KECCAK256"),
    (0x30, "ADDRESS"), (0x31, "BALANCE"), (0x32, "ORIGIN"), (0x33, "CALLER"), (0x34, "CALLVALUE"),
    (0x35, "CALLDATALOAD"), (0x36, "CALLDATASIZE"), (0x37, "CALLDATACOPY"), (0x38, "CODESIZE"),
    (0x39, "CODECOPY"), (0x3a, "GASPRICE"), (0x3b, "EXTCODESIZE"), (0x3c, "EXTCODECOPY"),
    (0x3d, "RETURNDATASIZE"), (0x3e, "RETURNDATACOPY"), (0x3f, "EXTCODEHASH"),
    (0x40, "BLOCKHASH"), (0x41, "COINBASE"), (0x42, "TIMESTAMP"), (0x43, "NUMBER"),
    (0x44, "PREVRANDAO"), (0x45, "GASLIMIT"), (0x46, "CHAINID"), (0x47, "SELFBALANCE"),
    (0x48, "BASEFEE"), (0x49, "BLOBHASH"), (0x4a, "BLOBBASEFEE"), (0x50, "POP"), (0x51, "MLOAD"),
    (0x52, "MSTORE"), (0x53, "MSTORE8"), (0x54, "SLOAD"), (0x55, "SSTORE"), (0x56, "JUMP"),
    (0x57, "JUMPI"), (0x58, "PC"), (0x59, "MSIZE"), (0x5a, "GAS"), (0x5b, "JUMPDEST"),
    (0x5c, "TLOAD"), (0x5d, "TSTORE"), (0x5e, "MCOPY"), (0xf0, "CREATE"), (0xf1, "CALL"),
    (0xf2, "CALLCODE"), (0xf3, "RETURN"), (0xf4, "DELEGATECALL"), (0xf5, "CREATE2"),
    (0xfa, "STATICCALL"), (0xfd, "REVERT"), (0xfe, "INVALID"), (0xff, "SELFDESTRUCT"),
];

fn mnemonic(opcode: u8) -> String {
    match opcode {
        0x5f..=0x7f => format!("PUSH{}", opcode - 0x5f),
        0x80..=0x8f => format!("DUP{}", opcode - 0x7f),
        0x90..=0x9f => format!("SWAP{}", opcode - 0x8f),
        0xa0..=0xa4 => format!("LOG{}", opcode - 0xa0),
        _ => NAMED
            .iter()
            .find(|(code, _)| *code == opcode)
            .map_or("INVALID", |(_, name)| name)
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn source(id: u32, path: &'static str, contents: &'static str) -> Source<'static> {
        Source {
            id,
            path,
            contents,
            language: "Core Solidity",
        }
    }

    const SOURCES: [Source<'static>; 2] = [
        source(0, "main.sol", "0123456789"),
        source(3, "std/std.sol", "abc"),
    ];

    /// PUSH1 1, PUSH0, SSTORE, STOP, and a PUSH2 whose immediate is cut off.
    const CODE: [u8; 6] = [0x60, 0x01, 0x5f, 0x55, 0x00, 0x61];

    fn range(start: u32, end: u32, context: Context) -> Range {
        Range {
            start,
            end,
            context,
        }
    }

    fn code(source: u32, offset: u32, length: u32) -> Context {
        Context::Code {
            source,
            offset,
            length,
        }
    }

    fn write(
        code_bytes: &[u8],
        definition: (u32, u32, u32),
        ranges: &[Range],
    ) -> Result<Value, Error> {
        let program = Program {
            contract: "C",
            definition,
            environment: Environment::Call,
            compilation_id: "solcore-1",
            code: code_bytes,
            ranges,
            program_variables: None,
            variables_at: &[],
            variable_lists: &[],
        };
        program_json(&program, &SOURCES)
    }

    #[test]
    fn program_gives_each_opcode_the_context_of_the_range_that_holds_its_offset() {
        // The first range holds the PUSH1 at 0 and the PUSH0 at 2, the next
        // two hold one instruction each, and the last holds the cut-off PUSH2.
        let ranges = [
            range(0, 3, code(0, 4, 2)),
            range(3, 4, Context::Remark("generated: entry".to_owned())),
            range(4, 5, Context::None),
            range(5, 6, code(3, 0, 3)),
        ];
        assert_eq!(
            write(&CODE, (0, 2, 5), &ranges).unwrap(),
            json!({
                "ethdebug": {"schema": "ethdebug/format/program", "version": "0.1.0-draft.3"},
                "compilation": {"id": "solcore-1"},
                "contract": {
                    "name": "C",
                    "definition": {"source": {"id": 0}, "range": {"offset": 2, "length": 5}},
                },
                "environment": "call",
                "instructions": [
                    {"offset": 0, "operation": {"mnemonic": "PUSH1"},
                     "context": {"code": {"source": {"id": 0}, "range": {"offset": 4, "length": 2}}}},
                    {"offset": 2, "operation": {"mnemonic": "PUSH0"},
                     "context": {"code": {"source": {"id": 0}, "range": {"offset": 4, "length": 2}}}},
                    {"offset": 3, "operation": {"mnemonic": "SSTORE"},
                     "context": {"remark": "generated: entry"}},
                    {"offset": 4, "operation": {"mnemonic": "STOP"}},
                    {"offset": 5, "operation": {"mnemonic": "PUSH2"},
                     "context": {"code": {"source": {"id": 3}, "range": {"offset": 0, "length": 3}}}},
                ],
            })
        );
    }

    #[test]
    fn program_refuses_ranges_that_do_not_tile_the_code_or_split_an_instruction() {
        let refused = |ranges: &[Range], expected: &str| {
            let message = write(&CODE, (0, 0, 1), ranges).unwrap_err().to_string();
            assert!(message.contains(expected), "{message}");
        };
        let none = |start, end| range(start, end, Context::None);
        refused(&[none(0, 4), none(3, 6)], "starts before 4");
        refused(&[none(3, 6), none(0, 3)], "are in no range");
        refused(&[none(0, 2), none(3, 6)], "bytes 2..3 are in no range");
        refused(&[none(1, 6)], "bytes 0..1 are in no range");
        refused(&[none(0, 5)], "ranges end at 5, but the code has 6 bytes");
        refused(&[none(0, 7)], "ranges end at 7, but the code has 6 bytes");
        refused(&[none(0, 3), none(3, 3), none(3, 6)], "is empty");
        refused(&[], "ranges end at 0, but the code has 6 bytes");
        // The PUSH1 at 0 owns the byte at 1.
        refused(
            &[none(0, 1), none(1, 6)],
            "1..6 does not start at an instruction",
        );
        refused(&[none(0, 1), none(1, 2), none(2, 6)], "1..2 does not start");
    }

    #[test]
    fn program_refuses_a_source_that_is_not_in_the_list_and_text_outside_it() {
        let message = write(&CODE, (0, 0, 1), &[range(0, 6, code(1, 0, 1))])
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("source 1 is not in the source list"),
            "{message}"
        );
        let message = write(&[], (9, 0, 1), &[]).unwrap_err().to_string();
        assert!(message.contains("definition of `C`"), "{message}");

        // Source 0 has 10 bytes.
        for definition in [(0, 8, 3), (0, 11, 0)] {
            let message = write(&CODE, definition, &[range(0, 6, Context::None)])
                .unwrap_err()
                .to_string();
            assert!(message.contains("definition of `C`"), "{message}");
        }
        let message = write(&CODE, (0, 0, 1), &[range(0, 6, code(0, 8, 3))])
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("source 0 has 10 bytes, but the range is 8..11"),
            "{message}"
        );
        assert!(write(&CODE, (0, 7, 3), &[range(0, 6, code(0, 8, 2))]).is_ok());
    }

    #[test]
    fn program_requires_source_ranges_to_end_at_utf8_boundaries() {
        let sources = [source(0, "main.sol", "aπb\r\n")];
        for (offset, length, valid) in [(1, 2, true), (2, 1, false), (1, 1, false)] {
            let ranges = [range(0, 1, code(0, offset, length))];
            let program = Program {
                contract: "C",
                definition: (0, 0, 6),
                environment: Environment::Call,
                compilation_id: "solcore-1",
                code: &[0x00],
                ranges: &ranges,
                program_variables: None,
                variables_at: &[],
                variable_lists: &[],
            };
            let result = program_json(&program, &sources);
            if valid {
                assert!(result.is_ok(), "{result:?}");
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .message()
                        .contains("splits a UTF-8 character")
                );
            }
        }
    }

    #[test]
    fn resources_and_compilation_list_the_sources_with_their_text() {
        let compilation = Compilation {
            id: "solcore-1",
            compiler_name: "solcore",
            compiler_version: "0.1.0",
            sources: &SOURCES,
        };
        let expected = json!({
            "id": "solcore-1",
            "compiler": {"name": "solcore", "version": "0.1.0"},
            "settings": {"ethdebugTypes": TYPES_DRAFT},
            "sources": [
                {"id": 0, "path": "main.sol", "contents": "0123456789", "language": "Core Solidity"},
                {"id": 3, "path": "std/std.sol", "contents": "abc", "language": "Core Solidity"},
            ],
        });
        assert_eq!(compilation_json(&compilation), expected);
        assert_eq!(
            resources_json(&compilation, &[], &[]),
            json!({
                "ethdebug": {"schema": "ethdebug/format/info/resources", "version": "0.1.0-draft.3"},
                "types": {},
                "pointers": {},
                "compilation": expected,
            })
        );
    }

    #[test]
    fn resources_name_types_by_position_and_list_pointer_templates() {
        let compilation = Compilation {
            id: "solcore-1",
            compiler_name: "solcore",
            compiler_version: "0.1.0",
            sources: &[],
        };
        let types = [
            TypeDesc::Struct {
                label: None,
                definition: None,
                members: Vec::new(),
            },
            TypeDesc::Bool {
                label: "bool".to_owned(),
            },
        ];
        let templates = [(
            "solcore$C$balances$entry".to_owned(),
            Template::MappingEntry { slot: 3 },
        )];
        let resources = resources_json(&compilation, &types, &templates);
        assert_eq!(
            resources["types"],
            json!({
                "t0": {"kind": "struct", "contains": []},
                "t1": {"kind": "bool", "label": "bool"},
            })
        );
        assert_eq!(
            resources["pointers"],
            json!({
                "solcore$C$balances$entry": {
                    "expect": ["key"],
                    "for": {
                        "name": "entry",
                        "location": "storage",
                        "slot": {"~keccak256": [{"~wordsized": 3}, {"~wordsized": "key"}]},
                    },
                },
            })
        );
    }

    #[test]
    fn pointer_identifiers_follow_the_pointer_schema() {
        for valid in ["a", "a0", "-$", "__init__", "$balance", "solcore$C$x$entry"] {
            assert!(is_pointer_identifier(valid), "{valid}");
        }
        for invalid in ["", "0a", "a b", "a.b", "~sum", "größe"] {
            assert!(!is_pointer_identifier(invalid), "{invalid}");
        }
    }

    fn variable(identifier: &str, declaration: Option<(u32, u32, u32)>, slot: u64) -> Variable {
        Variable {
            identifier: identifier.to_owned(),
            declaration,
            ty: Some(TypeRef::Inline(Box::new(TypeDesc::Number {
                label: "uint256".to_owned(),
            }))),
            pointer: Some(Pointer::Storage {
                name: identifier.to_owned(),
                slot,
            }),
        }
    }

    #[test]
    fn program_lists_variables_at_the_named_instructions_and_in_the_program_context() {
        let ranges = [
            range(0, 3, code(0, 4, 2)),
            range(3, 4, Context::Remark("generated: entry".to_owned())),
            range(4, 5, Context::None),
            range(5, 6, Context::None),
        ];
        let lists = [
            vec![variable("count", None, 0)],
            vec![variable("count", Some((0, 1, 5)), 0)],
            Vec::new(),
        ];
        let pointer = json!({"name": "count", "location": "storage", "slot": 0});
        let ty = json!({"kind": "number", "label": "uint256"});
        let with = |variables_at: &[(u32, u32)], program_variables| {
            let program = Program {
                contract: "C",
                definition: (0, 2, 5),
                environment: Environment::Call,
                compilation_id: "solcore-1",
                code: &CODE,
                ranges: &ranges,
                program_variables,
                variables_at,
                variable_lists: &lists,
            };
            program_json(&program, &SOURCES)
        };

        // Code and variables share one context, a remark and variables too, and
        // an instruction without either has no context.
        let json = with(&[(0, 0), (3, 0), (4, 0)], Some(1)).unwrap();
        let instructions = json["instructions"].as_array().unwrap();
        assert_eq!(
            instructions[0]["context"],
            json!({
                "code": {"source": {"id": 0}, "range": {"offset": 4, "length": 2}},
                "variables": [{"identifier": "count", "type": ty, "pointer": pointer}],
            })
        );
        assert_eq!(
            instructions[1]["context"],
            json!({"code": {"source": {"id": 0}, "range": {"offset": 4, "length": 2}}})
        );
        assert!(instructions[4].get("context").is_none());
        assert_eq!(
            instructions[2]["context"],
            json!({
                "remark": "generated: entry",
                "variables": [{"identifier": "count", "type": ty, "pointer": pointer}],
            })
        );
        assert_eq!(
            instructions[3]["context"],
            json!({"variables": [{"identifier": "count", "type": ty, "pointer": pointer}]})
        );
        assert_eq!(
            json["context"],
            json!({"variables": [{
                "identifier": "count",
                "declaration": {"source": {"id": 0}, "range": {"offset": 1, "length": 5}},
                "type": ty,
                "pointer": pointer,
            }]})
        );

        // An empty list writes no `variables` (the schema requires one item).
        let json = with(&[(0, 2)], Some(2)).unwrap();
        assert!(json.get("context").is_none());
        assert!(
            json["instructions"][0]["context"]
                .get("variables")
                .is_none()
        );
    }

    #[test]
    fn program_refuses_variable_lists_that_do_not_fit_the_code() {
        let ranges = [range(0, 6, Context::None)];
        let lists = [vec![variable("count", Some((7, 0, 1)), 0)], Vec::new()];
        let refused = |variables_at: &[(u32, u32)],
                       program_variables,
                       lists: &[Vec<Variable>],
                       expected: &str| {
            let program = Program {
                contract: "C",
                definition: (0, 2, 5),
                environment: Environment::Call,
                compilation_id: "solcore-1",
                code: &CODE,
                ranges: &ranges,
                program_variables,
                variables_at,
                variable_lists: lists,
            };
            let message = program_json(&program, &SOURCES).unwrap_err().to_string();
            assert!(message.contains(expected), "{message}");
        };
        refused(&[], Some(2), &lists[1..], "variable list 2 does not exist");
        refused(
            &[(0, 5)],
            None,
            &lists[1..],
            "variable list 5 does not exist",
        );
        refused(
            &[(2, 0), (0, 0)],
            None,
            &lists[1..],
            "variables at 0 follow variables at 2",
        );
        refused(
            &[(1, 0)],
            None,
            &lists[1..],
            "variables at 1 are not at an instruction",
        );
        refused(
            &[(0, 0), (0, 0)],
            None,
            &lists[1..],
            "variables at 0 follow",
        );
        refused(&[], Some(0), &lists, "declaration of `count` is invalid");
        let nameless = [vec![variable("", None, 0)]];
        refused(&[], Some(0), &nameless, "has no identifier");
    }

    #[test]
    fn instruction_offsets_skip_push_immediates() {
        assert_eq!(instruction_offsets(&CODE), [0, 2, 3, 4, 5]);
        assert_eq!(instruction_offsets(&[]), Vec::<u32>::new());
    }

    #[test]
    fn a_pointer_name_that_is_not_an_identifier_is_left_out() {
        let pointer = Pointer::Storage {
            name: "größe".to_owned(),
            slot: 2,
        };
        assert_eq!(
            pointer_json(&pointer),
            json!({"location": "storage", "slot": 2})
        );
    }

    #[test]
    fn a_stack_pointer_names_a_slot_counted_from_the_top() {
        assert_eq!(
            pointer_json(&Pointer::Stack { slot: 0 }),
            json!({"location": "stack", "slot": 0})
        );
        assert_eq!(
            pointer_json(&Pointer::Stack { slot: 7 }),
            json!({"location": "stack", "slot": 7})
        );
    }

    #[test]
    fn mnemonics_follow_the_opcode_table() {
        let names: Vec<String> = [0x00, 0x20, 0x44, 0x5f, 0x60, 0x7f, 0x80, 0x9f, 0xa4, 0xfd]
            .into_iter()
            .map(mnemonic)
            .collect();
        assert_eq!(
            names,
            [
                "STOP",
                "KECCAK256",
                "PREVRANDAO",
                "PUSH0",
                "PUSH1",
                "PUSH32",
                "DUP1",
                "SWAP16",
                "LOG4",
                "REVERT"
            ]
        );
        for unassigned in [0x0c, 0x21, 0x4b, 0xa5, 0xef, 0xf6, 0xfe] {
            assert_eq!(mnemonic(unassigned), "INVALID");
        }
        let mut sorted = NAMED.to_vec();
        sorted.sort_by_key(|(code, _)| *code);
        assert_eq!(sorted, NAMED, "the table is in opcode order");
    }
}
