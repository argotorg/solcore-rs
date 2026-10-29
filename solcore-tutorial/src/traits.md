# Traits and implementations

**Goal:** declare an operation in a `trait` and implement it for 2 types.
A trait names an operation, and each `impl` defines it for one type.
Here, the same `Toggle.toggle` call changes a switch or a door according to the type of its argument.

<div class="tutorial-exercise" data-title="Switch and door traits">
<a class="tutorial-example" href="../projects/trait.json?tab=run&amp;view=tests">Open Switch and door traits in a full tab</a>
</div>

```solidity
{{#include ../examples/trait/LightSwitch.sol}}
```

`trait Toggle<a>` requires a `toggle` function that takes an `a` and returns an `a`.
`impl Toggle<Switch>` supplies the behavior for `Switch`: `Off` becomes `On`, and `On` becomes `Off`.
`impl Toggle<Door>` supplies the behavior for `Door`: `Closed` becomes `Open`, and `Open` becomes `Closed`.

The contract's `flip` method calls `Toggle.toggle(state)` and stores the returned `Switch`.
Its `flipDoor` method calls `Toggle.toggle(door)` and stores the returned `Door`.
The argument's type selects the implementation, so callers use one operation name for both types.
The next chapter shows how a generic function can require an operation without choosing one concrete type.

## Run and observe

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 4 tests should pass.
The send on `flip` makes `isOn` return `true`; the send on `flipDoor` makes `isOpen` return `true`.
Running **Run test** for an individual expectation also replays its preceding sends.

For a manual experiment, select **Try calls** in the **Run** tab and use **Reset contract** if shown.
Then select each function below and click **Call function**.
These functions take no arguments, so there is no Arguments field to fill in.

| Function | Result |
| --- | --- |
| `isOn()` | `false` |
| `isOpen()` | `false` |
| `flip()` | Success |
| `isOn()` | `true` |
| `isOpen()` | `false` |
| `flipDoor()` | Success |
| `isOpen()` | `true` |

The calls update different fields using the same trait operation.
Tests and manual calls use independent state, so a passing test does not change the manual contract.

## Change it

Add a `flipBoth()` method that toggles the switch and the door together.
Use `Toggle.toggle` once for each field, and check the result through **Try calls**.

<details>
<summary>Hint and answer</summary>

Inside `LightSwitch`, add a public `flipBoth` function with no parameters and no return value, using the declaration of `flip` as a guide.
Its statements are `state = Toggle.toggle(state);` and `door = Toggle.toggle(door);`.
Each call selects a different `impl` from the argument's type.

Editing the source starts a new manual contract session.
Call `flipBoth()`, then `isOn()` and `isOpen()`; both readings should be `true`.
Call `flipBoth()` again and read both fields; both should be `false`.
Next call `flip()` followed by `flipBoth()`; `isOn()` should return `false` and `isOpen()` should return `true`.
This last sequence checks that each field is toggled from its own current state.

</details>

**Check your understanding:** how does `Toggle.toggle` choose between the switch and door implementations?

<details>
<summary>Answer</summary>

The argument's type determines the implementation: `Switch` selects `impl Toggle<Switch>`, and `Door` selects `impl Toggle<Door>`.
The trait declares the operation both implementations must provide.

</details>
