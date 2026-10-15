import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

// Return a *dynamic sum by value* from a dispatched function — the case the
// generic ABI encoder used to get wrong (it wrote only the top-level tag word,
// collapsing the whole value to a single 0x00…0 head). The companion
// `abi_dyn_sum.sol` deliberately avoids this by returning `memory(bytes)` /
// individual words; here we exercise the fixed `sum(f,g):ABIEncode` head-offset
// path head-on.
//
//   D2 : L(uint256) | R(bytes)               -- dynamic (R carries bytes)
//   D3 : X(uint256) | Y(uint256) | Z(bytes)  -- dynamic, 3 constructors
//   S2 : P(uint256) | Q(uint256)             -- static  (control: inline, no offset)
//
// A dynamic sum is referenced by a 32-byte offset and laid out inline in the
// tail as [tag][branch]. The tag is one flat bytes32 keccak256("Name(argSigs)")
// variant tag, so even D3's three constructors each encode as a single tag word
// (no positional inl/inr chain). A static sum stays inline as [tag][branch] with
// no leading offset.
enum D2 { L(uint256), R(memory<bytes>) }
enum D3 { X(uint256), Y(uint256), Z(memory<bytes>) }
enum S2 { P(uint256), Q(uint256) }

// #[scenario("abi_dyn_sum_return")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x7100f9ef000000000000000000000000000000000000000000000000000000000000002a, label = "makeL(42) -> D2.L(42): dynamic sum, offset+inline [keccak(\"L(uint256)\")][value]") -> success(0x00000000000000000000000000000000000000000000000000000000000000209579c5ddf61e49d0bf336001c25c13f5689ee1abeee1a378b6bef308961641ec000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xfe41f86d00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000002beef000000000000000000000000000000000000000000000000000000000000, label = "makeR(0xbeef) -> D2.R(0xbeef): dynamic branch, nested offset to bytes") -> success(0x000000000000000000000000000000000000000000000000000000000000002086c01c8118a42b422dd5ce2f69a3259cf8ea4606b78834ab34f9c573e887ea8100000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000002beef000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x0fbac4920000000000000000000000000000000000000000000000000000000000000005, label = "makeX(5) -> D3.X(5): dynamic sum, [off][keccak(\"X(uint256)\")][value]") -> success(0x0000000000000000000000000000000000000000000000000000000000000020337229d96e709f076d800bbc51bd1a1c2d4bef08e1bc014b6824ac7256f23c430000000000000000000000000000000000000000000000000000000000000005)]
// #[raw(0x81f1a3fc0000000000000000000000000000000000000000000000000000000000000007, label = "makeY(7) -> D3.Y(7): flat keccak tag, [off][keccak(\"Y(uint256)\")][value]") -> success(0x0000000000000000000000000000000000000000000000000000000000000020a63f6e5f3b8cca289dfc8649ede8fbf4eeb9991f44d7bc70c3fb53b128521a580000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x5799515400000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000002beef000000000000000000000000000000000000000000000000000000000000, label = "makeZ(0xbeef) -> D3.Z(0xbeef): flat keccak(\"Z(bytes)\") tag then bytes leaf") -> success(0x0000000000000000000000000000000000000000000000000000000000000020a011a608dd8f3a630cc6857d22ecee9497045e5027b542f24d7b506a447fd3c100000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000002beef000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x3aec6f2e0000000000000000000000000000000000000000000000000000000000000009, label = "makeP(9) -> S2.P(9): static sum, inline [keccak(\"P(uint256)\")][value], no offset") -> success(0x5e1331e0b0cf9596f31b4ecaa4d4d045bc6de1b91d2401dff29221cab1c175b40000000000000000000000000000000000000000000000000000000000000009)]
// #[raw(0xbd82d2590000000000000000000000000000000000000000000000000000000000000008, label = "makeQ(8) -> S2.Q(8): static sum, inline [keccak(\"Q(uint256)\")][value], no offset") -> success(0x5dcad53f88dc9d20c351c1bb58593dde38a673b5308c8751aaeb91edd8c979520000000000000000000000000000000000000000000000000000000000000008)]
contract DynSumRet {
  constructor() {}

  // ── shallow dynamic sum ────────────────────────────────────────────────
  // inl branch (static uint256 payload) of a dynamic sum: still takes the
  // dynamic encode path (offset word + inline [tag][value] in the tail).
  function makeL(n : uint256) public returns (D2) {
    return D2.L(n);
  }

  // inr branch carrying a dynamic bytes payload: [off][1][off][len][data].
  function makeR(b : memory<bytes>) public returns (D2) {
    return D2.R(b);
  }

  // ── deeply right-nested dynamic sum ────────────────────────────────────
  // outer inl: [off][0][value]
  function makeX(n : uint256) public returns (D3) {
    return D3.X(n);
  }

  // inr(inl …): two dynamic-sum levels, so two nested offsets: [off][1][off][0][value]
  function makeY(n : uint256) public returns (D3) {
    return D3.Y(n);
  }

  // inr(inr bytes): nested offsets down to the bytes leaf:
  // [off][1][off][1][off][len][data]
  function makeZ(b : memory<bytes>) public returns (D3) {
    return D3.Z(b);
  }

  // ── static sum control ─────────────────────────────────────────────────
  // Byte-identical to the pre-fix output: inline [tag][value], no offset word.
  function makeP(n : uint256) public returns (S2) {
    return S2.P(n);
  }

  function makeQ(n : uint256) public returns (S2) {
    return S2.Q(n);
  }
}
