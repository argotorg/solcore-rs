import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;
pragma no-patterson-condition ;
pragma no-coverage-condition ;
pragma no-bounded-variable-condition ;

// Direct tests for `abi_encode` over user-defined algebraic data types (ADTs).
//
// An ADT reaches `abi_encode` through its auto-derived `Generic` representation
// and the ABIGeneric bridges (std/ABIGeneric.sol): a product constructor
// represents as the primitive tuple of its fields, and a sum represents as the
// binary `sum(f, g)` type (inl = first constructor, inr = second). Each method
// encodes an ADT value and returns the `memory(bytes)` result, which the
// dispatcher then ABI-encodes as `bytes`, so the return data is
//   [0x20 offset][length][ abi_encode output, padded ]
// and the inner payload is EXACTLY what `abi_encode` produced for the ADT.
//
// The data types are left to auto-derive their instances: this gives them not
// only a `Generic` instance but a CONCRETE `ABIAttribs` instance reporting the
// representation's real head size. That concrete instance is what makes
// `abi_encode` correct here — a manual `Generic` instance (which requires
// `pragma no-generic-instance-for`) would suppress the derived `ABIAttribs`, so
// `headSize` would fall back to the catch-all `default instance t:ABIAttribs`
// (32 bytes) and truncate the encoding to its first word. See DeriveGeneric.hs.
//
// The sum discriminant is one bytes32 keccak256("Ctor(argSigs)") variant tag
// (the ABI wire form), not a positional 0/1 index.
//
// Layouts pinned here:
//   * static product  (uint256, uint256)      -> two head words   (len 0x40)
//   * static sum       Choice                  -> [tag][branch]     (len 0x40)
//   * dynamic sum      StrBox                   -> a dynamic value: the head slot
//        holds an offset (0x20) to the sum body [tag][branch...] laid out in the
//        tail — even the static (Empty) branch keeps that offset wrapper.

// static product
enum Point { Point(uint256, uint256) }

// static sum
enum Choice { First(uint256), Second(uint256) }

// dynamic sum (the Text branch carries a dynamic string)
enum StrBox { Empty(uint256), Text(memory<string>) }

// #[scenario("abi_encode_adt")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xf540ea6a00000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000004, label = "encPoint(3,4) -> abi_encode(Point) static product, len 0x40") -> success(0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000004)]
// #[raw(0xfcfe4c2a0000000000000000000000000000000000000000000000000000000000000005, label = "encFirst(5) -> abi_encode(Choice.First) static sum keccak(\"First(uint256)\") tag, len 0x40") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000400b6b8ec01e9ca27b1c98dfafa117db975f33702e6972f3ea96d21bdbd4c2d8bd0000000000000000000000000000000000000000000000000000000000000005)]
// #[raw(0xf59a35db0000000000000000000000000000000000000000000000000000000000000006, label = "encSecond(6) -> abi_encode(Choice.Second) static sum keccak(\"Second(uint256)\") tag, len 0x40") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000403c23b65b9c7c76e50799666ccdaac3876fb62ace725b91b19545919d5966bf770000000000000000000000000000000000000000000000000000000000000006)]
// #[raw(0x94fb55dc0000000000000000000000000000000000000000000000000000000000000009, label = "encEmpty(9) -> abi_encode(StrBox.Empty) dynamic sum static branch, keccak(\"Empty(uint256)\") tag, len 0x60") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000020f2c6b74a4e42434bf406d1452c6d967321daaad79270822b1e44a42b0aa6aa1f0000000000000000000000000000000000000000000000000000000000000009)]
// #[raw(0xfc30865a, label = "encText() -> abi_encode(StrBox.Text \"abc\") dynamic sum dynamic branch, keccak(\"Text(string)\") tag, len 0xa0") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000205ac0919f1b41505333556b99846ff7ae803c70a805340929c8325e9da81f1c2d000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000036162630000000000000000000000000000000000000000000000000000000000)]
contract AbiEncodeAdt {
  constructor() {}

  // Static product: encodes as the tuple (a, b) — two inline head words.
  function encPoint(a : uint256, b : uint256) public returns (memory<bytes>) {
    return abi_encode(Point(a, b));
  }

  // Static sum, left constructor: [keccak("First(uint256)")][x].
  function encFirst(x : uint256) public returns (memory<bytes>) {
    return abi_encode(Choice.First(x));
  }

  // Static sum, right constructor: [keccak("Second(uint256)")][x].
  function encSecond(x : uint256) public returns (memory<bytes>) {
    return abi_encode(Choice.Second(x));
  }

  // Dynamic sum, static branch: still offset-wrapped — [0x20] -> [keccak("Empty(uint256)")][n].
  function encEmpty(n : uint256) public returns (memory<bytes>) {
    return abi_encode(StrBox.Empty(n));
  }

  // Dynamic sum, dynamic branch: [0x20] -> [keccak("Text(string)")][branch offset][len][data].
  function encText() public returns (memory<bytes>) {
    let raw : string = "abc";
    let s : memory<string> = Str.fromString(raw);
    return abi_encode(StrBox.Text(s));
  }
}
