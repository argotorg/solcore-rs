# Values, functions, and control flow

**Goal:** follow a local variable through a helper call, an `if`, and a loop.

<div class="tutorial-exercise" data-title="Values and control flow">
<a class="tutorial-example" href="../projects/tutorial-basics.json?tab=run&amp;view=tests">Open Values and control flow in a full tab</a>
</div>

```solidity
{{#include ../examples/tutorial-basics/Basics.sol}}
```

## Predict and run

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 5 tests should pass:

| Function | Arguments | Result |
| --- | --- | --- |
| `adjust` | `21, true` | `42` |
| `adjust` | `21, false` | `21` |
| `adjust` | `0, true` | `0` |
| `sumBelow` | `0` | `0` |
| `sumBelow` | `5` | `10` |

The `// #[...]` lines are test comments describing inputs and expected results.
You will learn how to write them in the [next chapter](tests.md).

## Values and local bindings

`uint256` represents an unsigned 256-bit value, and `bool` has the values `true` and `false`.
`let result: uint256 = value;` introduces a local binding with an explicit type.
It exists only during the call and can be reassigned.

`double` is a top-level helper.
Its parameter and return types are part of its declaration, and the contract calls it by name.
It is not a public contract method.
In `adjust`, only a true `bonus` enters the `if` body; both branches then reach the final return.

The standard library defines `uint256` around the low-level `word` type and supplies its operators through traits.
Use `uint256` for public parameters and return values; primitive `word` is not supported there.
`uint256` has a finite range: design calculations so their results stay within it.

Each loop iteration costs gas.
Keep inputs small: a finite loop can still exhaust the runner's gas limit.

## Change it

In the **Run** tab, select **Try calls**, choose `sumBelow(uint256)`, enter `[6]` in **Arguments**, and click **Call function**.
Predict the result first.
Then change `i < limit` to `i <= limit`, predict the new result, and call the function again with `[6]`.
Restore `i < limit` afterward.

<details>
<summary>Hint and answer</summary>

The original loop adds the integers from `0` through `5`, giving **15**.
With `i <= limit`, it also adds `6`, giving **21**.
Changing the condition changes the function's meaning, so restore it before continuing.

</details>

**Check your understanding:** why does `adjust(21, false)` return `21` without calling `double`?

<details>
<summary>Answer</summary>

`result` starts at `21`, and a false `bonus` skips the only statement that changes it.
The function therefore returns the original value.

</details>
