//! Writer for the stock ethdebug format: programs, resources and the compilation.
//!
//! Plain data in, JSON out. The crate knows nothing about the compiler that
//! produced the data. Field names and required fields follow the schemas of
//! ethdebug/format at commit 16246cd, specification version 0.1.0-draft.2:
//! `program`, `program/instruction`, `program/context`, `materials/compilation`,
//! `materials/source`, `materials/source-range` and `info/resources`. Only
//! source ranges and remarks are written; the schemas reject unknown fields,
//! so nothing else is.

use std::{error, fmt};

use serde_json::{Value, json};

const VERSION: &str = "0.1.0-draft.2";

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
        "sources": sources,
    })
}

/// The resources object (`info/resources`) with empty type and pointer tables.
pub fn resources_json(compilation: &Compilation<'_>) -> Value {
    json!({
        "ethdebug": stamp("ethdebug/format/info/resources"),
        "types": {},
        "pointers": {},
        "compilation": compilation_json(compilation),
    })
}

/// A program object with one instruction per opcode of `program.code`.
///
/// Each instruction gets the context of the range that contains its offset.
/// Refused: ranges that are empty, unsorted, overlapping, leave a gap or pass
/// the end of the code; a code context or the definition naming a source that
/// is not in `sources`.
pub fn program_json(program: &Program<'_>, sources: &[Source<'_>]) -> Result<Value, Error> {
    let (def_source, def_offset, def_length) = program.definition;
    check_source(sources, def_source).map_err(|why| {
        Error::new(format!(
            "definition of `{}` is invalid: {why}",
            program.contract
        ))
    })?;
    check_ranges(program.ranges, program.code.len(), sources)?;

    let mut instructions = Vec::new();
    let (mut pc, mut range) = (0, 0);
    while pc < program.code.len() {
        while program.ranges[range].end as usize <= pc {
            range += 1;
        }
        let opcode = program.code[pc];
        let mut instruction = json!({
            "offset": pc,
            "operation": { "mnemonic": mnemonic(opcode) },
        });
        match &program.ranges[range].context {
            Context::Code {
                source,
                offset,
                length,
            } => {
                instruction["context"] = json!({ "code": source_range(*source, *offset, *length) })
            }
            Context::Remark(text) => instruction["context"] = json!({ "remark": text }),
            Context::None => {}
        }
        instructions.push(instruction);
        pc += 1 + push_data_len(opcode);
    }

    Ok(json!({
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
    }))
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
        if let Context::Code { source, .. } = context {
            check_source(sources, *source)
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

fn check_source(sources: &[Source<'_>], id: u32) -> Result<(), String> {
    if sources.iter().any(|source| source.id == id) {
        return Ok(());
    }
    Err(format!("source {id} is not in the source list"))
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
                "ethdebug": {"schema": "ethdebug/format/program", "version": "0.1.0-draft.2"},
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
    fn program_refuses_ranges_that_do_not_tile_the_code() {
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
    }

    #[test]
    fn program_refuses_a_source_that_is_not_in_the_list() {
        let message = write(&CODE, (0, 0, 1), &[range(0, 6, code(1, 0, 1))])
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("source 1 is not in the source list"),
            "{message}"
        );
        let message = write(&[], (9, 0, 1), &[]).unwrap_err().to_string();
        assert!(message.contains("definition of `C`"), "{message}");
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
            "sources": [
                {"id": 0, "path": "main.sol", "contents": "0123456789", "language": "Core Solidity"},
                {"id": 3, "path": "std/std.sol", "contents": "abc", "language": "Core Solidity"},
            ],
        });
        assert_eq!(compilation_json(&compilation), expected);
        assert_eq!(
            resources_json(&compilation),
            json!({
                "ethdebug": {"schema": "ethdebug/format/info/resources", "version": "0.1.0-draft.2"},
                "types": {},
                "pointers": {},
                "compilation": expected,
            })
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
