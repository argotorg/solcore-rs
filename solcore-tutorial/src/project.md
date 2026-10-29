# Build a bounded counter

**Goal:** combine persistent state, a condition, rejection, and tests in a small feature you can explain.
This exercise revisits the counter from [Contracts and persistent state](state.md) and the revert behavior from [Enums and pattern matching](enums.md).
Then build [an event booking contract](ticket-office.md) that combines modules, enums, traits, generics, and compile-time computation.

Start from the Counter example below and keep its `count`, `add`, and `read` members.
Your counter should accept a total of at most **10** units.
Adding more than the remaining capacity must revert, without changing the count.
Add a public `remaining()` function returning `uint256`.

This is a numeric exercise, not a payment or token contract.
Any caller may add; access control is outside its scope.
The starter shares your saved Counter edits from the state chapter; use **Reset project** if you want the original source.

<div class="tutorial-exercise" data-title="Counter starter">
<a class="tutorial-example" href="../projects/tutorial-counter.json?tab=run&amp;view=tests">Open the Counter starter in a full tab</a>
</div>

## Write the behavior first

Use these tests as your target, replacing the starter's test comments:

| Test comment | Expected behavior |
| --- | --- |
| Send `add(4)` | Accepted |
| Send `add(6)` | Accepted; the count reaches the bound |
| Expect `add(1)` to revert | Rejected |
| Expect `read()` to return `10` | Rejection preserved the existing value |
| Expect `remaining()` to return `0` | No space remains |

Keep the sends and rejected-call expectation above `add`, and the read expectations above their respective functions.
In the **Run** tab, select **Run tests** and click **Run all tests from start** after each change.

## Think about arithmetic before coding

Maintain the invariant **`count <= 10`** and compare `amount` with `10 - count` before adding.
This checks the requested increment against the remaining capacity without first computing a potentially overflowing sum.
Every accepted addition then stays within the bound.

Explain why the subtraction is valid: the count starts at zero, and every accepted update stays within the bound.
No other function should write an arbitrary value into `count`.

<details>
<summary>First hint</summary>

Use an `if` at the beginning of `add`.
If `amount > uint256(10) - count`, call `revertWithError("capacity exceeded")`.
Otherwise keep the original assignment.
`remaining()` returns `uint256(10) - count`.

</details>

<details>
<summary>Complete, tested solution</summary>

The solution has its own saved workspace, so opening it keeps your Counter starter edits.

<div class="tutorial-exercise" data-title="Bounded counter solution">
<a class="tutorial-example" href="../projects/tutorial-bounded-counter.json?tab=run&amp;view=tests">Open the bounded counter solution in a full tab</a>
</div>

```solidity
{{#include ../examples/tutorial-bounded-counter/BoundedCounter.sol}}
```

All 5 tests should pass.
The expected revert is itself a passing test when rejection occurs.
The subsequent read verifies that the count is still `10`.

</details>

## Try your own cases

Select **Try calls** in the **Run** tab and use **Reset contract** if shown.
Call `remaining()` and expect `10`.
No Arguments field is shown for this no-argument function.
Call `add` with `[11]` and expect a revert, then call `read()` and expect `0`.
Call `add` with `[0]` and verify that it succeeds without changing the count.
These cases explore the empty, oversized, and zero-input boundaries independently of the test sequence.

**Check your understanding:** why does this guard remain correct for very large `uint256` inputs?
What extra requirement would you need if only a particular account should be allowed to increment the counter?

<details>
<summary>Answer</summary>

The guard compares `amount` with the remaining capacity before performing the addition.
The invariant keeps the subtraction within range, and the guard ensures an accepted addition cannot exceed `10` or overflow.
Restricting updates to one account would also require checking the caller's identity before changing storage.

</details>
