pragma solcore;

// std.unchecked.unsigned: unchecked (wrapping) arithmetic for word and
// uint8..uint256.
// The compiler desugars `+ - * / %` written inside an `unchecked { ... }` block
// to the UncheckedAdd/UncheckedSub/UncheckedMul/UncheckedDiv/UncheckedMod
// classes declared here, so a block using them must import this module or
// std.unchecked.signed. Results wrap modulo 2^N. As in Solidity, division and
// modulo by zero still revert with Panic(0x12).
import * from std;
import * from std.unsigned;
import { iszero } from std.opcodes;

export { UncheckedAdd, UncheckedSub, UncheckedMul, UncheckedDiv, UncheckedMod, nonZeroDivisor };

trait UncheckedAdd<t> {
    function add(l: t, r: t) returns (t);
}

trait UncheckedSub<t> {
    function sub(l: t, r: t) returns (t);
}

trait UncheckedMul<t> {
    function mul(l: t, r: t) returns (t);
}

trait UncheckedDiv<t> {
    function div(l: t, r: t) returns (t);
}

trait UncheckedMod<t> {
    function mod(l: t, r: t) returns (t);
}

// Reverts with Panic(0x12) when `b` is zero; otherwise returns it.
function nonZeroDivisor(b: word) returns (word) {
    if (tobool(iszero(b))) {
        panic(0x12);
    }
    return b;
}

// --- word ---
impl UncheckedAdd<word> {
    function add(x: word, y: word) returns (word) {
        return Add.add(x, y);
    }
}

impl UncheckedSub<word> {
    function sub(x: word, y: word) returns (word) {
        return Sub.sub(x, y);
    }
}

impl UncheckedMul<word> {
    function mul(x: word, y: word) returns (word) {
        return Mul.mul(x, y);
    }
}

impl UncheckedDiv<word> {
    function div(x: word, y: word) returns (word) {
        return uDivChecked(x, y);
    }
}

impl UncheckedMod<word> {
    function mod(x: word, y: word) returns (word) {
        return uModChecked(x, y);
    }
}

// --- uint8 ---
impl UncheckedAdd<uint8> {
    function add(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(Add.add(Typedef.rep(x), Typedef.rep(y)), 0xff));
    }
}

impl UncheckedSub<uint8> {
    function sub(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(Sub.sub(Typedef.rep(x), Typedef.rep(y)), 0xff));
    }
}

impl UncheckedMul<uint8> {
    function mul(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(Mul.mul(Typedef.rep(x), Typedef.rep(y)), 0xff));
    }
}

impl UncheckedDiv<uint8> {
    function div(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMod<uint8> {
    function mod(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

// --- uint16 ---
impl UncheckedAdd<uint16> {
    function add(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(Add.add(Typedef.rep(x), Typedef.rep(y)), 0xffff));
    }
}

impl UncheckedSub<uint16> {
    function sub(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(Sub.sub(Typedef.rep(x), Typedef.rep(y)), 0xffff));
    }
}

impl UncheckedMul<uint16> {
    function mul(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(Mul.mul(Typedef.rep(x), Typedef.rep(y)), 0xffff));
    }
}

impl UncheckedDiv<uint16> {
    function div(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMod<uint16> {
    function mod(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

// --- uint32 ---
impl UncheckedAdd<uint32> {
    function add(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(Add.add(Typedef.rep(x), Typedef.rep(y)), 0xffffffff));
    }
}

impl UncheckedSub<uint32> {
    function sub(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(Sub.sub(Typedef.rep(x), Typedef.rep(y)), 0xffffffff));
    }
}

impl UncheckedMul<uint32> {
    function mul(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(Mul.mul(Typedef.rep(x), Typedef.rep(y)), 0xffffffff));
    }
}

impl UncheckedDiv<uint32> {
    function div(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMod<uint32> {
    function mod(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

// --- uint64 ---
impl UncheckedAdd<uint64> {
    function add(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(Add.add(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffff));
    }
}

impl UncheckedSub<uint64> {
    function sub(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(Sub.sub(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffff));
    }
}

impl UncheckedMul<uint64> {
    function mul(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(Mul.mul(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffff));
    }
}

impl UncheckedDiv<uint64> {
    function div(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMod<uint64> {
    function mod(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

// --- uint128 ---
impl UncheckedAdd<uint128> {
    function add(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(Add.add(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffffffffffffffffffff));
    }
}

impl UncheckedSub<uint128> {
    function sub(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(Sub.sub(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffffffffffffffffffff));
    }
}

impl UncheckedMul<uint128> {
    function mul(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(Mul.mul(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffffffffffffffffffff));
    }
}

impl UncheckedDiv<uint128> {
    function div(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMod<uint128> {
    function mod(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

// --- uint256 ---
impl UncheckedAdd<uint256> {
    function add(x: uint256, y: uint256) returns (uint256) {
        return Typedef.abs(Add.add(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedSub<uint256> {
    function sub(x: uint256, y: uint256) returns (uint256) {
        return Typedef.abs(Sub.sub(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMul<uint256> {
    function mul(x: uint256, y: uint256) returns (uint256) {
        return Typedef.abs(Mul.mul(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedDiv<uint256> {
    function div(x: uint256, y: uint256) returns (uint256) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}

impl UncheckedMod<uint256> {
    function mod(x: uint256, y: uint256) returns (uint256) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
