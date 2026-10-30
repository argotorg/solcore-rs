pragma solcore;

// Exercises std.signed and std.unsigned semantics. Each function returns the raw
// two's-complement word (via Typedef.rep) so the harness can check exact values.
import * from std;
import * from std.signed;
import * from std.unsigned;
import * from std.dispatch;

// #[scenario("int_ops")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xb8b1f3d0, label = "mulNeg() -> -25 (signed mul, two's complement word)") -> success(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe7)]
// #[raw(0xdf34e8b6, label = "ltSigned() -> 1 (-5 < 5 signed)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0xcbcc13e1, label = "divSigned() -> -6 (sdiv -20/3 truncates toward zero)") -> success(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa)]
// #[raw(0x19a419e0fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8, label = "negate8(-8) -> 8 (int8 param decode + return encode)") -> success(0x0000000000000000000000000000000000000000000000000000000000000008)]
// #[raw(0x11f8b959000000000000000000000000000000000000000000000000000000003b9aca00000000000000000000000000000000000000000000000000000000003b9aca00, label = "addI32(1e9,1e9) -> 2e9 (int32 decode/encode, in range)") -> success(0x0000000000000000000000000000000000000000000000000000000077359400)]
// #[raw(0xd5d88a2ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffc18, label = "setAcc(-1000) -> -1000 (int64 storage field, CanStore)") -> success(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffc18)]
// #[raw(0x4a2ee8d0, label = "getAcc() -> -1000 (reads stored int64)") -> success(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffc18)]
contract IntOps {
    acc: int64;

    constructor() {}

    // int8 param + int8 return: ABIDecode reduces the param, ABIEncode returns it.
    // -8 negated -> 8
    function negate8(x: int8) public returns (int8) {
        return 0 - x;
    }

    // int32 params + return: exercises ABIDecode/ABIEncode of int32.
    // 1000000000 + 1000000000 = 2000000000 (in range, no overflow).
    function addI32(a: int32, b: int32) public returns (int32) {
        return a + b;
    }

    // int64 storage field via CanStore/StorageType.
    function setAcc(v: int64) public returns (int64) {
        acc = v;
        return acc;
    }

    function getAcc() public returns (int64) {
        return acc;
    }

    // signed multiply: (-5) * 5 = -25
    function mulNeg() public returns (uint256) {
        let d: int256 = 5;
        return uint256(Typedef.rep((0 - d) * d));
    }

    // signed compare: -5 < 5 -> true
    function ltSigned() public returns (uint256) {
        let d: int256 = 5;
        let e: int256 = 0 - d;
        if (e < d) {
            return uint256(1);
        } else {
            return uint256(0);
        }
    }

    // signed divide (truncates toward zero): -20 / 3 = -6
    function divSigned() public returns (uint256) {
        let m20: int256 = 0 - 20;
        let three: int256 = 3;
        return uint256(Typedef.rep(m20 / three));
    }
}
