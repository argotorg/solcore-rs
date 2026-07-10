# Solcore standard library

This directory started as a vendored copy of the standard library from the
Y-Nak/solcore Haskell implementation. The recorded upstream reference is
`ac6f8957` (`/Users/y_nak/github.com/Y-Nak/solcore/std`). The initial vendor was
added in `1b9cde0`; its provenance note was updated in `420701f`.

The Rust compiler no longer treats these files as a byte-for-byte mirror. They
are currently maintained as a deliberately diverged Rust variant of that
snapshot. The Haskell implementation is still the language-design reference,
but its standard library is not automatically authoritative when the Rust
implementation has stricter ABI semantics, additional safety checks, or
compiler-generated overlay support.

## Divergence history

All current Rust-side divergence from the original four shared modules was
first introduced by these commits:

- `37ca038` (`Route contract dispatch through std dispatch`) added the Rust
  indirect-call spelling used by dispatch, concrete bool/bytes/bytes32 ABI
  evidence, and selector helper functions used by the then-current dispatch
  lowering.
- `2077269` (`Support word ABI dispatch in std`) added canonical `uint256` ABI
  spelling plus encode/decode evidence for the primitive `word` type.
- `258bd8f` (`Complete compiler-owned contract ABI overlays`) added product-ADT
  ABI evidence, constructor/deployment support, and bounded decoding of
  untrusted ABI input. `bc6b17a` is its compiler-side prerequisite for
  structured default-instance heads.

The copies under `crates/parser/tests/fixtures/corpus/ok/std/` are parser corpus
fixtures, not another implementation. Keep each fixture byte-identical to its
counterpart in this directory.

## Current semantic differences

### `std.solc`

- `Generic` is currently defined and exported by the canonical prelude. This
  lets compiler-generated constructor and runtime overlays obtain derived
  product-ADT ABI evidence through their private canonical `std` dependency.
- Generic product ADTs receive `ABIAttribs`, `ABIEncode`, and `ABIDecode`
  evidence through `ABITuple(rep)`. The wrapper preserves a user ADT as one ABI
  tuple component instead of flattening its representation into the enclosing
  parameter list.
- The old blanket `default instance t:ABIAttribs` is replaced by concrete
  instances. Unknown types must not silently become static 32-byte ABI values,
  and the blanket instance would also compete with Generic-derived evidence.
- `word`, `bool`, `bytes`, and `bytes32` have explicit ABI evidence where needed
  by the Rust dispatch pipeline. `word` is encoded as the canonical Solidity ABI
  type `uint256`. `bytes4` currently has only `ABIAttribs`; the compiler rejects
  it as an external ABI leaf until signature, encode, and decode support agree.
- `BoundedMemoryWordReader` limits constructor decoding to the copied creation
  argument suffix. `CalldataWordReader` also checks reads, advances, and copies
  against `calldatasize()`.
- The byte-like and `ABITuple` decode paths check dynamic offsets, sizes,
  rounding, and tuple offsets for alignment, overflow, truncation, and backward
  references into a value's own head slot. Boolean decoding accepts only
  canonical ABI values zero and one. The decoder does not yet prove that every
  dynamic tail starts after the complete enclosing tuple head. The external
  runtime and constructor paths combine these checks with bounded readers to
  guarantee range safety, but not every canonical layout constraint.

The bounds and canonical-encoding rules are language-independent correctness
and safety fixes. They should eventually be applied to the Haskell standard
library too, rather than removed from this Rust variant merely to regain textual
parity.

### `Generic.solc`

This is now a compatibility module that re-exports `Generic` from `std.solc`.
Existing source using `import std.Generic.{*}` therefore keeps the same class
identity as compiler-generated evidence.

This placement is an implementation choice, not a Rust language requirement.
It can be reverted to the Haskell module layout if prepared HIR instead injects
canonical private imports for `std.Generic` and `std.ABIGeneric`, and all class
identity checks are updated accordingly.

### `ABIGeneric.solc`

The default `ABIAttribs` and `ABIEncode` bridges moved to `std.solc`, where the
canonical product-ADT path can wrap representations in `ABITuple`. The default
Generic `ABIDecode` bridge was added there as part of the Rust product-ADT ABI
support. This module retains sum-representation instances and the explicit
`encode`/`decode` helpers.

Those helpers still encode the representation directly and can therefore have
a different wire shape from the canonical external product-ADT ABI for dynamic
products. They are representation-level SOP codecs, distinct from the
canonical external ABI, unless they are later changed to use the `ABITuple`
path.

### `dispatch.solc`

- Function values are invoked through `invokable.invoke`, matching the Rust
  frontend/specializer's indirect-call path.
- `word` and `bool` have canonical signature spellings.
- A Generic product ADT receives a parenthesized `SigString`, so a value such as
  `Point(word, word)` contributes `(uint256,uint256)` as one method parameter.
- Dynamic calldata safety is provided by the checked readers in `std.solc`.

`selector_matches_const` and `dispatch_has_selector` were introduced for an
older direct-emitter path and currently have no internal callers. The
`std.ABIGeneric` import may also be broader than the currently supported
single-constructor product-ADT surface. These are retained for now but should be
revalidated before being considered part of the stable standard-library API.

## Compiler coupling and current limits

The Rust frontend constructs compiler-owned HIR before type checking. Runtime
dispatch receives private canonical references to `std` and `std.dispatch`
without requiring source imports. A non-empty constructor is still prepared
only when the source explicitly has the canonical `import std.{*}`; its
generated references then use a private `std` alias. Constructor overlays
directly depend on `BoundedMemoryWordReader`; runtime and constructor overlays
depend on the ABI classes and derived Generic evidence documented above.

The standard-library Generic-based default ABI instances are syntactically
broad, while the compiler currently exposes canonical external ABI only for
supported, non-recursive, single-constructor product ADTs with canonical
component types. The frontend restriction is authoritative. A future
compiler-private `CanonicalABI` evidence class could encode this policy more
precisely and reduce the amount of provisional Generic evidence in the public
prelude.

Compiler-synthesized `Generic` evidence is limited to user and external-library
modules. ADTs owned by the canonical `std` library do not derive it merely
because `Generic` is exported from the prelude; internal representation wrappers
must use deliberate handwritten evidence. Besides keeping the public evidence
surface explicit, this avoids populating every compilation's trait environment
with unused derived clauses for std-internal ADTs.

## Updating this fork

Do not replace this directory wholesale from the Haskell repository without
reconciling the differences listed above. When syncing upstream changes:

1. Diff each shared module against the recorded Haskell revision.
2. Preserve or deliberately supersede every divergence listed above.
3. Prefer upstreaming language-independent ABI correctness and safety fixes to
   the Haskell implementation, then re-vendoring the shared result.
4. Record the new Haskell revision and any new Rust-only semantic difference in
   this file.
5. Refresh the parser corpus mirrors and verify that they remain byte-identical.
