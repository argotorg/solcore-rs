//! Conformance of the files `--ethdebug` writes, checked on every program the
//! ethdebug tests use: each directory of `tests/ethdebug` and the inline programs
//! of the CLI tests.

use std::collections::BTreeMap;

#[path = "support/ethdebug_output.rs"]
mod ethdebug_output;
#[path = "support/ethdebug_schemas.rs"]
mod ethdebug_schemas;

use ethdebug_output::{Output, cases, compile, decode, immediate_len, opcode_name, source_ranges};
use ethdebug_schemas::PinnedSchemas;

/// What the check of one program counted.
#[derive(Default)]
struct Counts {
    programs: usize,
    instructions: usize,
    with_range: usize,
    with_remark: usize,
    with_neither: usize,
    ranges: usize,
}

/// For every program, compiled with `--ethdebug`:
///
/// 1. Every document (resources, compilation, creation program, runtime program
///    of each contract) validates against the pinned ethdebug schemas.
/// 2. The offsets of the instructions begin at 0, increase strictly, and are
///    exactly the instruction starts of an independent decode of the bytecode,
///    which ends where the code ends.
/// 3. Every mnemonic is the name of the opcode at its offset in that decode.
/// 4. An instruction's context holds a source range, or a `generated:` remark, or
///    neither, and never both, and nothing but code, remark and variables.
/// 5. Every source range of every document names a source of the compilation and
///    lies inside its text on character boundaries.
/// 6. The bytecode is the same as the one `--bin` writes without debug
///    information.
#[test]
fn every_emitted_document_conforms_to_the_format() {
    let schemas = PinnedSchemas::load();
    let mut counts = Counts::default();
    let cases = cases();
    let expected = 2 * cases.len();
    for case in cases {
        let output = compile(&case.dir, &["--ethdebug"]);
        check_documents(&case.name, &output, schemas.as_ref());
        check_programs(&case.name, &output, &mut counts);
        check_bytecode_is_unchanged(&case.name, &case.dir, &output);
    }
    println!(
        "{} programs, {} instructions: {} with a range, {} with a remark, {} with neither; {} source ranges",
        counts.programs,
        counts.instructions,
        counts.with_range,
        counts.with_remark,
        counts.with_neither,
        counts.ranges
    );
    // The corpus must exercise every kind of context, or the checks above can
    // pass on documents that hold none of them.
    assert!(
        counts.programs >= expected,
        "{} programs, expected {expected}",
        counts.programs
    );
    assert!(counts.with_range > 0 && counts.with_remark > 0 && counts.with_neither > 0);
}

/// Property 1.
fn check_documents(name: &str, output: &Output, schemas: Option<&PinnedSchemas>) {
    let Some(schemas) = schemas else { return };
    let mut documents = vec![
        (
            "ethdebug_resources.json".to_owned(),
            "schema:ethdebug/format/info/resources",
        ),
        (
            "ethdebug_compilation.json".to_owned(),
            "schema:ethdebug/format/materials/compilation",
        ),
    ];
    for contract in output.contracts() {
        for suffix in ["_ethdebug.json", "_ethdebug-runtime.json"] {
            documents.push((
                format!("{contract}{suffix}"),
                "schema:ethdebug/format/program",
            ));
        }
    }
    for (file, schema) in documents {
        schemas
            .validate(schema, &output.json(&file))
            .unwrap_or_else(|error| panic!("{name}: {file} does not match {schema}:\n{error}"));
    }
}

/// Properties 2 to 5, for the creation and runtime program of each contract.
fn check_programs(name: &str, output: &Output, counts: &mut Counts) {
    let compilation = output.compilation();
    let sources: BTreeMap<u64, &str> = compilation["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|source| {
            (
                source["id"].as_u64().unwrap(),
                source["contents"].as_str().unwrap(),
            )
        })
        .collect();
    for contract in output.contracts() {
        for (program, code) in output.programs(&contract) {
            let what = format!("{name}: {contract} {}", program["environment"]);
            let instructions = program["instructions"].as_array().unwrap();
            let decoded = decode(&code);

            // 2. Offsets.
            assert_eq!(
                instructions.len(),
                decoded.len(),
                "{what}: instruction count"
            );
            let mut previous: Option<u64> = None;
            for (instruction, expected) in instructions.iter().zip(&decoded) {
                let offset = instruction["offset"].as_u64().unwrap();
                assert!(
                    previous.is_none_or(|previous| previous < offset),
                    "{what}: offset {offset} does not follow {previous:?}"
                );
                previous = Some(offset);
                assert_eq!(offset as usize, expected.pc, "{what}: {instruction}");
                // 3. Mnemonics.
                assert_eq!(
                    instruction["operation"]["mnemonic"],
                    opcode_name(expected.opcode),
                    "{what}: {instruction}"
                );
                assert_eq!(
                    expected.immediate.len(),
                    immediate_len(expected.opcode),
                    "{what}: the immediate of the opcode at {offset} is cut off"
                );

                // 4. Contexts.
                let context = instruction["context"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                for key in context.keys() {
                    assert!(
                        ["code", "remark", "variables"].contains(&key.as_str()),
                        "{what}: unexpected `{key}` in {instruction}"
                    );
                }
                let (code, remark) = (context.get("code"), context.get("remark"));
                assert!(
                    code.is_none() || remark.is_none(),
                    "{what}: a range and a remark: {instruction}"
                );
                if let Some(remark) = remark {
                    let remark = remark.as_str().unwrap();
                    assert!(
                        remark
                            .strip_prefix("generated: ")
                            .is_some_and(|reason| !reason.is_empty()),
                        "{what}: remark `{remark}`"
                    );
                }
                counts.instructions += 1;
                match (code, remark) {
                    (Some(_), _) => counts.with_range += 1,
                    (_, Some(_)) => counts.with_remark += 1,
                    _ => counts.with_neither += 1,
                }
            }
            let end = decoded.last().map_or(0, |last| last.end());
            assert_eq!(end, code.len(), "{what}: the instructions end at {end}");

            // 5. Source ranges.
            let mut ranges = Vec::new();
            source_ranges(&program, &mut ranges);
            for range in ranges {
                let contents = sources
                    .get(&range.source)
                    .unwrap_or_else(|| panic!("{what}: source {} is not listed", range.source));
                let end = range.offset + range.length;
                assert!(
                    end as usize <= contents.len(),
                    "{what}: range {}..{end} passes the end of source {} ({} bytes)",
                    range.offset,
                    range.source,
                    contents.len()
                );
                assert!(
                    contents.is_char_boundary(range.offset as usize)
                        && contents.is_char_boundary(end as usize),
                    "{what}: range {}..{end} of source {} splits a character",
                    range.offset,
                    range.source
                );
                counts.ranges += 1;
            }
            counts.programs += 1;
        }
    }
}

/// Property 6: `--ethdebug` writes the same `.bin` and `.bin-runtime` files as
/// `--bin`.
fn check_bytecode_is_unchanged(name: &str, dir: &std::path::Path, with_debug: &Output) {
    let plain = compile(dir, &["--bin"]);
    let bytecode = |output: &Output| -> Vec<String> {
        output
            .files()
            .into_iter()
            .filter(|file| file.ends_with(".bin") || file.ends_with(".bin-runtime"))
            .collect()
    };
    let files = bytecode(&plain);
    assert!(!files.is_empty(), "{name}: --bin wrote no bytecode");
    assert_eq!(files, bytecode(with_debug), "{name}: the files differ");
    for file in files {
        assert_eq!(
            plain.bytes(&file),
            with_debug.bytes(&file),
            "{name}: {file} differs with --ethdebug"
        );
    }
}
