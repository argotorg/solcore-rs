# Backend E2E fixtures

Both the Yul and Sonatina backends generate a test for every `**/main.sol`
fixture in this directory. Selector-dispatched fixtures explicitly open both
`std` and `std.dispatch` with `import * from ...`. Expectations live next to the contract
function they exercise:

```solcore
// #[(0, 1) -> 1]
// #[(1, 1) -> 2]
function add(x: uint256, y: uint256) public returns (uint256) {
  return Add.add(x, y);
}
```

The directive grammar is `#[(arguments) -> expected]`. Directive values are
typed contextually from the target function's canonical ABI signature. Decimal
and hexadecimal `uint256` values, booleans, and static tuples are supported. An
argument or result with the wrong type or arity is rejected while resolving the
fixture, before any EVM call is made. The execution fixtures deliberately use
only selector ABI types supported by the shared reference std. In particular,
they do not expose primitive `word`. Direct ADTs and the
`calldata<array<T>>` ADT surface use raw scenario comments instead: their selectors
are derived from `T`'s structural Generic representation, and
algebraic/dynamic-array values are outside the inline directive value grammar.

State-changing calls use `#[send(arguments)]`. A send directive submits a
transaction, waits for a successful receipt, and preserves its storage changes
for every directive that follows it in the fixture. It has no result
expectation because transaction receipts do not expose EVM returndata. Put a
normal call directive on a later public method to assert the persisted state:

```solcore
// #[send(41)]
function set(value: uint256) public { stored = value; }

// #[() -> 41]
function readAfterSend() public returns (uint256) { return stored; }
```

The outer parentheses delimit the argument or result list; another pair is
needed for a tuple value. Thus a single composite argument and result use
double parentheses:

```solcore
// #[((7, 1)) -> (7, 1)]
function echo(point: (uint256, uint256)) public returns (uint256, uint256) {
  return point;
}
```

Top-level tuple returns are flattened into multiple ABI results. The language's
right-nested tuple representation also flattens a nested tuple used as one ABI
parameter: the single argument `((uint256, uint256), uint256)` is written as
`((7, 1, 9))` in a directive. By contrast, two parameters consisting of a pair
and a scalar are written as `((7, 1), 9)`. The complete shared example is in
`composite-values/main.sol`. Normal comments are ignored, while a malformed
comment beginning with `#[` is an error.

Raw scenarios handle ABI shapes not supported by typed expectations, arbitrary
calldata, value transfers, constructor failures, and ordered transactions. Put
consecutive scenario comments directly before the contract declaration:

```solcore
// #[scenario("value-transfer")]
// #[deploy(0x, value = 42) -> success]
// #[raw(0xb69ef8a8, label = "balance()") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xdeadbeef, value = 1, label = "unknown selector") -> revert(0x4924aef0)]
contract C {
  // ...
}
```

`scenario("name", evm = "osaka")` is optional descriptive metadata; the contract
to deploy is the declaration owning the comments. `deploy(bytes, value = n)`
appends the supplied bytes to creation bytecode. It must precede all calls and
may appear only once. Omitting it deploys with empty arguments and zero value.
A deployment may expect `success`, `success(bytes)`, `revert`, or `revert(bytes)`;
omitting bytes checks only the status, whereas `success(0x)` or `revert(0x)`
requires empty output. A constructor-only scenario is supported, and no call may
follow a deployment expected to fail.

`raw(bytes, value = n, label = "description")` supplies complete calldata,
including any selector. `value` defaults to zero and `label` is optional.
Successful raw calls require `success(bytes)` so their output is always checked;
`revert` accepts any revert payload and `revert(bytes)` requires an exact match.
Byte strings use a `0x` prefix and an even number of hexadecimal digits, including
`0x` for empty bytes. Values are decimal or hexadecimal uint256 amounts in wei.
Names and labels are quoted strings supporting `\\`, `\"`, `\n`, `\r`, and `\t`.
Each directive occupies one line comment or one block comment; a block comment
can wrap a long directive over multiple lines. Ordinary comments are ignored.
Malformed directives, multiple contract scenarios, and mixing a scenario with
typed method expectations are errors rather than silently skipped tests.

Within each compiled backend/codegen variant, raw steps execute in source order
as real transactions from `0x1212121212121212121212121212120000000012`. The same
execution supplies the status/output assertion and the state observed by later
steps. Both backends compile and execute these scenarios against Osaka, regardless
of the optional `evm` metadata, on a fresh dedicated Osaka Anvil instance. Typed
method expectations keep their existing behavior: ordinary expectations use
`eth_call`, while `#[send(...)]` persists state but has no output expectation.
Raw scenarios are supported by the native backend E2E runners.

The upstream raw vectors have been migrated into these source comments. Their
calldata, value transfers, expected status/output, call order, labels, and EVM
metadata are retained; there are no companion JSON files. The historical
2f372bde snapshot supplied 51 executable source/vector pairs; later upstream and
local regressions extend that corpus. Core sources are syntax-migrated semantic
ports, not byte-exact copies of that snapshot.

The active std/semantic target is now `a7b7afb0`. Variant-related raw vectors
have been updated to its constructor-keccak tags, and latest struct,
message-context, and assembly regressions are included. Local additions reject
unknown constructor tags and verify the corrected single-tag static size for
three or more variants. See `SEMANTIC_DIFFERENCES.md` for upstream defects.

For a finite derived ADT `T`, ABI JSON retains the source spelling (`T` or
`T[]`), while selectors still use its structural Generic signature (for
example, `rt(sum(uint256,bytes))`). On the wire, compiler-derived
multi-constructor ADTs use one full `keccak256("Constructor(field signatures)")`
tag, followed by that constructor's payload. Explicit structural `sum`
encoders and `std.ABIGeneric.encode`/`decode` retain 0/1 Generic tags.
Upstream still cannot emit parameterized ADT metadata; Rust deliberately
supports source spellings such as `Point<uint256>`.
This extension is supported only when every type argument, including an unused
phantom argument, has the required ABI evidence. Finite nested instantiations
are distinguished from definition-recursive representations. Recursive,
manually represented, and same-named non-std array/location types are rejected
before backend execution, as is a `calldata<array<t>>` handle nested anywhere
inside an encoded ADT result.

Each case is lowered by the selected backend, compiled to EVM creation
bytecode, deployed to Anvil, and called through the generated ABI selector.
Set `E2E=1` to run execution tests. `E2E_PIPELINE_ONLY=1` stops after backend
code generation; `E2E_REQUIRED=1` makes missing tools an error. Anvil defaults
to the Osaka hardfork for directive fixtures; `ANVIL_HARDFORK` can override it
for an alternate Yul runtime. Raw scenarios always compile and run against Osaka.

For local optimized runs, use the workspace's E2E profile. It uses moderate
optimization (`opt-level = 2`) without LTO, keeping execution representative
while avoiding the native release profile's link-time optimization cost. Run
the complete E2E fixture set through Yul:

```sh
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-yul --test e2e --locked -- \
  --nocapture --test-threads=1
```

Run the same complete E2E fixture set through Sonatina:

```sh
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-sonatina --test e2e --locked -- \
  --nocapture --test-threads=1
```
