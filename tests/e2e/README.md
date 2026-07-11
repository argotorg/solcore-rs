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

The directive grammar is `#[(arguments) -> expected]`. A parenthesized result
denotes multiple outputs, and nested parentheses denote a tuple value. Decimal
and hexadecimal `uint256` words, booleans, and static tuples are supported.
Normal comments are ignored, while a malformed comment beginning with `#[` is
an error.

Each case is lowered by the selected backend, compiled to EVM creation
bytecode, deployed to Anvil, and called through the generated ABI selector.
Set `E2E=1` to run execution tests. `E2E_PIPELINE_ONLY=1` stops after backend
code generation; `E2E_REQUIRED=1` makes missing tools an error. Anvil defaults
to the Osaka hardfork to match Sonatina's target; `ANVIL_HARDFORK` can override
it for an alternate runtime.
