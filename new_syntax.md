# Proposal: Solidity-Style Syntax for Solcore

## Overview

The new Solcore syntax should be as close as practical to the current Solidity
0.8 family of syntax. Solidity familiarity is the default, not an absolute
compatibility requirement: Solcore should deliberately diverge where Solidity
syntax restricts the type system, complicates parsing, or preserves sugar that
Core does not intend to support.

The goal is not to preserve compatibility with the old Solcore syntax. The goal
is to make Solcore code recognizable to Solidity developers while giving
Solcore's type system a regular, parser-friendly surface. Old Solcore spelling
should not remain part of the new canonical syntax merely for compatibility.

This proposal describes surface syntax. It does not specify implementation
details, internal representation, migration tooling, or the semantic relation
that determines which explicit type conversions are valid.

## Basic Principles

- When Solidity syntax fits Solcore without constraining the language, use it.
- Use name-first declarations so that every type position accepts the complete
  type grammar without parser feedback from name or type resolution.
- Do not include old Solcore syntax in the new canonical syntax solely for
  compatibility.
- Introduce Solcore-specific features as regular extensions of the surrounding
  grammar.
- Avoid syntax that looks like Solidity but means something different.
- Deliberately omitted Solidity sugar is out of scope, not deferred for a later
  compatibility phase.

## File Extension

Both Classic Solidity and Core Solidity source files use the `.sol` extension.
A prototype may temporarily use `.solc` for technical reasons, but `.solc` is
not part of the language specification.

## Pragma

Solidity pragmas are accepted.

```solidity
pragma solidity ^0.8.23;
pragma abicoder v2;
```

Solcore-specific pragmas use the `pragma solcore ...` namespace.

```solidity
pragma solcore noCoverageCondition;
pragma solcore noPattersonCondition;
pragma solcore noBoundVariableCondition;
pragma solcore noGenericInstanceFor MyType;
```

> How are we going to tell the file uses Core syntax?
> `pragma solidity 1.0`?
> or `pragma solcore version 0.1` ?
[name=marcin]

## Modules

Core imports use dotted module names rather than string paths. Selective imports
follow Classic Solidity's `import {f, g} from ...` ordering.

```solidity
import std;
import std.dispatch;
import * as dispatch from std.dispatch;
import {address, uint256 as U256} from std;
import {foo, bar as baz} from @ext.foo.bar;
```

In particular, Core does not use either of these forms:

```solidity
import "M/N.sol";
import M.N.{f, g};
```

During transition, Classic Solidity supports both its existing string imports
and dotted module imports. Core supports only the dotted form. The namespace
alias form remains available in Classic; Core uses the same spelling when a
namespace alias is needed.

The source module `M.N` maps to a `.sol` source file. Exact package and module
resolution rules are separate from this syntax proposal.

### Export

This proposal does not select an `export` or re-export syntax. The March 2026
module discussion left export undecided pending a clearer interoperability
model between Classic and Core. A module's public-interface and re-export rules
must therefore be specified together with that model rather than inherited from
the old Solcore syntax.

## Contract

Contracts, interfaces, and libraries keep Solidity-style declaration shells.
Named parameters and fields use the name-first declaration syntax described
below.

```solidity
contract Token {
  balances: mapping(address => uint256);

  constructor(initialSupply: uint256) payable {
    balances[msg.sender] = initialSupply;
  }

  function balanceOf(account: address) public returns (uint256) {
    return balances[account];
  }

  fallback() external payable {
    // Handle unmatched selectors, including empty calldata if desired.
  }
}
```

`constructor` and the single general `fallback` entry point are part of the
initial Core surface. `receive` is deliberately omitted; a contract that needs
special behavior for empty calldata implements it explicitly in `fallback`.

Built-in function attributes such as `public`, `external`, `internal`,
`private`, `pure`, `view`, and `payable` keep their Solidity spelling. This does
not imply support for user-defined modifier declarations.

## Function

Functions use Solidity's `function` and `returns` structure, with name-first
parameter declarations.

```solidity
function f(x: uint256) public returns (uint256) {
  return x + 1;
}

function pair() returns (uint256, uint256) { ... }
function namedResult() returns (result: uint256) { ... }
function nop() { ... }
```

The old `public function f(x : T) -> U` form is not used.
> why not ?

## Variable Declarations

Every named value binding places the name or binding pattern before its type.
The colon introduces a type; a declaration never starts by guessing whether an
identifier denotes a type.

Fields and parameters are already in declaration contexts:

```solidity
amount: uint256;
owner: address;
name: string memory;
balances: mapping(address => uint256);

function transfer(to: address, amount: uint256) { ... }
```

Local declarations use `let`, with or without an explicit type.

```solidity
let amount: uint256 = readAmount();
let owner: address;
let inferred = computeValue();
```

The same rule applies to destructuring. The type describes the complete binding
pattern rather than being distributed among its elements.

```solidity
let (amount, ok): (uint256, bool) = readResult();
let (left, right) = pair();
```

This is an intentional incompatibility with Classic Solidity's prefix-type
declarations. It avoids single-word-type restrictions, makes tuple bindings
regular, and lets the parser recognize declarations without semantic
information. The spelling may resemble an old Solcore declaration, but it is
chosen for these grammar properties rather than for source compatibility.

## Types

Types that already exist in Solidity retain their type syntax. Because a type
appears after `:` in a named declaration, it may contain any number of tokens.

```solidity
uint256
address
mapping(address => uint256)
uint256[]
uint256[4]
(uint256, bool)
function(uint256) internal returns (bool)
```

Qualified and generic Solcore types use dotted names and angle brackets.

```solidity
Option<uint256>
Result<uint256, Error>
collections.Map<address, Option<uint256>>
```

User-defined value types use Solidity syntax.

```solidity
type Wad is uint256;
```

If transparent aliases are needed, a separate alias syntax can be considered.

```solidity
alias Word = uint256;
```

## Type Conversions and Type Constraints

Explicit type conversion uses one canonical form:

```solidity
expression as Type
```

The target of `as` is the complete type grammar, not a single identifier or a
special parser production. These examples illustrate the grammar; the type
checker still decides whether each particular source-to-target conversion is
defined.

```solidity
let n = raw as uint256;
let pairValue = value as (uint256, bool);
let callback = candidate as function(uint256) internal returns (bool);
let result = value as pkg.Result<uint256, Error>;
```

`as` is left-associative, so `x as T as U` converts first to `T` and then to
`U`. A conversion does not bypass type safety: if the language defines no
conversion between the source and target types, analysis rejects it.

Function-style conversion syntax such as `T(expression)` is not part of Core.
That form would require the parser to know whether `T` is a type, and it does
not extend naturally to arbitrary multi-token types. Parenthesized call syntax
therefore always remains an ordinary call or constructor expression.

`as` means conversion, not type annotation. When an expression only needs an
expected type to guide inference, use a typed binding:

```solidity
let value: T = expression;
```

There is no separate general-purpose `expression : T` annotation syntax in
this proposal. This resolves the previous conversion/annotation open question
without overloading `:` outside binding positions.

## Struct and Enum

Structs and ordinary enums keep Solidity-style declaration structure, with
name-first fields.

```solidity
struct Pair {
  x: uint256;
  y: uint256;
}

enum Status {
  Pending,
  Filled,
  Cancelled
}
```

Solcore algebraic data types are represented as enums that can carry payloads.

```solidity
enum Option<T> {
  None,
  Some(T)
}

enum Result<T, E> {
  Ok(T),
  Err(E)
}
```

Values are constructed with qualified names.

```solidity
Option.Some(1)
Option.None
```

## Trait and Impl

The old `class` / `instance` syntax is replaced with `trait` / `impl`.

```solidity
trait Eq<T> {
  function eq(x: T, y: T) returns (bool);
}

impl Eq<uint256> {
  function eq(x: uint256, y: uint256) returns (bool) {
    return x == y;
  }
}
```

Constraints are written with `where`.

```solidity
impl<T> Eq<Option<T>> where T: Eq {
  function eq(x: Option<T>, y: Option<T>) returns (bool) {
    return true;
  }
}
```

## Generics

The old `forall` syntax is not used. Type parameters are written after the name
with angle brackets.

```solidity
function id<T>(x: T) returns (T) {
  return x;
}

function eqSelf<T>(x: T) returns (bool) where T: Eq {
  return Eq.eq(x, x);
}
```
> this is at odds with the explicit declaration rume we have adopted some time ago; we should discuss this
[name=marcin]

## Comptime

`comptime` modifies a binding and therefore appears immediately before the
binding pattern. Its position is consistent with name-first declarations.

```solidity
function pow(comptime n: uint256, x: uint256) returns (uint256) {
  return x ** n;
}
```
> perhaps add an example of `let comptime` ... or shoud we use `const` for that?

## Pattern Matching

Pattern matching uses a block form with `match`, `case`, and `default`.

```solidity
match (value) {
  case Option.Some(x) {
    return x;
  }
  case Option.None {
    return 0;
  }
}
```

When matching multiple values at once, use tuple-like syntax.

```solidity
match (x, y) {
  case (Option.Some(a), Option.Some(b)) {
    return a + b;
  }
  default {
    return 0;
  }
}
```
> we should perhaps discuss pattern matching in expressions
[name=marcin]

## Statements

Statements follow Solidity where that does not conflict with name-first local
declarations.

```solidity
return;
return x;
if (cond) { ... } else { ... }
for (let i: uint256 = 0; i < n; i = i + 1) { ... }
while (cond) { ... }
break;
continue;
unchecked { ... }
assembly { ... }
revert;
```

Reverting with encoded data is provided by explicit low-level or
standard-library operations. Core does not add custom-error declaration or
`revert ErrorName(...)` sugar.

## Expressions

Operators, ordinary calls, field access, index access, and explicit conversion
prefer familiar Solidity spelling.

```solidity
x + y
x == y
!ok
x & y
x << y
cond ? a : b
balances[account]
token.balanceOf(account)
expression as T
```

Solidity call-option syntax such as `f{value: v, gas: g}(arg)` is not part of
Core. Calls that need explicit value, gas, or creation salt use explicit
standard-library call operations.

## Deliberately Omitted Solidity Sugar
> This we should defnitely discuss. We do not have error/event declarations yet, but perhaps we s
> hould in the future.

Core will not support the following constructs, either in the initial release
or as features deferred for a later migration phase:

- contract inheritance, including `is`, base-constructor calls, and `super`;
- user-defined modifier declarations and applications;
- event declarations and `emit` statements;
- custom error declarations and custom-error revert sugar;
- the separate `receive` entry point; and
- Solidity call-option syntax.

The corresponding Core mechanisms are composition and traits, ordinary helper
functions and explicit control flow, low-level logs or standard-library event
helpers, explicit revert-data helpers, the general `fallback` entry point, and
explicit call APIs.

This is a scope decision rather than an implementation schedule. Adding any of
these constructs later would require a new proposal with a reason other than
initial Solidity migration convenience.

For example, EVM logs can be exposed through `log0` through `log4` and
higher-level standard-library helpers.

```solidity
import {log1} from std.opcodes;

log1(offset, size, topic);
```

## Rewrite Examples

Old Solcore:

```solidity
public function balanceOf(account : address) -> uint256 {
  return balances[account];
}
```

New Solcore:

```solidity
function balanceOf(account: address) public returns (uint256) {
  return balances[account];
}
```

Classic Solidity declaration:

```solidity
mapping(address => uint256) balances;
```

New Solcore declaration:

```solidity
balances: mapping(address => uint256);
```

Previous proposed selective import:

```solidity
import std.{address, uint256 as U256};
```

Revised selective import:

```solidity
import {address, uint256 as U256} from std;
```

Classic Solidity conversion:

```solidity
uint256(value)
```

New Solcore conversion:

```solidity
value as uint256
```

The same conversion production also accepts complex target types without
special cases:

```solidity
value as pkg.Result<uint256, Error>
```
