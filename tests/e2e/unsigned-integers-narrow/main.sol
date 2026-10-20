// std.unsigned: arithmetic, bitwise, comparison and bounds per width.
// Parameters and results are raw words: each function rebuilds the uintN
// value with Typedef.abs and returns Typedef.rep of the result. Operations
// wrap modulo 2^N (results are masked to the type's width).
// cmp* returns a bitmask: lt=1, gt=2, le=4, ge=8, eq=16, ne=32.
import * from std;
import * from std.unsigned;
import * from std.dispatch;

contract UnsignedIntegers {
  // ---- uint8 ----
  // #[(1, 2) -> 3]
  // #[(255, 1) -> 0]
  // #[(255, 255) -> 254]
  function addU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x + y));
  }
  // #[(5, 3) -> 2]
  // #[(0, 1) -> 255]
  // #[(3, 5) -> 254]
  function subU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x - y));
  }
  // #[(6, 7) -> 42]
  // #[(255, 2) -> 254]
  // #[(16, 16) -> 0]
  function mulU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x * y));
  }
  // #[(7, 2) -> 3]
  // #[(255, 1) -> 255]
  // #[(255, 255) -> 1]
  function divU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x / y));
  }
  // #[(7, 3) -> 1]
  // #[(255, 10) -> 5]
  // #[(3, 7) -> 3]
  function modU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x % y));
  }
  // #[(255, 15) -> 15]
  // #[(240, 60) -> 48]
  function andU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x & y));
  }
  // #[(240, 15) -> 255]
  // #[(254, 1) -> 255]
  function orU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x | y));
  }
  // #[(255, 1) -> 254]
  // #[(255, 15) -> 240]
  function xorU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x ^ y));
  }
  // #[(0) -> 255]
  // #[(255) -> 0]
  // #[(15) -> 240]
  function notU8(a: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    return uint256(Typedef.rep(~x));
  }
  // #[(1, 2) -> 37]
  // #[(2, 1) -> 42]
  // #[(255, 255) -> 28]
  // #[(0, 255) -> 37]
  function cmpU8(a: uint256, b: uint256) public returns (uint256) {
    let x: uint8 = Typedef.abs(Typedef.rep(a));
    let y: uint8 = Typedef.abs(Typedef.rep(b));
    let r: uint256 = uint256(0);
    if (x < y) { r = r + uint256(1); }
    if (x > y) { r = r + uint256(2); }
    if (x <= y) { r = r + uint256(4); }
    if (x >= y) { r = r + uint256(8); }
    if (x == y) { r = r + uint256(16); }
    if (x != y) { r = r + uint256(32); }
    return r;
  }
  // #[() -> (0, 255)]
  function boundsU8() public returns (uint256, uint256) {
    let lo: uint8 = Bounded.minVal();
    let hi: uint8 = Bounded.maxVal();
    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));
  }

  // ---- uint16 ----
  // #[(1, 2) -> 3]
  // #[(65535, 1) -> 0]
  // #[(65535, 65535) -> 65534]
  function addU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x + y));
  }
  // #[(5, 3) -> 2]
  // #[(0, 1) -> 65535]
  // #[(3, 5) -> 65534]
  function subU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x - y));
  }
  // #[(6, 7) -> 42]
  // #[(65535, 2) -> 65534]
  // #[(256, 256) -> 0]
  function mulU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x * y));
  }
  // #[(7, 2) -> 3]
  // #[(65535, 1) -> 65535]
  // #[(65535, 65535) -> 1]
  function divU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x / y));
  }
  // #[(7, 3) -> 1]
  // #[(65535, 10) -> 5]
  // #[(3, 7) -> 3]
  function modU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x % y));
  }
  // #[(65535, 15) -> 15]
  // #[(240, 60) -> 48]
  function andU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x & y));
  }
  // #[(240, 15) -> 255]
  // #[(65534, 1) -> 65535]
  function orU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x | y));
  }
  // #[(65535, 1) -> 65534]
  // #[(255, 15) -> 240]
  function xorU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x ^ y));
  }
  // #[(0) -> 65535]
  // #[(65535) -> 0]
  // #[(15) -> 65520]
  function notU16(a: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    return uint256(Typedef.rep(~x));
  }
  // #[(1, 2) -> 37]
  // #[(2, 1) -> 42]
  // #[(65535, 65535) -> 28]
  // #[(0, 65535) -> 37]
  function cmpU16(a: uint256, b: uint256) public returns (uint256) {
    let x: uint16 = Typedef.abs(Typedef.rep(a));
    let y: uint16 = Typedef.abs(Typedef.rep(b));
    let r: uint256 = uint256(0);
    if (x < y) { r = r + uint256(1); }
    if (x > y) { r = r + uint256(2); }
    if (x <= y) { r = r + uint256(4); }
    if (x >= y) { r = r + uint256(8); }
    if (x == y) { r = r + uint256(16); }
    if (x != y) { r = r + uint256(32); }
    return r;
  }
  // #[() -> (0, 65535)]
  function boundsU16() public returns (uint256, uint256) {
    let lo: uint16 = Bounded.minVal();
    let hi: uint16 = Bounded.maxVal();
    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));
  }

  // ---- uint32 ----
  // #[(1, 2) -> 3]
  // #[(4294967295, 1) -> 0]
  // #[(4294967295, 4294967295) -> 4294967294]
  function addU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x + y));
  }
  // #[(5, 3) -> 2]
  // #[(0, 1) -> 4294967295]
  // #[(3, 5) -> 4294967294]
  function subU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x - y));
  }
  // #[(6, 7) -> 42]
  // #[(4294967295, 2) -> 4294967294]
  // #[(65536, 65536) -> 0]
  function mulU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x * y));
  }
  // #[(7, 2) -> 3]
  // #[(4294967295, 1) -> 4294967295]
  // #[(4294967295, 4294967295) -> 1]
  function divU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x / y));
  }
  // #[(7, 3) -> 1]
  // #[(4294967295, 10) -> 5]
  // #[(3, 7) -> 3]
  function modU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x % y));
  }
  // #[(4294967295, 15) -> 15]
  // #[(240, 60) -> 48]
  function andU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x & y));
  }
  // #[(240, 15) -> 255]
  // #[(4294967294, 1) -> 4294967295]
  function orU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x | y));
  }
  // #[(4294967295, 1) -> 4294967294]
  // #[(255, 15) -> 240]
  function xorU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    return uint256(Typedef.rep(x ^ y));
  }
  // #[(0) -> 4294967295]
  // #[(4294967295) -> 0]
  // #[(15) -> 4294967280]
  function notU32(a: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    return uint256(Typedef.rep(~x));
  }
  // #[(1, 2) -> 37]
  // #[(2, 1) -> 42]
  // #[(4294967295, 4294967295) -> 28]
  // #[(0, 4294967295) -> 37]
  function cmpU32(a: uint256, b: uint256) public returns (uint256) {
    let x: uint32 = Typedef.abs(Typedef.rep(a));
    let y: uint32 = Typedef.abs(Typedef.rep(b));
    let r: uint256 = uint256(0);
    if (x < y) { r = r + uint256(1); }
    if (x > y) { r = r + uint256(2); }
    if (x <= y) { r = r + uint256(4); }
    if (x >= y) { r = r + uint256(8); }
    if (x == y) { r = r + uint256(16); }
    if (x != y) { r = r + uint256(32); }
    return r;
  }
  // #[() -> (0, 4294967295)]
  function boundsU32() public returns (uint256, uint256) {
    let lo: uint32 = Bounded.minVal();
    let hi: uint32 = Bounded.maxVal();
    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));
  }
}
