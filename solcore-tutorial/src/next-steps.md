# Where to go next

**Goal:** use an enum behind a module interface to preserve an invariant, then choose a project that combines more of the features you have learned.
The [event booking exercise](ticket-office.md) combines the language features in a complete reservation workflow.
This example explores a different module boundary: hiding an enum's constructor to preserve a storage invariant.

<div class="tutorial-exercise" data-title="AMM invariants">
<a class="tutorial-example" href="../projects/invariants.json?tab=run&amp;view=tests">Open AMM invariants in a full tab</a>
</div>

```solidity
{{#include ../examples/invariants/Amm.sol}}
```

The representation of `Pool` lives in a separate file:

```solidity
{{#include ../examples/invariants/pool.sol}}
```

`export { Pool, ... }` exports the type without its constructor, unlike `Pool(*)`.
Outside callers create values through `mkPool`, keeping its initialization check in one place.
`require(condition, "message")` continues when the condition is true and reverts with the message when it is false.
`swapXforY` returns a new `Pool` together with an output amount; the contract stores the updated `Pool`.
This example records numeric reserves.
It does not implement token transfers, liquidity-provider share accounting, or access control for a deployed exchange.

## Run and observe

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 5 tests should pass.
Starting from `(10,1000)`, the `swap(10)` send leaves `(20,500)`.
Select **Try calls** and use **Reset contract** if shown.
Choose `swap(uint256)`, enter `[10]` in **Arguments**, and click **Call function**; it returns `500`.
Call `poolX()` and `poolY()` without arguments to confirm `20` and `500`.

## Change it

Read the condition in `mkPool`.
If you change the first initial reserve in Amm's constructor to `uint256(0)`, will compilation or execution fail?
Try it and inspect the result.

<details>
<summary>Hint and answer</summary>

The types still match, so compilation succeeds.
Deployment reverts because `mkPool`'s `require` fails.
Calling `poolX()` without arguments in **Try calls** attempts that initial deployment and reports the failure.
Restore `uint256(10)` and try again.
Type constraints and runtime value checks have distinct roles.

</details>

## Choose a direction

| What to explore | Example | Where to look |
| --- | --- | --- |
| Mappings, enums, and modules | [Mini NFT](../projects/mini-nft.json?tab=run&view=calls) | `Option<address>`, `mint`, `exists` |
| Combining traits, generics, and modules | [Composition](../projects/composition.json?tab=run&view=calls) | `engine.sol` and its three contracts |
| Dividing state and behavior across modules | [Extensions](../projects/extensions.json?tab=run&view=calls) | The module state structs and `update` |
| Lambdas and locking wrappers | [Reentrancy](../projects/reentrancy.json?tab=run&view=calls) | `withWriteLock`, `withReadLock`, `lam` |

These are examples for studying design.
Composition's signature example omits replay protection using a nonce, chain ID, and contract address.
The vault examples track credits but omit asset custody.
A locking example alone does not establish the security of a real contract.

Solcore is under development; check unfamiliar constructs with the current compiler and standard library.
Support for a user-defined type in a program does not guarantee that external tools can encode it for the contract's ABI.
Execution in the browser does not verify deployment on a live network.

For a small next project, add state inspection and transition tests to Escrow, or implement another ordered type for Registry.
Include old versions and invalid transitions in your tests as well as successful inputs, so later changes can be checked against those behaviors.

**Check your understanding:** what does hiding the `Pool` constructor prevent, and which value checks are still performed at runtime?

<details>
<summary>Answer</summary>

Callers outside the module cannot construct a `Pool` directly, so they must use its exported functions.
`mkPool` checks that both reserves are nonzero, and `swapXforY` checks that the reserve product has not decreased.
These conditions concern values supplied during execution and are checked at runtime.

</details>
