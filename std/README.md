# Solcore standard library snapshot

The `.sol` files in this directory come from the Haskell reference standard
library at revision
`a7b7afb0638e50895008d778a167afd0e50a2c86`:

```text
https://github.com/argotorg/solcore/tree/a7b7afb0638e50895008d778a167afd0e50a2c86/std
```

Upstream now uses canonical `.sol` syntax, so no syntax migration is required.
Local whitespace-only formatting is applied with `solcore-fmt`; source tokens,
literals, and comments are retained.
The copies under
`crates/parser/tests/fixtures/corpus/ok/std/` are parser fixtures and must remain
byte-for-byte identical to the files here.

This README is Rust-repository metadata; it is not part of the upstream std
snapshot.

## Synchronization policy

Keep the vendored sources aligned with the recorded upstream revision, allowing
local whitespace-only formatting with `solcore-fmt`. When upstream changes,
update and format all eight files and their parser-corpus copies together.

Do not reproduce an upstream compiler bug or problematic specification merely
for compatibility. Record confirmed upstream problems in the local `issues.md`
without staging that file, and document intentional Rust behavior in
[`SEMANTIC_DIFFERENCES.md`](../SEMANTIC_DIFFERENCES.md). Any necessary library
exception must be documented explicitly instead of silently changing the
recorded snapshot.

## Compatibility decisions

The canonical analysis of Haskell/Rust semantic differences, ABI evidence
gaps, and compatibility decisions is in
[`SEMANTIC_DIFFERENCES.md`](../SEMANTIC_DIFFERENCES.md). ABI metadata support
alone does not make a type externally dispatchable. Keep ABI JSON, selector
spelling, calldata decoding, and result encoding aligned.

## Verification

After every std update, verify at least:

```sh
for file in ABIGeneric.sol Generic.sol StorageGeneric.sol dispatch.sol \
  eip712.sol eip7951.sol opcodes.sol std.sol; do
  cmp "std/$file" "crates/parser/tests/fixtures/corpus/ok/std/$file" || exit 1
done
cargo test -p solcore-parser -p solcore-hir-ty -p solcore-specialize --locked
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-yul --test e2e --locked -- \
  --nocapture --test-threads=1
```

Imported upstream raw-vector cases retain their calldata, output, and EVM
metadata; fixtures may append local regressions. Both Yul and Sonatina compile
and execute the complete set against Osaka. See
[`tests/e2e/README.md`](../tests/e2e/README.md) for the full validation
commands.
