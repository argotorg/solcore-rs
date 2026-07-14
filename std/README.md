# Solcore standard library snapshot

The `.solc` files in this directory are vendored from the Haskell reference
implementation at revision `ac6f8957`:

```text
https://github.com/argotorg/solcore/tree/ac6f8957/std
```

They are kept byte-for-byte identical to that reference snapshot. The copies
under `crates/parser/tests/fixtures/corpus/ok/std/` are parser fixtures and must
also remain byte-for-byte identical to the files here.

This README is Rust-repository metadata; it is not part of the upstream std
snapshot.

## Synchronization policy

Do not apply Rust-only semantic fixes directly to these `.solc` files.

When a shared standard-library defect is found:

1. reproduce it with the Haskell compiler and the upstream std;
2. fix it in the Haskell reference implementation first;
3. record the new upstream revision;
4. re-vendor the complete upstream std change here and in the parser corpus;
5. verify both backends against the updated snapshot.

Compiler-side compatibility code may differ between the Haskell and Rust
implementations, but the vendored std source should not.

## Compatibility decisions

The canonical analysis of Haskell/Rust semantic differences, ABI evidence
gaps, and the recommended owner for each fix is in
[`SEMANTIC_DIFFERENCES.md`](../SEMANTIC_DIFFERENCES.md). ABI metadata support
alone does not make a type externally dispatchable. Keep ABI JSON, selector
spelling, calldata decoding, and result encoding aligned.

## Verification

After every std update, verify at least:

```sh
for file in ABIGeneric.solc Generic.solc dispatch.solc opcodes.solc std.solc; do
  cmp "std/$file" "crates/parser/tests/fixtures/corpus/ok/std/$file" || exit 1
done
cargo test -p solcore-parser -p solcore-hir-ty -p solcore-specialize --locked
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-yul -p solcore-sonatina --test e2e --locked -- \
  --nocapture --test-threads=1
```
