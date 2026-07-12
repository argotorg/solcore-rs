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

## Known issue: canonical ABI spelling and runtime dispatch disagree

The Haskell compiler's ABI JSON renderer maps the primitive `word` type to the
canonical Solidity ABI name `uint256`. Runtime selector dispatch does not make
the same conversion: the generated `Method` value retains the source-level
`word` argument and result types.

For example, this contract is currently rejected by the Haskell compiler:

```solcore
import std.{*};
import std.dispatch.{*};

contract C {
  public function run() -> word { return 42; }
}
```

Type checking the generated dispatch entry fails with:

```text
cannot entail: word : ABIEncode
```

The relevant pieces of the current snapshot are:

- `dispatch.solc` has no `word : SigString` instance;
- `std.solc` has no `word : ABIEncode` instance;
- `std.solc` has no `ABIDecoder(word, reader) : ABIDecode(word)` instance;
- the blanket `ABIAttribs` instance does provide the 32-byte static layout, so
  layout metadata alone does not make `word` externally callable.

A contract containing a source-defined `main() -> word` can still compile.
That does not exercise selector dispatch: the Haskell dispatch desugaring
suppresses its generated `RunContract.exec` entry whenever the contract already
defines `main`. The reference dispatch integration fixtures avoid the defect by
using `uint256` in their public method signatures.

### Required upstream fix

This should be fixed in the Haskell reference implementation. ABI metadata and
runtime dispatch must accept the same public surface.

For `word`, the minimal upstream std fix is to provide the missing canonical
evidence:

- `word : SigString`, producing `"uint256"`;
- `word : ABIEncode`, storing the primitive word directly;
- `ABIDecoder(word, reader) : ABIDecode(word)`, reading the primitive word
  directly.

After that change is accepted upstream, vendor it here rather than maintaining
a Rust-only patch. An alternative compiler-side fix would generate canonical
`uint256` ABI adapters around source-level `word` methods, but it is a larger
change and must be implemented consistently in ABI generation, dispatch, and
specialization.

Regression coverage upstream should include a selector-dispatched public
method with both a `word` argument and a `word` result. A source-defined
`main` is not a sufficient regression test.

## Related, but separate: user ADTs in external ABI

The current Haskell std also does not make arbitrary user ADTs externally
dispatchable. `std.ABIGeneric` provides explicit representation-level
`encode`/`decode` helpers, but `std.dispatch` has no generic `SigString`
instance for an ADT, and the Generic helpers are not a complete external ABI
contract.

Consequently, a public method such as `echo(Point) -> Point` currently fails at
`Point : SigString`, even when `Point : Generic(rep)` is available. Existing
Haskell Generic integration tests keep ADTs internal and expose canonical
primitive arguments and results.

Do not solve this by adding Rust-only Generic dispatch instances here. External
ADT layout, selector spelling, tuple boundaries, encode/decode behavior, and
ABI JSON all need one upstream design before this repository can test that
surface against the shared std.

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
