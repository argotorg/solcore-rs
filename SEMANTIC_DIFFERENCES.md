# Haskell/Rust semantic compatibility and standard-library policy

This document is the canonical record of known semantic differences between
the Haskell and Rust Solcore implementations. It records which behavior should
win and where a fix belongs. The parity TSV files are executable test ledgers;
they are not the language specification.

The canonical Core source surface is specified separately in
[`syntax.md`](syntax.md) and intentionally does not preserve legacy Haskell
spellings. The pinned Haskell compiler remains a comparison baseline for
already-supported semantics and the shared standard library, not a
source-syntax compatibility target. Consequently, legacy `.sol` corpus
acceptance records migration coverage but does not make a spelling canonical.

## Comparison baseline

- Haskell reference: [`argotorg/solcore@2f372bde`](https://github.com/argotorg/solcore/tree/2f372bde2801612814015a22319d0bc51486cbf0).
- Rust baseline: `8536dc3dc673aee71cbc8f25d3e49208b9a614c2`; this change set
  synchronizes it with that reference.
- Standard library target: the syntax-migrated semantic snapshot of `2f372bde`
  in [`std/`](std/).
- Validation date: 2026-08-11.

The complete 499-source reference corpus in
[`reference-frontend.tsv`](crates/parser/tests/fixtures/corpus/reference-frontend.tsv)
records the exact target snapshot with Haskell flags `-n -g`:
specialization/Hull emission and generated contract dispatch were disabled. It
used the default legacy type-class resolver. Its 337 passes, 160 failures, and
two timeouts describe that configuration, not the whole Haskell compiler. The
[corpus README](crates/parser/tests/fixtures/corpus/README.md) records the exact
command, timeout policy, and diagnostic-code sentinel.

The Rust accepted-corpus gate in
[`frontend_smoke.rs`](crates/hir-ty/tests/frontend_smoke.rs) runs the full Rust
frontend, including generated dispatch. Keep these categories separate:

1. a genuine language/type-system difference;
2. a solver-mode difference (`legacy` versus `tabled`);
3. a phase difference (`-g` versus generated dispatch);
4. a shared-std defect; and
5. an implementation defect after both sides run in the same mode.

The canonical syntax specification and safety invariants take priority over
accidentally accepted legacy fixtures. For an external ABI, a type is supported
only when ABI metadata, selector spelling, argument decoding, and result
encoding agree.

## Decision summary

“Owner” identifies the implementation that should change. “Harness” means that
the compiler behaviors already agree once the same options are used.

| Area / witness | Observed behavior | Recommendation and owner |
| --- | --- | --- |
| `for` post-clause `let` ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/cases/for-let-post.sol)) | Rust accepts it; the Haskell parser accepts only assignments in the post clause. | A post clause has the same forms as an init clause, as the Haskell language documentation says. **Fix Haskell parser and its negative fixture.** |
| Calling a `word` (`Uncurry`, `rec`) | Haskell accepts invocation of a value annotated as `word`; Rust reports a non-callable value. | Only function/invokable values are callable. **Fix Haskell type checking; keep Rust.** |
| Explicit closure desugaring ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/cases/compose_desugared.sol)) | Haskell says the generated-style `invoke` implementation is not polymorphic enough; Rust accepts it. | Accept the explicit representation if it is valid closure-conversion output. **Fix Haskell rank-polymorphic checking**, while retaining a Rust specialization regression. |
| Narrowed instance member ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/cases/ixa.sol)) | Haskell accepts `size : Proxy(memory(a)) -> word` where the instantiated class requires `Proxy(memory(array(a)))`; Rust rejects it. | An instance member must implement the instantiated class signature. **Fix Haskell instance checking; keep Rust.** |
| Recursive/table-reuse fixtures | Haskell legacy rejects `super-class-recursive-arg`, `tabled-answer-reuse`, and `tabled-mutual-chain`; Haskell tabled mode and Rust accept them. | These are not semantic differences under the tabled resolver. Make tabled canonical, or record the mode in each verdict. **Fix Haskell configuration and the harness.** |
| Polymorphic comptime argument ([fixture](crates/parser/tests/fixtures/corpus/fail/test/examples/comptime/ct_param_poly_runtime.sol)) | The Haskell legacy frontend first reports ambiguity. Both Haskell tabled full-pipeline mode and Rust specialization reject a runtime value passed to a comptime parameter; the Rust frontend-only parity probe intentionally defers it. | The latent comptime obligation is already preserved through Rust specialization. **Keep the specialization regression and record the phase in the harness.** |
| Parameterized contract `main` ([fixture](crates/parser/tests/fixtures/corpus/ok/test/examples/cases/multi-stmt-var-leaf.sol)) | Haskell suppresses generated dispatch whenever a local `main` exists and accepts parameters; Rust rejects them because the runtime entry receives no arguments. | A source runtime entry must be zero-argument. **Fix Haskell dispatch validation; keep Rust.** |
| Missing helper imports (`contract-local-derive`, `contract-local-type-same-name`, `field-helper-cxt-collision`, `pair-bug`, `ufcs-no-conflict`) | Haskell `-g` verdicts pass; both full frontends fail because the fixtures omit `std.dispatch`. | This is a mode mismatch. Compare both with dispatch or both without it. **Fix the harness/fixtures.** |
| Primitive `word` in public ABI | Both metadata emitters call it `uint256`, but shared std cannot dispatch source `word`. Both compilers report missing evidence; current Rust tabled resolution terminates with a bounded `SC0207`. | Add complete `word` evidence in the **upstream Haskell std**, then re-vendor. Keep a Rust regression proving bounded failure while evidence is missing. |
| User ADTs in public ABI | At `2f372bde`, upstream supports non-recursive, compiler-derived `Generic` ADTs both directly and under its legacy `calldata(array(T))` spelling; runtime selectors spell the Generic representation structurally. Its `ContractDispatch.abiTypeOf` emits source metadata only for a nullary `TyCon n []`, so a concrete parameterized ADT can derive runtime evidence but still fails upstream ABI JSON emission. Rust mirrors the nullary surface and intentionally extends metadata to source spellings such as `Point<uint256>`. Recursive, excluded, and manually represented ADTs lack the derived decode path. | Keep the safe Rust parameterized-metadata extension, but do not describe it as exact target-emitter parity. Keep non-derived forms rejected with structured diagnostics, and never infer ABI meaning from a same-named user `array`/`calldata` type. |
| Derived ADTs in storage | `2f372bde` and Rust derive per-type `StorageSize` and `storage<T>: CanStore<T>` for eligible, non-recursive compiler-owned `Generic` ADTs when `std.StorageGeneric` is visible in the definition module. Direct fields, ADTs as mapping values, and `memory<bytes>`/`memory<string>` leaves are supported. Recursive ADTs and an ADT field whose leaf is a whole mapping remain unstorable. | This is parity. Keep derivation compiler-owned and definition-scoped, and reject unsupported leaves or recursion at the storage use site with structured diagnostics. The no-dispatch ledger's recursive-ADT pass is a phase difference, not a semantic acceptance. |
| ABI type validation | Haskell passes other nullary names through and uses `error` for unsupported shapes. Rust uses canonical checks and diagnostics, including rejection of `memory<DynArray<address>>` outputs in `storage_array` and `ufcs_array`. | Validate against the dispatchable ABI surface. **Fix the Haskell ABI emitter; keep Rust's diagnostic model.** |
| Signature/selector collisions | Rust rejects duplicate signatures and distinct signatures with the same four-byte selector. Haskell has no equivalent preflight. | Reject both before code generation. **Fix Haskell dispatch generation.** |
| Nested tuple boundary | Both flatten the language's right-nested pair representation at the top ABI boundary. | This is shared. **Fix both compilers and the language ABI design together** if nested boundaries must be preserved. |
| Top-level `abi_encode` result | `2f372bde` returns a valid length-prefixed `memory(bytes)` and dispatch returns only its payload. Rust vendors the same paired `std.sol`/`dispatch.sol` change. | This is parity. Keep the two std changes atomic and pin direct static/dynamic/ADT encodings in both backends. |
| Textual Yul identifiers and template meta expressions | Both implementations accept standard Yul names beginning with `_`/`$` and containing `$`. Haskell's shared parser also exposes its backtick/`${...}` Template Haskell antiquotes to legacy `.sol` and `.hull` source, then prints their payload as raw Yul; Rust recognizes those forms only to issue a targeted source diagnostic. | **Keep identifier parity, but keep unresolved antiquotes out of source Yul.** Split Haskell's ordinary and quasiquote parsers; retain Rust's negative regressions so internal template syntax cannot bypass validation or reach a backend. |

## Evidence and rationale

### Syntax and ordinary type checking

Haskell
[`forPostP`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Solcore/Frontend/Parser/Stmt.hs#L116-L123)
uses only `forAssignP`, while its init parser also uses `forLetP`. The same
revision's [syntax documentation](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/doc/src/sail/syntax.md#L333-L339)
says the post clause follows the init grammar. Rust's
[`parsed_stmt_parser`](crates/parser/src/parse/stmt.rs) uses one `for_item` for
both positions. Haskell is the outlier.

Haskell's acceptance of calls through a `word` annotation remains a
type-checking defect. The `ixa` case is another Haskell false acceptance:
substitution of the instance head into the class signature does not equal the
implementation signature. Rust's `SC0221` should remain.

The target Yul parser now backtracks keyword matches, so names such as
`format`, `letish`, and `continueish` remain identifiers. Rust already has that
behavior because its lexer chooses the complete identifier token before the
Yul parser matches token variants; dedicated lexer and statement regressions
pin the parity. Rust also accepts standard Yul-only identifiers beginning with
`_`/`$` or containing `$` in every Yul name position without widening ordinary
Solcore identifiers. The executable
[`yul-special-identifiers`](tests/e2e/yul-special-identifiers/main.sol)
regression carries those names through both backends.

The two superficially similar Haskell meta forms have a different status.
Commit
[`8223e09d`](https://github.com/argotorg/solcore/commit/8223e09d007644ee55b5e63c8d6eaea271e6ad46)
introduced `YMeta` as a "Yul antiquoter" for
[`Language.Yul.QuasiQuote`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Language/Yul/QuasiQuote.hs#L55-L60).
The ordinary source frontend and the quasiquoter both call the same `yulBlock`
parser, so backtick and `${...}` expressions currently leak into legacy `.sol` and
`.hull`; the pretty-printer removes their delimiters and emits the contents as
raw Yul. Neither the target's
[`YulExpr` grammar](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/doc/railroad/sail.bnf#L308-L311)
nor standard Yul defines that source syntax. Rust therefore tokenizes a
complete antiquote only to report that it is internal template syntax and
produces an error expression; no unresolved meta payload enters HIR or either
backend. The upstream correction is to reserve `YMeta` for the quasiquote
parser rather than preserving the shared-parser leak as a language feature.

### Resolver and comptime modes

Haskell defaults to `LegacyResolution` in
[`Options.hs`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Solcore/Pipeline/Options.hs#L55-L72),
while its tabled tests select `TabledResolution` in
[`test/Cases.hs`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/test/Cases.hs#L644-L688).
Direct runs confirm that the three recursive/reuse fixtures pass in tabled
mode. Their legacy failures must not be described as Rust solver extensions.

`ct_param_poly_runtime.sol` is a phase-sensitive case rather than a remaining
semantic difference. Haskell tabled full-pipeline mode and Rust specialization
both report a runtime value passed to `Wrap.unwrap`'s comptime parameter, while
the Rust frontend-only corpus probe has not reached that phase. Haskell also
intentionally defers polymorphic cases in
[`Frontend/ComptimeCheck.hs`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Solcore/Frontend/ComptimeCheck.hs#L196-L215).
Rust already has latent-call analysis in
[`infer/comptime.rs`](crates/hir-ty/src/infer/comptime.rs) and specialization
checks in [`evaluate/core.rs`](crates/specialize/src/evaluate/core.rs); the
obligation survives into that check and produces `SC0409`.

### Contract lowering and parity configuration

Haskell
[`contractDispatchTopDecls`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Solcore/Desugarer/ContractDispatch.hs#L36-L43)
suppresses generated runtime dispatch for any contract-local `main`, without an
arity check. Rust mirrors suppression but adds
[`contract_runtime_main_diagnostics`](crates/hir-ty/src/contract/dispatch.rs),
because the runtime convention invokes it with no arguments. Haskell should add
the same validation.

Ordinary Haskell corpus tests and the verdict generator disable dispatch. Rust
`reference_accepted_corpus_passes_the_full_frontend` enables it. Thus
`contract-local-derive`, `contract-local-type-same-name`,
`field-helper-cxt-collision`, `pair-bug`, and `ufcs-no-conflict` pass only the
Haskell no-dispatch run; a Haskell full run rejects the same missing
`std.dispatch` names. They do not demonstrate implicit Haskell bindings.

The current
[`rust-rejected-reference-passes.tsv`](crates/parser/tests/fixtures/corpus/rust-rejected-reference-passes.tsv)
has 91 diagnostic rows across 51 paths. Thirty-nine paths are intentional Rust
negative `imports/*` fixtures; only 12 unique non-import paths remain as
reference compatibility cases. Manifest row counts are not
semantic-difference counts.

### Derived ADT storage

Upstream `2f372bde`'s
[`DeriveGeneric`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Solcore/Desugarer/DeriveGeneric.hs#L31-L47)
uses the `StorageDeriving` marker exported by
[`std.StorageGeneric`](std/StorageGeneric.sol) to emit concrete
`StorageSize` and `storage<T>: CanStore<T>` instances beside each eligible
compiler-derived `Generic` instance. The structural std implementation stores
sum, product, and unit representations leaf by leaf; it also bridges dynamic
`memory<bytes>` and `memory<string>` leaves. This supports direct ADT contract
fields, nested ADTs, and ADTs used as mapping values without making whole
mappings copyable values.

Rust mirrors that definition-side plan in
[`solver/derived_storage.rs`](crates/hir-ty/src/solver/derived_storage.rs) and
emits the concrete specialized methods in
[`specialize/derived_storage.rs`](crates/specialize/src/specialize/derived_storage.rs).
Definition-side evidence remains available to importing consumers without
letting a consumer import retroactively enable derivation. The dedicated
[`derived_storage_frontend.rs`](crates/hir-ty/tests/derived_storage_frontend.rs)
regressions pin that boundary and reject recursive ADTs and mapping-valued ADT
fields, including an otherwise-unused mapping-valued field; the storage E2E
fixtures cover direct, nested, mapping-value, enum, boolean, and dynamic-leaf
round trips.

`storage-adt-recursive-fail.sol` passes only in the `-g` reference ledger
because generated dispatch never makes its constructor/storage obligation
reachable. The full target frontend and the Rust full-frontend gate both reject
the required recursive storage assignment (`CanStore` upstream and the
corresponding `Assign` obligation in Rust). Its allowance is therefore a
recorded phase difference, not a Rust/Haskell semantic divergence.

### ABI metadata, selectors, and runtime evidence

Haskell
[`abiTypeOf`](https://github.com/argotorg/solcore/blob/2f372bde2801612814015a22319d0bc51486cbf0/src/Solcore/Desugarer/ContractDispatch.hs#L385-L405)
and Rust [`abi_type_of`](crates/hir-ty/src/contract/abi.rs) render primitive
`word` as `uint256`. Runtime dispatch is separate: generated `Method` values
retain source types and require classes from `std.dispatch` and `std`.

The current shared snapshot has this evidence matrix:

| Source ABI type | Selector spelling (inputs) | Decode input | Encode result | Status |
| --- | --- | --- | --- | --- |
| `uint256` | yes | yes | yes | complete |
| `address` | yes | yes | yes | complete |
| `bytes32` | yes | yes | yes | complete |
| `bytes4` | yes | yes | yes | complete at `2f372bde` |
| `memory<string>` | yes | yes | yes | complete |
| `memory<bytes>` | yes | yes | yes | complete |
| `()` | yes | yes | yes | complete |
| `bool` | yes | yes (strict) | yes | complete at `2f372bde` |
| `word` (ABI `uint256`) | **no** | **no** | **no** | unsupported by dispatch |
| pair/tuple | recursive | recursive | recursive | complete only when all components are complete |
| `calldata<array<t>>` | recursive `SigString(t) <> "[]"` | lazy calldata handle | **no** | input-only; complete when `t` has the required input evidence |
| non-recursive derived ADT (direct or array element) | structural Generic representation | compiler-derived `ABIDecode` | representation bridge | complete when every concrete type argument has the required evidence |
| recursive/excluded/manual ADT | rejected | no compiler-owned derived decode path | not accepted by Rust ABI preflight | unsupported |

The target's top-level `abi_encode` reserves a length word, writes the ABI
payload after it, and returns a valid `memory<bytes>`. Generated dispatch then
uses `MemoryPointer.ptr` and `MemorySize.len` to return only that payload, so
existing external ABI results remain unchanged while direct callers can safely
pass the encoded bytes to generic memory operations. The paired std update is
exercised by the `abi-encode-types` and `abi-encode-adt` raw vectors.

For `word`, the minimum upstream std correction is:

- `word : SigString`, returning `"uint256"`;
- `word : ABIEncode`, storing the primitive word directly; and
- `ABIDecoder(word, reader) : ABIDecode(word)`, reading it directly.

Until that upstream change is re-vendored, Rust's table-entry and work-fuel
bounds make the generated dispatch probe terminate with `SC0207`; the
`missing_word_abi_evidence` UI regression exercises the real shared std path.

At `2f372bde`, `std.dispatch` supplies `SigString` for sums and
`calldata<array<t>>`, plus a default bridge through `Generic<rep>`;
`std.ABIGeneric` supplies concrete compiler-derived `ABIAttribs` and
`ABIDecode` evidence for each eligible ADT and the matching default
representation-driven `ABIEncode` bridge. Rust accepts finite, compiler-owned
plans directly and beneath canonical std `calldata<array<...>>` wrappers. It
computes selectors from the instantiated Generic `SigString` (comma-joined
products and explicit `sum(l,r)` nodes), while ABI JSON keeps the source
spelling (`T` or `T[]`). For a nullary ADT this mirrors the target convention.
For a concrete parameterized ADT, however, upstream `abiTypeOf` has no matching
case and fails metadata emission; Rust's spelling such as `Point<uint256>` is
an intentional safe extension beyond exact target-emitter behavior. Neither
metadata convention claims that an arbitrary Solidity ABI consumer understands
Solcore sums.

Under that Rust metadata extension, concrete parameterized ADTs are supported
when every type argument discharges the generated ABI constraints, including
phantom parameters that do not occur in the instantiated Generic
representation. Recursion tracking distinguishes concrete instantiations, so
finite shapes such as `Box<Box<uint256>>` remain valid, while a definition whose
unspecialized representation mentions itself is rejected before expansion.
`no-generic-instance-for` and visible manual `Generic` evidence remain errors.
Manual `SigString`, `ABIAttribs`, `ABIEncode`, and `ABIDecode` instances are
rejected only when selected by the generated external ABI. Unrelated instances,
including those imported for a hand-written fallback dispatch table, remain
usable as in the pinned reference. The check follows solver evidence for the
selector, input decoder, and output encoder, including representation evidence
selected by compiler-derived ABI instances in their defining modules.
The `calldata<array<t>>` location itself remains input-only: the target std has
no `ABIEncode` instance for that lazy handle, so Rust follows derived Generic
representations and rejects the handle from every nested result position even
though the same type is valid in a parameter.

The known primitive-`word` evidence gap also applies when `word` occurs inside
a Generic representation or as a concrete ADT argument: metadata can spell the
shape as `uint256`, but full generated dispatch still reports the bounded
missing-evidence diagnostic described above.

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

The `storage_array` and `ufcs_array` rows in the Rust rejection allowance are
also phase-sensitive: the target verdict was recorded with generated dispatch
disabled, while Rust's full gate reaches external ABI validation. Rust rejects
their `memory<DynArray<address>>` result with `SC0231` because only canonical
`memory<string>` and `memory<bytes>` currently have matching metadata and
runtime evidence. Keeping the structured rejection is intentional.

## Standard-library recommendation

The `.sol` files in [`std/`](std/) are a canonical-syntax port of the shared
semantic artifact, not a Rust semantic fork. Do not apply Rust-only semantic
edits. A shared-std fix must:

1. reproduce with the pinned Haskell compiler and upstream std;
2. be fixed and tested in upstream Haskell std first;
3. pin the new upstream revision;
4. be re-vendored, migrated to canonical syntax, and copied byte-for-byte into
   `std/` and `crates/parser/tests/fixtures/corpus/ok/std/`; and
5. pass full dispatch and backend tests on both implementations.

The required invariant is:

> A public source type is supported if and only if ABI JSON can represent it,
> its canonical input signature can be hashed, calldata can be decoded into it,
> and a result can be encoded from it.

For the derived-ADT extension, “ABI JSON can represent it” means the explicit
source-name metadata convention (`SourceName` directly and `SourceName[]` for
lazy arrays), plus Rust's deliberate parameterized spelling extension such as
`Point<uint256>`. Selector hashing uses the structural Generic spelling. Both
spellings must be derived from the same compiler-owned ADT plan; accepting a
manual or recursive representation would break that link and is therefore
prohibited. The parameterized spelling satisfies this Rust invariant even
though the target's `abiTypeOf` fails before producing equivalent JSON.

The next upstream std change should complete `word`, then extend the
argument/result matrix for `word`, `uint256`, `address`, `bytes4`, `bytes32`,
`bool`, `memory<string>`, `memory<bytes>`, supported tuples, and canonical
calldata-array inputs. Unsupported location wrappers, calldata-array results,
std leaf types, and ADTs outside the finite compiler-derived surface remain
explicitly rejected.
Each test must use generated selector dispatch and must not define source
`main`, because source `main` suppresses the path under test.

Haskell ABI diagnostics and collision checks do not belong in std. Keep `import
* from std.dispatch;` explicit until both compilers have a specified
compiler-private dependency mechanism.

## Keeping the parity ledger honest

- Record Haskell solver mode and enabled phases with every generated verdict.
- Compare the same pipeline on both sides: full dispatch for contract fixtures,
  and no dispatch for isolated frontend fixtures.
- Exclude intentional Rust-only negative import fixtures from reference-pass
  difference counts.
- When resolving a divergence, remove its TSV allowance and add the smallest
  regression on the corrected side.

After a std update, verify the copies and then the full pipelines:

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

The byte-exact target raw-vector metadata is preserved, while both Yul and
Sonatina compile and execute the complete set against Osaka. Use the full
backend commands documented in [`tests/e2e/README.md`](tests/e2e/README.md).
