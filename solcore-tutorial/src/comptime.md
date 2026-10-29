# Compile-time computation

**Goal:** use `comptime` to calculate a value before execution and distinguish it from an argument supplied by a future caller.

<div class="tutorial-exercise" data-title="Compile-time answer">
<a class="tutorial-example" href="../projects/comptime.json?tab=run&amp;view=tests">Open Compile-time answer in a full tab</a>
</div>

```solidity
{{#include ../examples/comptime/Answer.sol}}
```

The parameter `comptime value: uint256` requires a value known while compiling.
The result type `comptime<uint256>` also describes a compile-time value.
Here `uint256(21)` is known, so the compiler evaluates `21 + 21`.
The `return result` in `answer()` uses the resulting `42` as an ordinary `uint256` return value.

This does not store the result of a previous execution in contract storage.
An ordinary parameter of a public function is supplied by a future caller and is not known while compiling.
Passing such a parameter to `double` is a compilation error, even if a future caller happens to supply `21`.

## Run and observe

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
The 1 test should pass, with expected and actual values both `42`.
Select **Try calls**, choose `answer()`, and click **Call function**.
It takes no arguments, so there is no Arguments field.
Its result is also `42`.

Click **Compile**, then inspect the **Yul** tab using the default **solc** backend.
Yul is an intermediate language the compiler generates before producing EVM bytecode.
Look for the constant `42` in the code that encodes the return value: the generated `abi_encode` call receives `42` directly.
There is no call to `double` or addition of `21 + 21` to produce that value at runtime.
The output still contains unrelated additions for memory management and interface handling.
Generated names and layout may change, but the distinction to look for is a computed constant in place of the calculation in `double`.

## Change it

Change the public declaration to `function answer(value: uint256) public returns (uint256)` and replace `double(uint256(21))` with `double(value)`.
Update its test comment to `// #[(21) -> 42]` so it supplies the new argument.
Click **Compile** and inspect **Problems** before attempting to run the test.
Why does supplying `21` in a test fail to make the public parameter a compile-time value?

<details>
<summary>Hint and answer</summary>

The compiler reports `SC0240: runtime value passed to comptime parameter 'value' of 'double'`, pointing to the argument in `double(value)`.
The public argument arrives when the contract is called; the compiler cannot use it to evaluate `double` ahead of time.
The test comment describes a later call and does not change that requirement.
This is a compilation error, so there is no new program to execute or expectation to check.

Restore `answer()`, the call `double(uint256(21))`, and the test comment `// #[() -> 42]`.
Compile and rerun the test to confirm that the original program works again.

</details>

**Check your understanding:** how does the Yul output show that the calculation in `double` happened before execution?

<details>
<summary>Answer</summary>

The return-value encoding uses the constant `42` directly.
It does not call `double` or calculate `21 + 21` to obtain that value when `answer()` runs.

</details>
