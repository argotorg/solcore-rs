# Solcore standard library

The twelve `.sol` files in this directory (including `unchecked/`) are the standard library for the Rust
compiler. Format them with `solcore-fmt`. Parser, frontend, specialization, and
backend tests read these same sources directly.

## Maintenance policy

Maintain the library alongside the Rust compiler. Update and format each
changed source. Cover language and ABI changes with focused frontend,
specialization, and backend regressions.

ABI metadata support alone does not make a type externally dispatchable. Keep
ABI JSON, selector spelling, calldata decoding, and result encoding aligned.

## Integer types and checked arithmetic

`signed.sol` provides `int8` .. `int256` and `unsigned.sol` provides `uint8` ..
`uint128` (`uint256` is in `std.sol`). They are accepted as external ABI types
(`intN`/`uintN`). Arithmetic on them and on `uint256` is checked as in
Solidity 0.8: overflow, underflow and `MIN / -1` revert with `Panic(0x11)`,
division or modulo by zero with `Panic(0x12)`. The per-type instances delegate
to the `uAddChecked` .. `sModChecked` helpers in `std.sol`, whose conditions are
bound to `let` locals rather than nested calls to keep call nesting within the
default parser stack. Regressions: `tests/e2e/int-ops` (ABI decode/encode and
storage), `tests/e2e/checked-arith`, and
the shared matrix in `crates/test-utils/src/e2e/integers.rs` (538 arithmetic,
bitwise, shift, comparison and bounds cases across all 11 added integer types).
Both backends generate its source fixtures at test time and execute them
against independently calculated results.

Integer defaulting selects the canonical `uint256` type. Arithmetic and compound
assignments therefore remain checked without a type annotation. Programs using
this default must import the standard library; an unavailable default type is
an error. Explicit `word` arithmetic retains the low-level EVM wrapping behavior.

Unary `-x` uses checked subtraction from zero. Negating a signed type's minimum
value panics with code `0x11`; negating a nonzero unsigned value also overflows.
Negative literals, including signed minima, use the type's `Int.fromInteger`
conversion. In a required `comptime` evaluation, a failing arithmetic check is
a compile error.

Literals and `Int.fromInteger` conversions for all these integer types are checked
against the type's bounds at compile time, before conversion to a 256-bit
word. `Typedef.abs` remains a raw representation conversion. Explicit `word`
literal conversions retain their modulo-2^256 behavior.

ABI decoding rejects noncanonical upper bits with `DirtyHigherBitsForInteger()`;
encoding masks unsigned values and sign-extends signed values to their declared
width, including raw values constructed through `Typedef.abs`. The shared ABI
matrix in `crates/test-utils/src/e2e/integers/abi.rs` covers all 11 types through
calldata and memory, including tuples, arrays, and malformed input.

## Unchecked arithmetic

`unchecked/unsigned.sol` declares the `UncheckedAdd`, `UncheckedSub`,
`UncheckedMul`, `UncheckedDiv`, and `UncheckedMod` traits, re-exported by
`unchecked/signed.sol`, with wrapping instances for `word`, `uint8` ..
`uint256`, and `int8` .. `int256`. The compiler lowers `+ - * / %`, their
compound assignments, and unary `-` written directly inside `unchecked { ... }`
to these traits, so such a block must import `std.unchecked.unsigned` or
`std.unchecked.signed`. Results wrap to the type's width and `MIN / -1` is
`MIN`; division or modulo by zero still panics with code `0x12`. The scope is
lexical: called functions and lambda bodies inside the block stay checked, and
nested blocks are rejected. Regressions: `tests/e2e/unchecked-scope` and the
unchecked part of the shared matrix in `crates/test-utils/src/e2e/integers.rs`.

## Shift operators

`std.sol` declares `ShiftAmount<n>`, `Shl<t, n>`, and `Shr<t, n>`. The compiler
lowers `<<`, `>>`, `<<=`, and `>>=` to `Shl.bshl` and `Shr.bshr`. Instances for
`word` and `uint256` are in `std.sol`; `unsigned.sol` and `signed.sol` provide
the `uintN` and `intN` instances. As in Solidity, the result has the type of the
shifted value, only unsigned types (`word`, `uint8` .. `uint256`) are shift
amounts, bits shifted past the width are discarded, `>>` is arithmetic for
signed types, and shifts never revert, inside or outside `unchecked`.
Regressions: `tests/e2e/shift-operators` and the shift part of the shared
matrix in `crates/test-utils/src/e2e/integers.rs`.

## Arrays and slices

The array syntax follows Solidity. The meaning of `T[]` depends on where it is
written:

- in a contract field it is a storage array `array<T>`;
- in a local, a parameter or a return type it is a memory array
  `memory<DynArray<T>>`;
- inside `storage<...>`, `mapping(...)` or `calldata<...>` it is `array<T>`;
- inside `memory<...>` it is `DynArray<T>`.

Fixed-size arrays `T[k]` are rejected.

`new T[](n)` allocates `n` zeroed elements with `newArray`. For
`new T[][](n)`, `newNestedArray` points every element at the empty array at
`0x60`.

Element access:

- `a[i]` and `a[i] = v` on memory arrays go through the `RValueIdxAccess`,
  `LValueIdxAccess` and `CanStore` instances.
- Any other collection with an `RValueIdxAccess` instance can also be indexed.
- `a.length` reads `Length.length(a)` unless the value has a `length` field.
- `push` and `pop` exist only for storage arrays; memory arrays have a fixed
  length.

Slices:

- `a[s:e]`, `a[s:]`, `a[:e]` and `a[:]` call `sliceRange`, `sliceFrom`,
  `sliceTo` and `sliceAll`.
- Each returns a read-only `arraySlice<B>` view over any base `B` with `Length`
  and `RValueIdxAccess` instances: memory, storage and calldata arrays, or
  another slice.
- `toArray(s)` copies a slice into a fresh memory array.

Memory arrays of word-sized elements may appear in public signatures, where
they sign as `T[]`.

Errors use Solidity's panic codes:

| Code | Cause |
|---|---|
| `0x32` | An index or slice bound is out of range. |
| `0x31` | `pop` on an empty storage array. |
| `0x41` | An allocation or storage array is too large. |

Regressions:
`tests/e2e/{array-syntax,mem-array,array-slice,calldata-slice,memory-array-abi,user-collection-index,allocator-bounds,memory-allocation-bounds}`.

## Verification

After every std update, verify at least:

```sh
cargo test -p solcore-parser -p solcore-hir-ty -p solcore-specialize --locked
E2E=1 E2E_REQUIRED=1 cargo test --profile e2e \
  -p solcore-yul --test e2e --locked -- \
  --nocapture --test-threads=1
```

Raw-vector cases retain their calldata, output, and EVM metadata; fixtures may
append regressions. Both Yul and Sonatina compile
and execute the complete set against Osaka. See
[`tests/e2e/README.md`](../tests/e2e/README.md) for the full validation
commands.
