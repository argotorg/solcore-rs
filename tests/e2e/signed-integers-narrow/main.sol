// std.signed: arithmetic, bitwise, comparison and bounds per width.
// Parameters and results are raw two's-complement words: each function
// rebuilds the intN value with Typedef.abs and returns Typedef.rep of the
// result. Negative operands are therefore passed sign-extended. Operations
// wrap at the type's width (signextend), including MIN / -1 and MIN % -1.
// cmp* returns a bitmask: lt=1, gt=2, le=4, ge=8, eq=16, ne=32.
import * from std;
import * from std.signed;
import * from std.dispatch;

contract SignedIntegers {
  // ---- int8 ----
  // #[(1, 2) -> 3]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb, 3) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(127, 1) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 127]
  function addI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x + y));
  }
  // #[(3, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb) -> 2]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 1) -> 127]
  // #[(127, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
  function subI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x - y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 6) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd6]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa) -> 42]
  // #[(127, 2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
  function mulI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x * y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
  // #[(7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 4]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
  function divI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x / y));
  }
  // #[(7, 3) -> 1]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 3) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  // #[(7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 1]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0]
  function modI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x % y));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 15) -> 15]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0, 12) -> 0]
  function andI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x & y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff5]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 127) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  function orI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x | y));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 127]
  function xorI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x ^ y));
  }
  // #[(0) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0]
  // #[(127) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
  function notI8(a: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    return uint256(Typedef.rep(~x));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0) -> 37]
  // #[(0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 42]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 28]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 127) -> 37]
  function cmpI8(a: uint256, b: uint256) public returns (uint256) {
    let x: int8 = Typedef.abs(Typedef.rep(a));
    let y: int8 = Typedef.abs(Typedef.rep(b));
    let r: uint256 = uint256(0);
    if (x < y) { r = r + uint256(1); }
    if (x > y) { r = r + uint256(2); }
    if (x <= y) { r = r + uint256(4); }
    if (x >= y) { r = r + uint256(8); }
    if (x == y) { r = r + uint256(16); }
    if (x != y) { r = r + uint256(32); }
    return r;
  }
  // #[() -> (0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80, 127)]
  function boundsI8() public returns (uint256, uint256) {
    let lo: int8 = Bounded.minVal();
    let hi: int8 = Bounded.maxVal();
    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));
  }

  // ---- int16 ----
  // #[(1, 2) -> 3]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb, 3) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(32767, 1) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 32767]
  function addI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x + y));
  }
  // #[(3, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb) -> 2]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 1) -> 32767]
  // #[(32767, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000]
  function subI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x - y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 6) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd6]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa) -> 42]
  // #[(32767, 2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000]
  function mulI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x * y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
  // #[(7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 4]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000]
  function divI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x / y));
  }
  // #[(7, 3) -> 1]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 3) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  // #[(7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 1]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0]
  function modI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x % y));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 15) -> 15]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0, 12) -> 0]
  function andI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x & y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff5]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 32767) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  function orI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x | y));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 32767]
  function xorI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x ^ y));
  }
  // #[(0) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0]
  // #[(32767) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000]
  function notI16(a: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    return uint256(Typedef.rep(~x));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0) -> 37]
  // #[(0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 42]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 28]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 32767) -> 37]
  function cmpI16(a: uint256, b: uint256) public returns (uint256) {
    let x: int16 = Typedef.abs(Typedef.rep(a));
    let y: int16 = Typedef.abs(Typedef.rep(b));
    let r: uint256 = uint256(0);
    if (x < y) { r = r + uint256(1); }
    if (x > y) { r = r + uint256(2); }
    if (x <= y) { r = r + uint256(4); }
    if (x >= y) { r = r + uint256(8); }
    if (x == y) { r = r + uint256(16); }
    if (x != y) { r = r + uint256(32); }
    return r;
  }
  // #[() -> (0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000, 32767)]
  function boundsI16() public returns (uint256, uint256) {
    let lo: int16 = Bounded.minVal();
    let hi: int16 = Bounded.maxVal();
    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));
  }

  // ---- int32 ----
  // #[(1, 2) -> 3]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb, 3) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(2147483647, 1) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 2147483647]
  function addI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x + y));
  }
  // #[(3, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb) -> 2]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 1) -> 2147483647]
  // #[(2147483647, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000]
  function subI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x - y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 6) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd6]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa) -> 42]
  // #[(2147483647, 2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000]
  function mulI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x * y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
  // #[(7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 4]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000]
  function divI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x / y));
  }
  // #[(7, 3) -> 1]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 3) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  // #[(7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 1]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0]
  function modI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x % y));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 15) -> 15]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0, 12) -> 0]
  function andI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x & y));
  }
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff5]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 2147483647) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  function orI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x | y));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 5) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 2147483647]
  function xorI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x ^ y));
  }
  // #[(0) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0]
  // #[(2147483647) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000]
  function notI32(a: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    return uint256(Typedef.rep(~x));
  }
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0) -> 37]
  // #[(0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 42]
  // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 28]
  // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 2147483647) -> 37]
  function cmpI32(a: uint256, b: uint256) public returns (uint256) {
    let x: int32 = Typedef.abs(Typedef.rep(a));
    let y: int32 = Typedef.abs(Typedef.rep(b));
    let r: uint256 = uint256(0);
    if (x < y) { r = r + uint256(1); }
    if (x > y) { r = r + uint256(2); }
    if (x <= y) { r = r + uint256(4); }
    if (x >= y) { r = r + uint256(8); }
    if (x == y) { r = r + uint256(16); }
    if (x != y) { r = r + uint256(32); }
    return r;
  }
  // #[() -> (0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000, 2147483647)]
  function boundsI32() public returns (uint256, uint256) {
    let lo: int32 = Bounded.minVal();
    let hi: int32 = Bounded.maxVal();
    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));
  }
}
