# Enums and pattern matching

**Goal:** group the data needed by each state into an `enum`, then use `match` to inspect it.

Building on the previous chapter's storage, model a transition from awaiting payment to funded to released.
This `Escrow` records numbers for learning.
It does not receive or transfer Ether, or restrict who can call its methods.

<div class="tutorial-exercise" data-title="Escrow states">
<a class="tutorial-example" href="../projects/pattern-matching.json?tab=run&amp;view=tests">Open Escrow states in a full tab</a>
</div>

```solidity
{{#include ../examples/pattern-matching/Escrow.sol}}
```

`AwaitingPayment` carries no data.
`Funded(uint256)` and `Released(uint256)` each carry a recorded amount.
The declaration shows which data belongs to which state, instead of keeping a numeric state flag and amount separately.
`case Phase.Funded(amount)` checks the variant and binds its payload to `amount`.
`_` ignores a payload; `default` handles cases not matched earlier.

`phase` lives in contract storage.
Storing this enum needs the `std.Generic` and `std.StorageGeneric` imports.
An enum declaration does not mean every possible user-defined type can be stored without restrictions.

The constructor runs once when a new contract instance is deployed.
Here it sets `phase` to `Phase.AwaitingPayment` before any calls are made.

## Run and observe

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 6 tests pass.
After the `deposit(42)` send, a second deposit reverts.
The release expectation returns `42`, and the final status is `2`.
An expectation does not commit state changes: that is why `release` has a `// #[send()]` after its `// #[() -> 42]` expectation.

To inspect the transitions yourself, select **Try calls** and use **Reset contract** if it is shown.
Then select each function and click **Call function** in this order:

| Function | Arguments | Result |
| --- | --- | --- |
| `status()` | `[]` | `0` |
| `deposit(uint256)` | `[42]` | Success |
| `status()` | `[]` | `1` |
| `release()` | `[]` | `42` |
| `status()` | `[]` | `2` |
| `release()` | `[]` | Revert |

For a no-argument function, `[]` in the table means there is no **Arguments** field to fill in.

The last revert is intentional.
`revertWithError("nothing to release");` calls a standard-library function with an error message string.
It stops the current call and rolls back that call's state changes, leaving changes from earlier successful calls intact.
Here it rejects a transition that is not allowed.
The `return 0;` after it is unreachable, but satisfies the function's declared return type during compilation.
The test comment `// #[() -> revert]` expects a call to fail this way; that rejection makes the test pass.

## Change it

Add `releasedAmount()`: return the recorded amount in the `Released` state, and `0` in every other state.
Place its test comment after the existing `status` method, so the test runs after the release send.

<details>
<summary>Hint and answer</summary>

Use `status()` as a guide.
Inside the same contract, add a public function named `releasedAmount`, with no parameters and a `uint256` return type.
Its body matches on `phase`: in `case Phase.Released(amount)`, use `return amount;`; in `default`, use `return 0;`.
Place `// #[() -> 42]` immediately above the new function, and keep that function after `status` so its expectation sees the preceding setup sends.

**Run all tests from start** now passes 7 tests.
Editing the source resets the manual contract session, so repeat deposit and release if you want to check this function through **Try calls**.

</details>

**Check your understanding:** why does the release expectation need a separate send before the final status expectation?

<details>
<summary>Answer</summary>

The release expectation checks its return value but discards its state change.
The send commits the transition to `Released`, so the later `status()` call sees status `2`.

</details>
