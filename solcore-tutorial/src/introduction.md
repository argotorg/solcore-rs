# Welcome

Learn to write a small Solcore contract, run it in your browser, and explain why it behaves as it does.
You will start with a function returning `42`, then work through values, tests, persistent state, enums, modules, traits, generics, and compile-time computation.
Later exercises revisit the counter, then combine these features in an event booking contract with reservations, cancellation, and access control.

## Who this is for

You should already be comfortable with variables, functions, and reading short programs in some language.
You do not need previous smart-contract experience, a wallet, or a local blockchain.
We introduce the contract concepts as they become necessary.
Allow about two to three hours for a first pass through the individual features, with extra time for the event booking project; each chapter can also be studied separately.

Solcore is an experimental Core Solidity compiler and a prototype implementation of Solidity's new type system.
It produces code for the Ethereum Virtual Machine (EVM), the runtime that executes Ethereum contracts.
This tutorial follows the **current `solcore-rs` implementation and its checked-in standard library**.

<aside class="tutorial-important" aria-label="Important">
<p><strong>Important</strong></p>
<p>Solcore and these examples are works in progress, not production-ready contracts.
The playground executes against an in-memory EVM; the exercises do not send real transactions or require real funds.</p>
</aside>

## Read → predict → run → change

Every chapter gives you a goal, an example, an expected result, and a small change to try.
Predict the result before running it.
The expandable hints let you get unstuck without reading the solution first.
At the end of a chapter, explain the result in your own words before continuing.

The lessons introduce the editor controls as you need them.
The [Using the editor](using-the-editor.md) appendix is a reference for running code, saving experiments, and troubleshooting.

[Start with your first program →](first-program.md)
