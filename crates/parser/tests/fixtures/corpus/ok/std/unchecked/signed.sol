// std.unchecked.signed: unchecked (wrapping) arithmetic for int8..int256.
// The compiler desugars `+ - * / %` written inside an `unchecked { ... }` block
// to the Unchecked* classes, which this module re-exports from
// std.unchecked.unsigned. Results wrap to the type's width (signextend), so
// MIN / -1 is MIN. As in Solidity, division and modulo by zero still revert
// with Panic(0x12).
import * from std;
import * from std.signed;
import * from std.unchecked.unsigned;
import { sdiv, smod, signextend } from std.opcodes;

export std.unchecked.unsigned.{ UncheckedAdd, UncheckedSub, UncheckedMul, UncheckedDiv, UncheckedMod };

// --- int8 ---
impl UncheckedAdd<int8> {
    function add(x: int8, y: int8) returns (int8) {
        return Typedef.abs(signextend(0, Add.add(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedSub<int8> {
    function sub(x: int8, y: int8) returns (int8) {
        return Typedef.abs(signextend(0, Sub.sub(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedMul<int8> {
    function mul(x: int8, y: int8) returns (int8) {
        return Typedef.abs(signextend(0, Mul.mul(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedDiv<int8> {
    function div(x: int8, y: int8) returns (int8) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(signextend(0, sdiv(Typedef.rep(x), d)));
    }
}

impl UncheckedMod<int8> {
    function mod(x: int8, y: int8) returns (int8) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(smod(Typedef.rep(x), d));
    }
}

// --- int16 ---
impl UncheckedAdd<int16> {
    function add(x: int16, y: int16) returns (int16) {
        return Typedef.abs(signextend(1, Add.add(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedSub<int16> {
    function sub(x: int16, y: int16) returns (int16) {
        return Typedef.abs(signextend(1, Sub.sub(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedMul<int16> {
    function mul(x: int16, y: int16) returns (int16) {
        return Typedef.abs(signextend(1, Mul.mul(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedDiv<int16> {
    function div(x: int16, y: int16) returns (int16) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(signextend(1, sdiv(Typedef.rep(x), d)));
    }
}

impl UncheckedMod<int16> {
    function mod(x: int16, y: int16) returns (int16) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(smod(Typedef.rep(x), d));
    }
}

// --- int32 ---
impl UncheckedAdd<int32> {
    function add(x: int32, y: int32) returns (int32) {
        return Typedef.abs(signextend(3, Add.add(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedSub<int32> {
    function sub(x: int32, y: int32) returns (int32) {
        return Typedef.abs(signextend(3, Sub.sub(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedMul<int32> {
    function mul(x: int32, y: int32) returns (int32) {
        return Typedef.abs(signextend(3, Mul.mul(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedDiv<int32> {
    function div(x: int32, y: int32) returns (int32) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(signextend(3, sdiv(Typedef.rep(x), d)));
    }
}

impl UncheckedMod<int32> {
    function mod(x: int32, y: int32) returns (int32) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(smod(Typedef.rep(x), d));
    }
}

// --- int64 ---
impl UncheckedAdd<int64> {
    function add(x: int64, y: int64) returns (int64) {
        return Typedef.abs(signextend(7, Add.add(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedSub<int64> {
    function sub(x: int64, y: int64) returns (int64) {
        return Typedef.abs(signextend(7, Sub.sub(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedMul<int64> {
    function mul(x: int64, y: int64) returns (int64) {
        return Typedef.abs(signextend(7, Mul.mul(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedDiv<int64> {
    function div(x: int64, y: int64) returns (int64) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(signextend(7, sdiv(Typedef.rep(x), d)));
    }
}

impl UncheckedMod<int64> {
    function mod(x: int64, y: int64) returns (int64) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(smod(Typedef.rep(x), d));
    }
}

// --- int128 ---
impl UncheckedAdd<int128> {
    function add(x: int128, y: int128) returns (int128) {
        return Typedef.abs(signextend(15, Add.add(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedSub<int128> {
    function sub(x: int128, y: int128) returns (int128) {
        return Typedef.abs(signextend(15, Sub.sub(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedMul<int128> {
    function mul(x: int128, y: int128) returns (int128) {
        return Typedef.abs(signextend(15, Mul.mul(Typedef.rep(x), Typedef.rep(y))));
    }
}

impl UncheckedDiv<int128> {
    function div(x: int128, y: int128) returns (int128) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(signextend(15, sdiv(Typedef.rep(x), d)));
    }
}

impl UncheckedMod<int128> {
    function mod(x: int128, y: int128) returns (int128) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(smod(Typedef.rep(x), d));
    }
}

// --- int256 ---
impl UncheckedAdd<int256> {
    function add(x: int256, y: int256) returns (int256) {
        return Typedef.abs(Add.add(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedSub<int256> {
    function sub(x: int256, y: int256) returns (int256) {
        return Typedef.abs(Sub.sub(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMul<int256> {
    function mul(x: int256, y: int256) returns (int256) {
        return Typedef.abs(Mul.mul(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedDiv<int256> {
    function div(x: int256, y: int256) returns (int256) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(sdiv(Typedef.rep(x), d));
    }
}

impl UncheckedMod<int256> {
    function mod(x: int256, y: int256) returns (int256) {
        let d: word = nonZeroDivisor(Typedef.rep(y));
        return Typedef.abs(smod(Typedef.rep(x), d));
    }
}
