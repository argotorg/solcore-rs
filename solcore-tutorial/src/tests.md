# Public functions and tests

**Goal:** call a public function with arguments and turn a result into a repeatable test.

<div class="tutorial-exercise" data-title="Calculator tests">
<a class="tutorial-example" href="../projects/std-usage.json?tab=run&amp;view=tests">Open Calculator tests in a full tab</a>
</div>

```solidity
{{#include ../examples/std-usage/Calculator.sol}}
```

## The contract interface

This contract has 2 public functions.
The ABI (Application Binary Interface) describes their names, parameter types, and return types so tools can encode calls and decode results.
In the playground, **Try calls** handles this encoding for supported types.

In the **Run** tab, select **Try calls**, choose `viaOperator(uint256,uint256)`, enter `[20, 22]` in **Arguments**, and click **Call function**.
The decoded result is **42**.
A successful **Compile** also makes the **ABI** tab available for inspection.

Arguments use a JSON array, not Solcore constructor syntax.
Booleans are `true`/`false`.
For integers too large to represent exactly as JavaScript numbers, use quoted decimal strings, for example `["20", "22"]`.
Use small inputs for now.

## Test comments belong beside the behavior

`// #[(20, 22) -> 42]` is a test comment supplying 2 inputs and the expected result.
This kind of test comment is an **expectation**.
It belongs to the function immediately below it and is understood by the playground and test tooling; it is not executed by the deployed contract.

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 4 tests should pass: each implementation returns `42` for `(20, 22)` and `7` for `(0, 7)`.
**Run test** above an individual test comment runs that test.
Test output shows expected and actual values, so a failed expectation is different from a syntax or type error.

Both implementations add values.
`a + b` resolves through library traits; `Num.add(a, b)` calls the method explicitly.
You do not need to define a trait yet to use this library behavior.

## Change it

Add the expectation `(3, 4) -> 7` to both functions.
Then deliberately change **only one expectation** to `8`, run that test, and inspect the failure.
Correct it afterward.

<details>
<summary>Hint and answer</summary>

Add `// #[(3, 4) -> 7]` above each function, next to its existing test comments.
With the intentional `8` expectation, compilation should still succeed, but the test should report expected `8`, actual `7`.
Restoring `7` leaves 6 passing tests.

</details>

**Check your understanding:** does calling a function through **Try calls** verify a test comment's expectation?

<details>
<summary>Answer</summary>

No: a manual call shows the result without checking it against a test comment.
Selecting a test can fill the manual controls, but only running the test checks its expected result.

</details>
