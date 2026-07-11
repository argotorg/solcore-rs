# Backend E2E fixtures

Both the Yul and Sonatina backends run every `**/main.solc` fixture in this
directory. Expectations live next to the contract function they exercise:

```solcore
// #[(0, 1) -> 1]
// #[(1, 1) -> 2]
public function add(x: uint256, y: uint256) -> uint256 {
  return Add.add(x, y);
}
```

The directive grammar is `#[(arguments) -> expected]`. Directive values are
typed contextually from the target function's canonical ABI signature. Decimal
and hexadecimal words, booleans, and static tuples (including the ABI tuple
representation of a single-constructor product data type) are supported. An
argument or result with the wrong type or arity is rejected while resolving the
fixture, before any EVM call is made.

The outer parentheses delimit the argument or result list; another pair is
needed for a tuple value. Thus a single composite argument and result use
double parentheses:

```solcore
data Point = Point(word, bool);

// #[((7, true)) -> ((7, true))]
public function echo(point: Point) -> Point {
  return point;
}
```

This nesting also distinguishes a single nested composite result from multiple
outputs. For example, `(((7, true), 9))` is one `TaggedPoint(Point, word)`
result, while `((7, true), 9)` is two results: a `Point` followed by a word.
The complete shared example is in `composite-values/main.solc`. A directive
such as `((7, 9), 9)` for `pack(Point, word)` is an error because the second
`Point` field must be a boolean. Normal comments are ignored, while a malformed
comment beginning with `#[` is an error.

Each case is lowered by the selected backend, compiled to EVM creation
bytecode, deployed to Anvil, and called through the generated ABI selector.
Set `E2E=1` to run execution tests. `E2E_PIPELINE_ONLY=1` stops after backend
code generation; `E2E_REQUIRED=1` makes missing tools an error. Anvil defaults
to the Osaka hardfork to match Sonatina's target; `ANVIL_HARDFORK` can override
it for an alternate runtime.
