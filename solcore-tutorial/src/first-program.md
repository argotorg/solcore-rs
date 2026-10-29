# Your first Solcore program

**Goal:** call a contract and identify its imports, public function, and return value.

<div class="tutorial-exercise" data-title="Hello contract">
<a class="tutorial-example" href="../projects/contract-output.json?tab=run&amp;view=calls">Open Hello contract in a full tab</a>
</div>

```solidity
{{#include ../examples/contract-output/Hello.sol}}
```

## Run it

1. Click **Try here** above to open the editor within this page.
   It uses the **solc** backend by default.
2. In the editor, open the **Run** tab and select **Try calls**.
3. Choose `answer()` and click **Call function**.
   This function takes no arguments, so there is no **Arguments** field to fill in.
4. Find the decoded result: **42**.
   A successful compilation alone is not an execution result.

The first call can take longer while the browser loads the compiler.
This program returns a value rather than printing text to a terminal.
Gas used describes execution work; its exact value can change with the compiler and backend, so it is not the goal of this exercise.

## Read it

`pragma solcore;` comes first in every Solcore source file.
Solidity files also end in `.sol`; the pragma marks this file as Solcore, and the compiler rejects a file without it.

`import * from std;` makes the standard library's exported names available, including `uint256`.
`std.dispatch` supplies the support for calling public contract functions.
Keep both imports in the contract examples that follow.

`contract Hello` groups contract code.
`function answer()` has no arguments.
`public` lets callers invoke this function, and `returns (uint256)` states its result type.
`return uint256(42);` constructs and returns the value.
Statements end with semicolons.

## Change it

Make this program return **7**, then call `answer()` again.
After that, deliberately remove the semicolon after the return statement and inspect the **Problems** tab.
Restore the semicolon before continuing.

<details>
<summary>Hint and answer</summary>

Replace `uint256(42)` with `uint256(7)`.
The decoded result should be `7`.
Removing the semicolon produces a syntax diagnostic; the source location should help you find the mistake.
A failed compilation produces no new successful execution.

</details>

**Check your understanding:** what is the difference between clicking **Compile** and **Call function**?

<details>
<summary>Answer</summary>

**Compile** checks the source and produces the code needed to run it.
**Call function** also executes the selected function and displays its result.

</details>
