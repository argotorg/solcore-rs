# Haskell/Rust semantic compatibility and standard-library policy

This document is the canonical record of known semantic differences between
the Haskell and Rust Solcore implementations. It records which behavior should
win and where a fix belongs. The parity TSV files are executable test ledgers;
they are not the language specification.

## Comparison baseline

- Haskell reference: [`argotorg/solcore@ac6f8957`](https://github.com/argotorg/solcore/tree/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f).
- Rust implementation: `solcore-rs@631d40814b28755bfd0afb6fa97a7a79895fa6ee`.
- Standard library: the byte-identical `ac6f8957` snapshot in [`std/`](std/).
- Validation date: 2026-07-14.

The reference corpus in
[`reference-frontend.tsv`](crates/parser/tests/fixtures/corpus/reference-frontend.tsv)
was produced with Haskell flags `-n -g`: specialization/Hull emission and
generated contract dispatch were disabled. It used the default legacy
type-class resolver. Its 278 passes, 153 failures, and two timeouts describe
that configuration, not the whole Haskell compiler.

The Rust accepted-corpus gate in
[`frontend_smoke.rs`](crates/hir-ty/tests/frontend_smoke.rs) runs the full Rust
frontend, including generated dispatch. Keep these categories separate:

1. a genuine language/type-system difference;
2. a solver-mode difference (`legacy` versus `tabled`);
3. a phase difference (`-g` versus generated dispatch);
4. a shared-std defect; and
5. an implementation defect after both sides run in the same mode.

Written syntax and safety invariants take priority over accidentally accepted
legacy fixtures. For an external ABI, a type is supported only when ABI
metadata, selector spelling, argument decoding, and result encoding agree.

## Decision summary

“Owner” identifies the implementation that should change. “Harness” means that
the compiler behaviors already agree once the same options are used.

| Area / witness | Observed behavior | Recommendation and owner |
| --- | --- | --- |
| `for` post-clause `let` ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/cases/for-let-post.solc)) | Rust accepts it; the Haskell parser accepts only assignments in the post clause. | A post clause has the same forms as an init clause, as the Haskell language documentation says. **Fix Haskell parser and its negative fixture.** |
| Pair-domain calls (`ListModule`, `Pair`, `Peano`) | Haskell accepts `f(x, y)` where `f : (a, b) -> c`; Rust models the parsed pair domain as one source argument and requires `f((x, y))`. | Preserve the Haskell source-arity convention: `(a, b) -> c` accepts two source arguments, while an explicitly tuple-valued domain is written with another grouping. **Fix Rust call/type lowering.** |
| Calling a `word` (`Uncurry`, `rec`) | Haskell accepts invocation of a value annotated as `word`; Rust reports a non-callable value. | Only function/invokable values are callable. **Fix Haskell type checking; keep Rust.** |
| Phantom result ambiguity ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/cases/match-compiler-undef-asm.solc)) | `Foo(42)` leaves `Foo(a)`'s phantom `a` unconstrained. Haskell rejects it; Rust accepts it. | Require an annotation or another constraint that determines `a`. **Fix Rust inference/ambiguity checking.** |
| Method-local `forall` ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/cases/strange-unbound.solc)) | Haskell retains the binder; Rust reports `a` as undefined. | The binder is in scope in the member signature. **Fix Rust name/type lowering.** |
| Comptime integer-literal patterns ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/comptime/integer-lit-pat.solc)) | Haskell matches literals against `comptime word`/`comptime integer`; Rust compares against the staging wrapper and rejects them. | Pattern compatibility should inspect the underlying numeric type. **Fix Rust pattern typing.** |
| Explicit closure desugaring ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/cases/compose_desugared.solc)) | Haskell says the generated-style `invoke` implementation is not polymorphic enough; Rust accepts it. | Accept the explicit representation if it is valid closure-conversion output. **Fix Haskell rank-polymorphic checking**, while retaining a Rust specialization regression. |
| Narrowed instance member ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/cases/ixa.solc)) | Haskell accepts `size : Proxy(memory(a)) -> word` where the instantiated class requires `Proxy(memory(array(a)))`; Rust rejects it. | An instance member must implement the instantiated class signature. **Fix Haskell instance checking; keep Rust.** |
| Multi-parameter class improvement ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/cases/bar.solc)) | Haskell infers the chained `Foo`/`Bar` result parameter; Rust leaves it ambiguous. | Apply the entailed result improvement. **Fix the Rust solver.** |
| Recursive/table-reuse fixtures | Haskell legacy rejects `super-class-recursive-arg`, `tabled-answer-reuse`, and `tabled-mutual-chain`; Haskell tabled mode and Rust accept them. | These are not semantic differences under the tabled resolver. Make tabled canonical, or record the mode in each verdict. **Fix Haskell configuration and the harness.** |
| Polymorphic comptime argument ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/comptime/ct_param_poly_runtime.solc)) | Haskell legacy first reports ambiguity; Haskell tabled full-pipeline mode rejects a runtime value passed to a comptime parameter. Rust accepts it. | Preserve the latent comptime obligation through specialization. **Fix Rust comptime checking.** |
| Parameterized contract `main` ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/cases/multi-stmt-var-leaf.solc)) | Haskell suppresses generated dispatch whenever a local `main` exists and accepts parameters; Rust rejects them because the runtime entry receives no arguments. | A source runtime entry must be zero-argument. **Fix Haskell dispatch validation; keep Rust.** |
| Missing helper imports (`field-helper-cxt-collision`, `pair-bug`) | Haskell `-g` verdicts pass; both full frontends fail because the fixtures omit `std.dispatch`. | This is a mode mismatch. Compare both with dispatch or both without it. **Fix the harness/fixtures.** |
| Three-argument constructor (`dispatch/miniERC20`) | Haskell full dispatch accepts it; Rust deployment lowering mismatches a nested product with a flat three-field destructure. | Preserve constructor arity through product construction/destructuring. **Fix Rust deployment lowering.** |
| Large dispatch (`dispatch/weth9`) | Haskell dispatch accepts it; Rust cannot close generated `RunContract` evidence. | The canonical surface should solve. **Fix the Rust solver/generated wrapper.** |
| Primitive `word` in public ABI | Both metadata emitters call it `uint256`, but shared std cannot dispatch source `word`. Haskell reports missing evidence; a local Rust full-pipeline probe did not terminate within eight minutes. | Add complete `word` evidence in the **upstream Haskell std**, then re-vendor. Rust must terminate with success or a bounded diagnostic while evidence is missing. |
| User ADTs in public ABI | Haskell metadata passes through a nullary source name or crashes on a parameterized ADT; runtime `SigString` is absent. Rust metadata lowers some auto-derived product ADTs to tuples. | Immediately reject incomplete ADT surfaces in Rust; design layout/spelling/codec semantics in the **upstream language and std**, then implement both sides. |
| ABI type validation | Haskell passes other nullary names through and uses `error` for unsupported shapes. Rust uses canonical checks and diagnostics. | Validate against the dispatchable ABI surface. **Fix the Haskell ABI emitter; keep Rust's diagnostic model.** |
| Signature/selector collisions | Rust rejects duplicate signatures and distinct signatures with the same four-byte selector. Haskell has no equivalent preflight. | Reject both before code generation. **Fix Haskell dispatch generation.** |
| Nested tuple boundary | Both flatten the language's right-nested pair representation at the top ABI boundary. | This is shared. **Fix both compilers and the language ABI design together** if nested boundaries must be preserved. |
| Yul `keccak256` (`uintdesugared`, `std/opcodes.solc`, `std/std.solc`) | Haskell's builtin table includes it. Rust lowering supports it, but Rust frontend typing omits its signature. | Expose the existing builtin in the frontend. **Fix Rust; do not edit std.** |

## Evidence and rationale

### Syntax and ordinary type checking

Haskell
[`forPostP`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Frontend/Parser/Stmt.hs#L113-L120)
uses only `forAssignP`, while its init parser also uses `forLetP`. The same
revision's [syntax documentation](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/doc/src/sail/syntax.md#L333-L339)
says the post clause follows the init grammar. Rust's
[`parsed_stmt_parser`](crates/parser/src/parse/stmt.rs) uses one `for_item` for
both positions. Haskell is the outlier.

Haskell
[`parenTypeP`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Frontend/Parser/SolcoreTypes.hs#L45-L65)
folds comma-separated types into the language's right-nested pair form and
removes singleton grouping. The intended source-level convention is explicit
in `cases/Pair.solc`: `f : (word, word) -> word` is invoked as `f(a, b)`, while
an explicitly tuple-valued domain is written `f : ((word, word)) -> word` and
invoked as `f((x, y))`. Rust's
[`type_parser`](crates/parser/src/parse/types.rs) instead stores every parsed
tuple domain as one `params` element before source-call arity checking. Rust
must preserve the Haskell comma-domain convention rather than rejecting the
reference calls. The separate acceptance of calls through a `word` annotation
remains a Haskell type-checking defect.

For phantom `Foo(a)`, the constructor payload contains only `word`, so
`Foo(42)` supplies no information for `a`. Haskell reports ambiguity; Rust lets
the unresolved result pass. This is not definite-assignment or assembly
analysis. The `ixa` case is another Haskell false acceptance: substitution of
the instance head into the class signature does not equal the implementation
signature. Rust's `SC0221` should remain.

### Resolver and comptime modes

Haskell defaults to `LegacyResolution` in
[`Options.hs`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Pipeline/Options.hs#L55-L72),
while its tabled tests select `TabledResolution` in
[`test/Cases.hs`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/test/Cases.hs#L628-L657).
Direct runs confirm that the three recursive/reuse fixtures pass in tabled
mode. Their legacy failures must not be described as Rust solver extensions.

`ct_param_poly_runtime.solc` is a real difference. Haskell tabled full-pipeline
mode reports a runtime value passed to `Wrap.unwrap`'s comptime parameter; Rust
accepts it. Haskell intentionally defers polymorphic cases in
[`Frontend/ComptimeCheck.hs`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Frontend/ComptimeCheck.hs#L196-L215).
Rust already has latent-call analysis in
[`infer/comptime.rs`](crates/hir-ty/src/infer/comptime.rs) and specialization
checks in [`evaluate/core.rs`](crates/specialize/src/evaluate/core.rs); this
path loses or prematurely discharges the obligation.

### Contract lowering and parity configuration

Haskell
[`contractDispatchTopDecls`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Desugarer/ContractDispatch.hs#L36-L43)
suppresses generated runtime dispatch for any contract-local `main`, without an
arity check. Rust mirrors suppression but adds
[`contract_runtime_main_diagnostics`](crates/hir-ty/src/contract/dispatch.rs),
because the runtime convention invokes it with no arguments. Haskell should add
the same validation.

Ordinary Haskell corpus tests and the verdict generator disable dispatch. Rust
`reference_accepted_corpus_passes_the_full_frontend` enables it. Thus
`field-helper-cxt-collision` and `pair-bug` pass only the Haskell no-dispatch
run; a Haskell full run rejects the same missing `std.dispatch` names. They do
not demonstrate implicit Haskell bindings.

[`rust-rejected-reference-passes.tsv`](crates/parser/tests/fixtures/corpus/rust-rejected-reference-passes.tsv)
has 86 diagnostic rows across 56 paths. Thirty-nine paths are intentional Rust
negative `imports/*` fixtures; only 17 unique non-import paths are reference
compatibility cases. Manifest row counts are not semantic-difference counts.

For actual full-pipeline differences, Haskell accepts `miniERC20` and `weth9`.
Rust's deployment function is built by
[`generated_deployment_main`](crates/hir-ty/src/prepare.rs); MiniERC20 exposes a
product/destructure mismatch. WETH9 reaches generated `RunContract` evidence
and exposes a Rust solver/wrapper limitation.

### ABI metadata, selectors, and runtime evidence

Haskell
[`abiTypeOf`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Desugarer/ContractDispatch.hs#L385-L398)
and Rust [`abi_type_of`](crates/hir-ty/src/contract/abi.rs) render primitive
`word` as `uint256`. Runtime dispatch is separate: generated `Method` values
retain source types and require classes from `std.dispatch` and `std`.

The current shared snapshot has this evidence matrix:

| Source ABI type | Selector spelling (inputs) | Decode input | Encode result | Status |
| --- | --- | --- | --- | --- |
| `uint256` | yes | yes | yes | complete |
| `address` | yes | yes | yes | complete |
| `bytes32` | yes | yes | yes | complete |
| `memory(string)` | yes | yes | yes | complete |
| `memory(bytes)` | yes | yes | yes | complete |
| `()` | yes | yes | yes | complete |
| `bool` | **no** | **no** | yes | output-only |
| `word` (ABI `uint256`) | **no** | **no** | **no** | unsupported by dispatch |
| pair/tuple | recursive | recursive | recursive | complete only when all components are complete |
| user ADT | **no generic instance** | representation helpers only | representation helpers only | no complete external contract |

For `word`, the minimum upstream std correction is:

- `word : SigString`, returning `"uint256"`;
- `word : ABIEncode`, storing the primitive word directly; and
- `ABIDecoder(word, reader) : ABIDecode(word)`, reading it directly.

The same audit should add input-side `bool` evidence if boolean parameters are
intended to be public. A result-only bool works because selectors omit result
types and `bool : ABIEncode` exists; that does not make bool a supported input.

Rust's structural product support is in
[`user_adt_product_fields`](crates/hir-ty/src/contract/abi.rs). Shared
`std/ABIGeneric.solc` provides representation helpers, but
`std/dispatch.solc` has no generic canonical `SigString`. Haskell also emits an
arbitrary nullary user type name as if it were a Solidity ABI name and reaches
a partial `error` for parameterized user types. Until a stable external ADT
design exists, Rust should reject this surface rather than emit metadata that
the shared std cannot dispatch.

Both emitters flatten right-nested pairs. Haskell does so in `flattenTuple` and
Rust in [`flatten_tuple`](crates/hir-ty/src/contract/abi.rs); observable behavior
is recorded in [`tests/e2e/README.md`](tests/e2e/README.md). The source
representation has already erased some nested boundaries, so std alone cannot
fix this.

Rust checks duplicate signatures and four-byte selector collisions in
[`contract/dispatch.rs`](crates/hir-ty/src/contract/dispatch.rs). Haskell should
perform the same preflight, replace partial ABI-renderer errors with structured
diagnostics, and replace arbitrary nullary-name passthrough with a canonical
allowlist or evidence-based query.

### Yul builtins

Haskell
[`yulPrimOps`](https://github.com/argotorg/solcore/blob/ac6f8957a78dc53248dbe053f1ddbc2a2201b81f/src/Solcore/Primitives/Primitives.hs#L252-L282)
declares `keccak256 : word -> word -> word`. Rust Sonatina lowering and Yul
name translation recognize it, but
[`yul_builtin_sig`](crates/hir-ty/src/infer/yul.rs) omits it. The `SC0211`
diagnostics on vendored std are a Rust frontend defect, not a std defect.

## Standard-library recommendation

The `.solc` files in [`std/`](std/) are a shared compatibility artifact, not a
Rust fork. Do not apply Rust-only semantic edits. A shared-std fix must:

1. reproduce with the pinned Haskell compiler and upstream std;
2. be fixed and tested in upstream Haskell std first;
3. pin the new upstream revision;
4. be re-vendored byte-for-byte into `std/` and
   `crates/parser/tests/fixtures/corpus/ok/std/`; and
5. pass full dispatch and backend tests on both implementations.

The required invariant is:

> A public source type is supported if and only if ABI JSON can represent it,
> its canonical input signature can be hashed, calldata can be decoded into it,
> and a result can be encoded from it.

The next upstream std change should complete `word` and input-side `bool`, then
add argument/result matrix tests for `word`, `uint256`, `address`, `bytes32`,
`bool`, `memory(string)`, `memory(bytes)`, and supported tuples. Unsupported
location wrappers, std leaf types, and user ADTs must be explicitly rejected
until specified. Each test must use generated selector dispatch and must not
define source `main`, because source `main` suppresses the path under test.

Compiler fixes such as Rust's missing `keccak256` signature, Haskell ABI
diagnostics, and collision checks do not belong in std. Keep
`import std.dispatch.{*};` explicit until both compilers have a specified
compiler-private dependency mechanism.

## Keeping the parity ledger honest

- Record Haskell solver mode and enabled phases with every generated verdict.
- Compare the same pipeline on both sides: full dispatch for contract fixtures,
  and no dispatch for isolated frontend fixtures.
- Record the actual diagnostic cause. `match-compiler-undef-asm` is phantom-type
  ambiguity, not definite assignment.
- Exclude intentional Rust-only negative import fixtures from reference-pass
  difference counts.
- When resolving a divergence, remove its TSV allowance and add the smallest
  regression on the corrected side.

After a std update, verify the copies and then the full pipelines:

```sh
for file in ABIGeneric.solc Generic.solc dispatch.solc opcodes.solc std.solc; do
  cmp "std/$file" "crates/parser/tests/fixtures/corpus/ok/std/$file" || exit 1
done
cargo test -p solcore-parser -p solcore-hir-ty -p solcore-specialize --locked
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-yul -p solcore-sonatina --test e2e --locked -- \
  --nocapture --test-threads=1
```
