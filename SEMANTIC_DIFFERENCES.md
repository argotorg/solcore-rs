# Haskell/Rust semantic compatibility and standard-library policy

This records current semantic differences, compatibility decisions, and shared
limitations. Resolved synchronization work is not a semantic difference. Test
verdict ledgers describe particular compiler modes, not the language specification.

## Comparison target

- Haskell: [`argotorg/solcore@a7b7afb0`](https://github.com/argotorg/solcore/tree/a7b7afb0638e50895008d778a167afd0e50a2c86),
  the upstream HEAD checked on 2026-10-10.
- All eight `.sol` files in [`std/`](std/) and
  [`corpus/ok/std/`](crates/parser/tests/fixtures/corpus/ok/std/) come from that
  upstream revision, with local whitespace-only formatting by `solcore-fmt`.
  Both local copies remain byte-for-byte identical to each other.
- Both compilers now use the canonical
  [source syntax](https://github.com/argotorg/solcore/blob/a7b7afb0638e50895008d778a167afd0e50a2c86/doc/src/sail/syntax.md).
  The older Haskell spellings are not a compatibility target.

The historical parser corpus is not a verdict ledger for this comparison
revision; its provenance and compiler options are documented in the
[corpus README](crates/parser/tests/fixtures/corpus/README.md).

## Shared self-recursion behavior

Both compilers allow a contract method to share a name with a top-level or
imported function. Unqualified calls inside the contract prefer its method.
This shared shadowing rule permits the silent self-call reported in
[solcore-rs #48](https://github.com/argotorg/solcore-rs/issues/48). Haskell
[PR #599](https://github.com/argotorg/solcore/pull/599) changed its duplicate
check to use qualified contract names, so latest Haskell also accepts the
original imported `setValue` example and emits a self-call. The compatibility
policy is to retain this latest-Haskell behavior in Rust without adding
`SC0225` or a new warning. For the unit-returning method with a runtime
argument in #48, both compilers emit unbounded self-recursion without a warning
or error. This shared problem remains unresolved and is tracked in
[upstream #617](https://github.com/argotorg/solcore/issues/617); matching the
reference does not mean that accidental self-recursion has been prevented.
Any future diagnostic should be considered as a separate language-policy change.
The overlap UI fixture verifies that this accepted name shadowing does not
suppress `SC0218`. General suppression after genuine item-resolution errors
is a separate behavior. In Rust, an explicit module-qualified call selects
the imported function; `contract-method-shadowing` tests both Rust call targets.

## Intentional corrections and retained differences

The linked upstream issues contain reproductions and discussion of the
reference behavior. The following Rust corrections remain intentional.

| Area | Latest reference behavior | Rust decision | Upstream issue |
| --- | --- | --- | --- |
| Unknown variant tag | `buildABIDecodeSum` checks all but the last constructor and otherwise decodes the last variant. | Check every tag and revert on an unknown value. `abi-sum-roundtrip` tests invalid tags for static and dynamic two-constructor types. | [#602](https://github.com/argotorg/solcore/issues/602) |
| Static variant head size | `buildABIAttribs` delegates to nested structural sums even though the derived encoder now emits only one tag. For three or more constructors this can count extra tag words. | Use one tag plus the maximum constructor-product head. The static 0/1/3-field variant regression requires exactly 128 bytes. | [#603](https://github.com/argotorg/solcore/issues/603) |
| Non-callable values | With specialization disabled, the frontend with legacy typeclass resolution accepts `Uncurry` and `rec`, which call a `word` value. | Reject non-callable values. | [#605](https://github.com/argotorg/solcore/issues/605) |
| Function tuple arity | Function lowering erases the distinction between one tuple argument and several arguments. Latest `ListModule`, `Pair`, and `Peano` accept calls inconsistent with their explicit function types. | Preserve source parameter arity. `function_tuple_parameter_arity` rejects `f(x, y)` when `f` requires one tuple, while accepting `f((x, y))`. | [#611](https://github.com/argotorg/solcore/issues/611) |
| Closure type names | `compose_desugared` reaches `SC0209` because source-declared `t_closure…` types are treated as generated closures by name and omitted from generated invocation arguments. Renaming the type avoids the error. | Accept the valid explicit closure representation; retain the specialization regression. | [#612](https://github.com/argotorg/solcore/issues/612) |
| Renamed type witnesses | With `import {Proxy as P} from lib`, `@word` is still annotated as unqualified `Proxy<word>`, which fails to unify with the expected `P<word>`. | Resolve the expected alias to the actual ADT and construct its visible `Proxy` constructor. `proxy_local_and_qualified` includes a renamed import. | [#613](https://github.com/argotorg/solcore/issues/613) |
| Assembly lexical scopes | Deeply nested let names can incorrectly conceal writes to outer variables, yielding stale constants. Loop initializer masking and later assembly blocks can also retain erased comptime identifiers. | Track writes in their actual scopes, substitute initializers before binding names, and preserve untouched outer constants. `asm-scope` executes both conditional-write paths. | [#614](https://github.com/argotorg/solcore/issues/614) |
| Compile-time heap invalidation | A runtime memory write can leave an older compile-time heap value available to a later comptime load. | Invalidate heap facts after residual memory effects. `ct_memory_after_runtime_asm` rejects direct, wrapped, branch, loop, and compound-expression witnesses. | [#615](https://github.com/argotorg/solcore/issues/615) |
| Effectful call arguments | Inlining an unused parameter can discard its runtime argument's side effect, including a write before a later memory read. | Keep calls whose arguments retain observable effects. `ignored-call-effects` and `ignored-builtin-effects` verify that writes, reverts, and allocations execute exactly once per call. | [#616](https://github.com/argotorg/solcore/issues/616) |
| Instance signatures | With specialization disabled, the frontend with legacy typeclass resolution accepts the narrowed `MemoryType.size` implementation in `ixa`. | Keep the instantiated class-signature check (`SC0221`). | [#606](https://github.com/argotorg/solcore/issues/606) |
| Runtime `main` | Any contract-local `main` suppresses dispatch, including a function with parameters. | Keep suppression, but require zero parameters (`SC0236`). The runtime has no argument source for this entry. | [#607](https://github.com/argotorg/solcore/issues/607) |
| ABI validation | Nullary names pass through metadata; unsupported shapes can reach a partial Haskell `error`. | Validate actual ABI evidence and use structured diagnostics. Reject same-named user-defined substitutes for canonical std wrappers and unsupported `memory<DynArray<address>>` outputs in `storage_array` and `ufcs_array`. | [#609](https://github.com/argotorg/solcore/issues/609) |
| Parameterized ADT metadata | Runtime evidence can be derived but `abiTypeOf` still handles source ADTs only as `TyCon n []`. | Keep the deliberate concrete source spelling extension, e.g. `Point<uint256>`, and require all concrete/phantom arguments to satisfy ABI evidence. | [#609](https://github.com/argotorg/solcore/issues/609) |
| Selector collisions | Distinct method signatures can share four selector bytes. Duplicate method names are already rejected by name resolution. | Reject selector collisions before emitting dispatch. | [#609](https://github.com/argotorg/solcore/issues/609) |
| Tuple return boundaries | Latest metadata preserves a single tuple return item, but the erased result passed to `abi_encode` does not preserve the corresponding outer dynamic-tuple offset. | Retain flattened output metadata consistent with runtime bytes. Correct the source boundary, wrapper, metadata, and encoding together before changing this. | [#604](https://github.com/argotorg/solcore/issues/604) |

For tuple returns, a concrete upstream witness is
`returns ((uint256, memory<bytes>))`: metadata reports one dynamic tuple, while
`std.dispatch.do_exec` encodes the pair directly. ABI consumers expecting the
advertised outer tuple would interpret the first returned integer as an
offset. Rust keeps the same payload with two output entries. Static and nested
tuple execution remains covered by `composite-values`.

## ABI support and invariants

Both compilers' right-nested product representation erases some source tuple
boundaries. Rust retains the flattened ABI convention covered by
`composite-values`; preserving those boundaries requires a coordinated
language/ABI change. This shared constraint is separate from the Haskell
return-metadata mismatch above.

An externally supported type must have compatible metadata, selector spelling,
argument decoding, and result encoding. Merely printing a type in ABI JSON is
not support. Current runtime evidence is:

| Source type | Input | Output | Notes |
| --- | --- | --- | --- |
| `uint256`, `address`, `bytes4`, `bytes32`, `bool` | yes | yes | Boolean decoding is strict. |
| `memory<string>`, `memory<bytes>`, unit, supported tuples | yes | yes | Tuple boundaries follow the convention above. |
| Primitive `word` | no | no | std still lacks `SigString`, `ABIEncode`, and `ABIDecode` instances even though metadata spells `uint256`. Missing evidence terminates with a bounded diagnostic. See [upstream #608](https://github.com/argotorg/solcore/issues/608). |
| Canonical `calldata<array<T>>` | yes | no | Lazy calldata handle; requires element input evidence. This restriction applies inside nested ADT results too. |
| Finite compiler-derived ADT | yes | yes | Requires the defining module's derivation markers and complete leaf/type-argument evidence. Multi-constructor ADTs use variant tags. |
| Recursive, excluded, or manually represented ADT | no | no | No compiler-owned finite decode/metadata plan. |

Selectors continue to use structural Generic `SigString` spellings, such as
`sum(uint256,bytes)`, while ADT ABI JSON retains source names. **Selector
spelling and constructor wire tags are different**: the variant tag is the full
32-byte keccak hash of the constructor name and its field signatures. This
does not claim general Solidity-tool support for arbitrary source ADT names.

Derived variant tags require definition-side `SigString` evidence from
`std.dispatch`; importing only `std.ABIGeneric` is insufficient in both
compilers. Rust diagnoses the missing evidence instead of falling back to
structural tags (`adt_abi_missing_sigstring`).

Explicit structural `sum` values and `std.ABIGeneric.encode`/`decode` still use
the original Generic 0/1 representation. Only the compiler-derived ADT
`ABIEncode`/`ABIDecode` path uses constructor tags. Product ADTs keep their
structural product layout.

Manual `SigString`, `ABIAttribs`, `ABIEncode`, and `ABIDecode` instances remain
usable in ordinary code and hand-written fallback dispatch. Rust rejects them
when selected by generated external ABI evidence. The check follows evidence
chosen in both consuming and defining modules, including selector and variant-tag
signatures and the actual constructor-product dependencies of derived encoders
and decoders. An output-only ADT's own unused `SigString` instance and unrelated
instances do not cause rejection.

Top-level `abi_encode` returns length-prefixed `memory<bytes>`, and generated
dispatch returns only its payload. These two std behaviors remain paired.
Derived storage remains definition-scoped and supports finite ADTs, mapping
values, and dynamic bytes/string leaves. Whole mapping-valued leaves and
recursive derived storage remain rejected.

## Resolver modes and validation scope

Haskell defaults to legacy resolution. Rust uses tabled resolution; compare
like modes before calling a discrepancy a language difference. The
`super-class-recursive-arg`, `tabled-answer-reuse`, and `tabled-mutual-chain`
cases succeed with Haskell tabled resolution and Rust.

Conversely, latest Haskell tabled resolution rejects `msg_context`,
`field-access-in-tuple`, `arrays/shadowed`, and `new-syntax/struct-storage` with
unresolved `CanStore` obligations. With specialization disabled, the frontend
with legacy typeclass resolution accepts all four, as does Rust. The solver
discrepancy is reported in [upstream #610](https://github.com/argotorg/solcore/issues/610).
Rust retains acceptance of these valid programs.

`ct_param_poly_runtime` is a phase difference: the Rust frontend preserves a
latent comptime obligation, and specialization rejects its runtime argument.
The Haskell frontend with legacy typeclass resolution first reports ambiguity.
Full compilation with tabled typeclass resolution also rejects the runtime
argument. A frontend-only pass does not mean runtime values are allowed as
comptime arguments.

Compare the same source with identical roots, std, resolver mode, and generated
dispatch settings. Parser-only examples are not necessarily valid complete
programs. Missing imports, intentionally negative declarations, or
parameterized runtime mains do not establish full-pipeline compatibility.
Frontend acceptance alone does not establish runtime correctness; use the
UI/dir fixtures and shared backend E2E tests for the affected behavior.

Use the existing workflows for verification:

```sh
for file in ABIGeneric.sol Generic.sol StorageGeneric.sol dispatch.sol \
  eip712.sol eip7951.sol opcodes.sol std.sol; do
  cmp "std/$file" "crates/parser/tests/fixtures/corpus/ok/std/$file" || exit 1
done
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-yul -p solcore-sonatina --test e2e --locked -- \
  --nocapture --test-threads=1
```

## Standard-library update policy

Preserve upstream library sources apart from local whitespace-only formatting
with `solcore-fmt`, and record their full revision. Copy and format updates in
both std locations together, keeping the local copies byte-for-byte identical.
When upstream has a confirmed bug or specification defect, document the witness
and the chosen correction;
there is no requirement to reproduce that defect or obtain an upstream fix
before preserving correct Rust behavior. Compiler-side corrections should be
kept explicit. If a library deviation beyond whitespace formatting becomes
necessary, document it per file instead of continuing to claim source alignment.
