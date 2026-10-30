# Build an event booking contract

**Goal:** combine modules, enums, traits, generics, and compile-time computation in a contract with a complete booking workflow.
Build a `TicketOffice` for an event with 12 seats, standard and premium tickets, cancellations, and an organizer who can close registration.
Unlike the bounded counter, this contract must keep each caller's booking consistent with the shared number of available seats.

The contract records seat reservations and price quotes in credits.
It does not collect payments or transfer refunds; the return value from cancellation is the quote being removed.

## Define the behavior

| Operation | Required behavior |
| --- | --- |
| Quote | Standard tickets cost 10 credits per seat; premium tickets cost 15. Quoting does not reserve seats. |
| Reserve | While registration is open, a caller without a booking can reserve between 1 and the remaining number of seats. |
| Inspect | Each caller sees only their own booking through `mySeats()` and `myQuote()`; `remaining()` is shared. |
| Cancel | While open, remove the caller's booking, return its recorded quote, and make its seats available again. |
| Close | Only the account that created the contract can close registration. Closing is permanent and preserves existing bookings. |
| Reject | Zero or excessive quantities, duplicate bookings, missing cancellations, and forbidden state transitions must revert. |

Both ticket categories use the same seat inventory.
Each account can hold one active booking, then cancel and book again while registration remains open.
After closing, quotes and inspection still work, but reservation and cancellation do not.

Keep 2 invariants in mind: the number of reserved seats stays between 0 and 12, and it equals the sum of all active bookings' quantities.
A rejected call must leave both the bookings and the seat count unchanged.

## Open the starter

The starter compiles, but its marked `TODO` bodies are unfinished: pricing and booking accessors return placeholder zeros, while reservation and cancellation reject calls.
Its 2 passing test comments only check the supplied capacity and initial open state; they do not mean that the exercise is finished.
Complete the tasks below and add checks for the required behavior.

<div class="tutorial-exercise" data-title="TicketOffice starter">
<a class="tutorial-example" href="../projects/tutorial-ticket-office-starter.json?tab=run&amp;view=calls">Open the TicketOffice starter in a full tab</a>
</div>

| File | Responsibility |
| --- | --- |
| `pricing.sol` | Declare `Priced`, implement it for `Standard` and `Premium`, and share the generic quote calculation. |
| `booking.sol` | Represent an empty or active booking and inspect its payload using `match`. |
| `TicketOffice.sol` | Store bookings, enforce capacity and lifecycle rules, and expose public functions. |

The supplied `mapping(address => Booking)` stores a separate `Booking` for each account address.
An unset entry reads as the first enum variant, `Booking.Empty`.
`msgSender()` from std returns the address of the account making the current call.
Indexing the mapping with it lets a caller change their own entry without supplying someone else's address.
The constructor saves its caller as the organizer; the supplied `close()` checks that identity before changing the `Sale` enum.

`capacity()` uses a `comptime` helper to multiply 3 rows by 4 seats while compiling.
The public method returns the resulting 12 at runtime.
Changing these layout constants is a source change, so the editor starts a new contract on the next call.

## 1. Implement pricing

In `pricing.sol`, complete `unitPrice` for both types, then implement `quote<a>` using `Priced.unitPrice(ticket)` and the requested quantity.
Keep the `where a: Priced` constraint: it makes that operation available for any type accepted by `quote`.
The 2 single-variant enums select pricing policies; a premium ticket costs more but still occupies one seat.

In **Run**, select **Try calls** and choose **Account 1** under **Caller**.
Call `quoteStandard(uint256)` with `[2]` and expect `20`, then `quotePremium(uint256)` with `[2]` and expect `30`.
The supplied public wrappers reject quantities outside 1 through 12 before multiplying.
They do not check remaining seats: a quote describes a price, not availability.

<details>
<summary>Hint and answer</summary>

The implementations return `uint256(10)` and `uint256(15)` respectively.
The generic function returns `Priced.unitPrice(ticket) * quantity`.
The argument's type selects the implementation; the multiplication belongs in the shared function.

</details>

## 2. Inspect a booking

Complete `seatsOf` and `costOf` in `booking.sol`.
For `Booking.Empty`, return zero.
For `Booking.Active(quantity, cost)`, return the appropriate payload field.
Use these helpers through `mySeats()` and `myQuote()` to inspect the caller's entry.
Before any reservation, both readings must be `0`.

<details>
<summary>Hint and answer</summary>

Match `Booking.Active(quantity, _)` to read its seat count and `Booking.Active(_, cost)` to read its quote.
Neither helper changes the booking or contract storage.
Keeping this interpretation in one module avoids repeating enum matching in every public getter.

</details>

## 3. Reserve and cancel

Complete the marked reservation and cancellation bodies in `TicketOffice.sol`.
Before computing a price or writing storage, check that registration is open, the quantity is positive, the caller has no booking, and the quantity fits in `capacity() - reserved`.
Comparing against the remaining capacity avoids first adding an arbitrarily large input.

After validation, store `Booking.Active(quantity, cost)` for the caller and increase `reserved` by the same quantity.
Return the recorded cost.
The supplied standard and premium entry points must both use this shared booking path.

For cancellation, first require open registration, then match the caller's booking.
An active booking supplies both the quantity to subtract and the quote to return.
Replace it with `Booking.Empty`; canceling an empty booking must revert.

After each source edit, start the manual sequence again: compilation changes discard the old execution session.
As Account 1, reserve 3 standard seats and expect a quote of `30`, `mySeats()` of `3`, and `remaining()` of `9`.
Cancel and expect `30`, then check that `mySeats()` is `0` and `remaining()` is `12`.
Cancel again and expect a revert.

<details>
<summary>Hint and answer</summary>

Treat the per-account entry and `reserved` as two representations of the same reservation.
Every successful reservation must update both; every successful cancellation must undo both.
If a caller already has a booking, reject a second reservation rather than overwriting the entry and losing its original quantity.
The checks and updates belong to one call, so a revert rolls back that call's changes.

</details>

## 4. Test the complete workflow

Use **Reset contract**, select **Account 1**, and make the first call as that account so it becomes the organizer.
Keep the same contract session while changing **Caller** for the following sequence.
For `[]`, the function takes no arguments and no **Arguments** field is shown.

| Caller | Function | Arguments | Expected result |
| --- | --- | --- | --- |
| Account 1 | `capacity()` | `[]` | `12` |
| Account 1 | `reserveStandard(uint256)` | `[3]` | `30` |
| Account 2 | `mySeats()` | `[]` | `0` |
| Account 2 | `reservePremium(uint256)` | `[2]` | `30` |
| Account 2 | `remaining()` | `[]` | `7` |
| Account 1 | `mySeats()` | `[]` | `3` |
| Account 1 | `reserveStandard(uint256)` | `[1]` | Revert: duplicate booking |
| Account 2 | `close()` | `[]` | Revert: not the organizer |
| Account 1 | `cancel()` | `[]` | `30` |
| Account 1 | `remaining()` | `[]` | `10` |
| Account 1 | `close()` | `[]` | Success |
| Account 2 | `cancel()` | `[]` | Revert: registration is closed |
| Account 2 | `mySeats()` | `[]` | `2` |
| Account 1 | `reserveStandard(uint256)` | `[1]` | Revert: registration is closed |

Also test quantities of 0, exactly 12, and 13 seats, using **Reset contract** before each case.
After reserving all 12 seats, switch to another caller and verify that reserving 1 more seat reverts.
After each rejection, inspect both `remaining()` and the caller's booking.
Check that canceling a reservation lets another caller use the released capacity.

Add test comments to make the single-account reserve, inspect, cancel, and close sequence repeatable.
Place sends before the expectations that depend on them, as in [Contracts and persistent state](state.md).
Once you add a close send, update the starter's final `isOpen()` expectation from `true` to `false`.
The solution includes such a sequence; use the caller-switching exercise above to check isolation and authorization as well.
Tests and manual calls use separate contract instances.

<details>
<summary>Complete, tested solution</summary>

The solution opens as a separate project, preserving your starter edits.
In **Run**, select **Run tests** and click **Run all tests from start**.
All 18 tests should pass.

<div class="tutorial-exercise" data-title="TicketOffice solution">
<a class="tutorial-example" href="../projects/tutorial-ticket-office.json?tab=run&amp;view=tests">Open the TicketOffice solution in a full tab</a>
</div>

The public contract coordinates the lifecycle and storage updates:

```solidity
{{#include ../examples/tutorial-ticket-office/TicketOffice.sol}}
```

The pricing module separates the two policies from their shared calculation:

```solidity
{{#include ../examples/tutorial-ticket-office/pricing.sol}}
```

The booking module owns the representation and its accessors:

```solidity
{{#include ../examples/tutorial-ticket-office/booking.sol}}
```

</details>

## Change it

Add a `Supporter` ticket type priced at 25 credits per seat and implement `Priced<Supporter>`.
Export `Supporter(*)` from `pricing.sol` and import `Supporter` in `TicketOffice.sol`.
Expose a `quoteSupporter` and `reserveSupporter` pair following the existing entry points.
The generic quote function, booking representation, cancellation, and capacity rule should continue to work unchanged.
Test that 2 supporter seats quote `50`, consume 2 seats, and release those same seats when canceled.

**Check your understanding:** why must closing preserve the booking entries, and why is a quote insufficient evidence that seats have been reserved?

<details>
<summary>Answer</summary>

Closing changes which transitions are allowed; it does not undo successful bookings.
Those entries must remain readable and consistent with `reserved`.
A quote only calculates a price, while a successful reservation validates availability and writes both the caller's booking and the shared seat count.

</details>
