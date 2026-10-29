# Generics and constraints

**Goal:** express a reusable operation with a type parameter and state its required capabilities with `where`.
The example is a version registry, with an ordering that compares the major version first.

<div class="tutorial-exercise" data-title="Generic Registry">
<a class="tutorial-example" href="../projects/generics.json?tab=run&amp;view=tests">Open Generic Registry in a full tab</a>
</div>

```solidity
{{#include ../examples/generics/Registry.sol}}
```

`max<a>(x: a, y: a)` takes 2 values of the same type and returns that type.
Because it uses `>`, it requires `where a: Ord`.
The function itself does not inspect any `Version` fields.

`Version` is an enum carrying 2 integers, extracted with `match`.
`impl Eq<Version>` defines equality; `impl Ord<Version>` defines ordering.
`Ord` requires `Eq`, so the example provides both implementations.
Different major versions are compared by major; minor versions matter only when the majors are equal.
Thus `2.1` is newer than `1.99`.

`publish` stores the `max` of the current and incoming version.
This reuses the ordering definition whenever a new value arrives.

## Run and observe

In the **Run** tab, select **Run tests** and click **Run all tests from start**.
All 6 tests should pass.
Publishing `1.9 → 2.1 → 1.99 → 2.0` leaves the registry at `2.1`.

Select **Try calls** in the **Run** tab and use **Reset contract** if shown.
Then select these functions and click **Call function** in order:

| Function | Arguments | Result |
| --- | --- | --- |
| `publish(uint256,uint256)` | `[1,9]` | Success |
| `publish(uint256,uint256)` | `[2,1]` | Success |
| `publish(uint256,uint256)` | `[1,99]` | Success |
| `newestMajor()` | `[]` | `2` |
| `newestMinor()` | `[]` | `1` |

The `[]` rows are no-argument functions: no Arguments field appears for them.

## Change it

Remove `where a: Ord` from the declaration of `max`, leaving its body and the `Version` implementations unchanged.
Click **Compile** and inspect **Problems**.
Explain why defining `impl Ord<Version>` does not make the generic function valid for every possible `a`.
Restore the constraint afterward and rerun the tests.

<details>
<summary>Hint and answer</summary>

The compiler reports `SC0207: cannot satisfy trait constraint: a: Ord`.
The `>` operation needs an ordering for the generic type `a`, not only for `Version`.
The constraint promises that callers supply a type with that capability.
Without it, the body of `max` asks for an operation that its declaration does not require.

Restore `where a: Ord` after `returns (a)`.
Compilation should succeed again, and all 6 tests should pass.

</details>

**Check your understanding:** why can the body of `max` work without knowing the fields of `Version`?

<details>
<summary>Answer</summary>

It only compares and returns values of type `a`.
The `Ord` constraint provides the comparison, and `impl Ord<Version>` supplies its meaning when `a` is `Version`.

</details>
