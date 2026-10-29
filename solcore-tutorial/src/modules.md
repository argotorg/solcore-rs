# Modules and optional values

**Goal:** split code across files and use `export` to choose which names other modules can use.

Represent a missing result explicitly, so a caller can decide how to handle it.
The `Option` used here is defined in the example's `option.sol`; it is not a built-in standard-library type.

<div class="tutorial-exercise" data-title="Splitter and Option">
<a class="tutorial-example" href="../projects/option.json?tab=run&amp;view=tests">Open Splitter and Option in a full tab</a>
</div>

## A reusable module

Select `option.sol` in the file list.

```solidity
{{#include ../examples/option/option.sol}}
```

The `a` in `Option<a>` is a type parameter.
For `Option<uint256>`, `Some` carries an integer and `None` carries no value.
`unwrapOr<a>` is a generic function: its control flow works for different choices of `a`.

Integer division drops the remainder: `uint256(10) / uint256(3)` gives `3`.
`checkedDiv` checks the divisor first and returns `Option.None` when it is zero, so callers can distinguish a missing result from a valid numeric result.

`export { Option(*), checkedDiv, unwrapOr }` makes these names available to other modules.
`Option(*)` exports the type and its constructors.

## Using the module

Return to `Splitter.sol`.
`from option` refers to the example's `option.sol`.
Selecting `option.sol` to read it does not change which file is compiled: compilation still starts from `Splitter.sol` and follows its imports.

```solidity
{{#include ../examples/option/Splitter.sol}}
```

`share` uses `unwrapOr` to choose `0` when the result is absent.
`remainder` uses `match`; if there are no recipients, it returns the original pot.
The same `Option` value lets each caller choose a policy appropriate to its purpose.

## Run and observe

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 4 tests pass.
You can also select **Try calls**, choose each function, enter its **Arguments**, and click **Call function**:

| Function | Arguments | Result |
| --- | --- | --- |
| `share(uint256,uint256)` | `[10,3]` | `3` |
| `remainder(uint256,uint256)` | `[10,3]` | `1` |
| `share(uint256,uint256)` | `[10,0]` | `0` |
| `remainder(uint256,uint256)` | `[10,0]` | `10` |

## Change it

Add an expectation for a pot of `14` and `4` recipients to each function.
Predict both the share and the remainder before running the tests.

<details>
<summary>Hint and answer</summary>

Add `// #[(14, 4) -> 3]` immediately above `share`, and `// #[(14, 4) -> 2]` immediately above `remainder`.
Integer division gives a share of `3`; the remainder is `14 - 3 * 4 = 2`.
**Run all tests from start** now passes 6 tests.

</details>

**Check your understanding:** why can `share` and `remainder` choose different behavior when `checkedDiv` returns `None`?

<details>
<summary>Answer</summary>

`checkedDiv` reports the missing result as a value without deciding what the caller should do next.
Each caller handles `None` itself: `share` chooses zero, while `remainder` keeps the original pot.

</details>
